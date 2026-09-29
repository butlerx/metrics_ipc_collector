//! Transport-independent recorder internals.
//!
//! In immediate mode every metric update becomes one event. With blocking IO
//! the event is written on the caller's thread before the call returns, so
//! short-lived processes do not lose updates. With the Tokio pipe writer the
//! event goes onto a bounded queue drained by a task.
//!
//! In batching mode updates only touch in-process cells, which a flusher
//! drains on an interval and once more when the recorder is dropped.

use crate::{
    events::{MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation},
    framing,
};
use std::{
    collections::{BTreeMap, HashMap},
    io::Write,
    mem,
    sync::{
        Arc, Mutex, PoisonError, RwLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Default number of events that can wait for the writer before new ones are dropped.
#[cfg(feature = "tokio")]
pub const DEFAULT_QUEUE_CAPACITY: usize = 8192;

/// Options shared by every recorder builder.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecorderConfig {
    #[cfg(feature = "tokio")]
    pub queue_capacity: Option<usize>,
    pub flush_interval: Option<Duration>,
}

/// Bounded queue from metric handles to the Tokio writer task.
///
/// Sending never blocks. When the writer falls behind, events are dropped
/// rather than stalling the caller or growing memory without bound.
#[cfg(feature = "tokio")]
#[derive(Debug, Clone)]
struct EventQueue {
    sender: tokio::sync::mpsc::Sender<MetricEvent>,
    dropped: Arc<AtomicU64>,
}

#[cfg(feature = "tokio")]
impl EventQueue {
    fn new(sender: tokio::sync::mpsc::Sender<MetricEvent>) -> Self {
        Self {
            sender,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    fn send(&self, event: MetricEvent) {
        if self.sender.try_send(event).is_err() && self.dropped.fetch_add(1, Ordering::Relaxed) == 0
        {
            log::warn!("Metrics IPC queue is full or closed, dropping events");
        }
    }
}

/// A blocking transport shared by every handle, written under a lock.
struct InlineWriter {
    writer: Mutex<Box<dyn Write + Send>>,
    failures: AtomicU64,
}

impl std::fmt::Debug for InlineWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InlineWriter").finish_non_exhaustive()
    }
}

impl InlineWriter {
    fn write_events(&self, events: impl IntoIterator<Item = MetricEvent>) {
        let mut out = Vec::new();
        for event in events {
            encode_into(&event, &mut out);
        }
        if out.is_empty() {
            return;
        }
        let result = framing::write_all_blocking(&mut *lock(&self.writer), &out);
        if let Err(e) = result
            && self.failures.fetch_add(1, Ordering::Relaxed) == 0
        {
            log::error!("Failed to write metric events: {e}");
        }
    }
}

/// State behind a blocking recorder. Dropping it flushes any batched updates.
#[derive(Debug)]
struct SyncShared {
    writer: InlineWriter,
    batch: Option<Arc<BatchRegistry>>,
}

impl SyncShared {
    fn flush(&self) {
        if let Some(batch) = &self.batch {
            self.writer.write_events(batch.drain());
        }
    }
}

impl Drop for SyncShared {
    fn drop(&mut self) {
        self.flush();
    }
}

/// Where a recorder's events go.
#[derive(Debug, Clone)]
enum Sink {
    Inline(Arc<SyncShared>),
    #[cfg(feature = "tokio")]
    Queue(EventQueue),
}

impl Sink {
    fn send(&self, event: MetricEvent) {
        match self {
            Self::Inline(shared) => shared.writer.write_events([event]),
            #[cfg(feature = "tokio")]
            Self::Queue(queue) => queue.send(event),
        }
    }
}

fn metric_data(key: &metrics::Key, operation: MetricOperation) -> MetricData {
    MetricData {
        name: key.name().to_string(),
        labels: key
            .labels()
            .map(|label| (label.key().to_owned(), label.value().to_owned()))
            .collect::<BTreeMap<_, _>>(),
        operation,
    }
}

/// Handle that sends one event per metric update.
#[derive(Debug)]
struct ImmediateHandle {
    key: metrics::Key,
    sink: Sink,
}

impl ImmediateHandle {
    fn push(&self, operation: MetricOperation) {
        self.sink
            .send(MetricEvent::Metric(metric_data(&self.key, operation)));
    }
}

impl metrics::CounterFn for ImmediateHandle {
    fn increment(&self, value: u64) {
        self.push(MetricOperation::IncrementCounter(value));
    }

    fn absolute(&self, value: u64) {
        self.push(MetricOperation::SetCounter(value));
    }
}

impl metrics::GaugeFn for ImmediateHandle {
    fn increment(&self, value: f64) {
        self.push(MetricOperation::IncrementGauge(value));
    }

    fn decrement(&self, value: f64) {
        self.push(MetricOperation::DecrementGauge(value));
    }

    fn set(&self, value: f64) {
        self.push(MetricOperation::SetGauge(value));
    }
}

impl metrics::HistogramFn for ImmediateHandle {
    fn record(&self, value: f64) {
        self.push(MetricOperation::RecordHistogram(value));
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default)]
struct CounterState {
    absolute: Option<u64>,
    delta: u64,
}

/// Counter updates accumulated between flushes.
#[derive(Debug, Default)]
struct CounterCell(Mutex<CounterState>);

impl CounterCell {
    fn take(&self) -> impl Iterator<Item = MetricOperation> {
        let state = mem::take(&mut *lock(&self.0));
        let absolute = state.absolute.map(MetricOperation::SetCounter);
        let delta = (state.delta > 0).then_some(MetricOperation::IncrementCounter(state.delta));
        absolute.into_iter().chain(delta)
    }
}

impl metrics::CounterFn for CounterCell {
    fn increment(&self, value: u64) {
        let mut state = lock(&self.0);
        state.delta = state.delta.saturating_add(value);
    }

    fn absolute(&self, value: u64) {
        *lock(&self.0) = CounterState {
            absolute: Some(value),
            delta: 0,
        };
    }
}

#[derive(Debug, Default)]
struct GaugeState {
    set: Option<f64>,
    delta: f64,
    dirty: bool,
}

/// Gauge updates accumulated between flushes.
#[derive(Debug, Default)]
struct GaugeCell(Mutex<GaugeState>);

impl GaugeCell {
    fn take(&self) -> Option<MetricOperation> {
        let state = mem::take(&mut *lock(&self.0));
        if !state.dirty {
            return None;
        }
        Some(
            state
                .set
                .map_or(MetricOperation::IncrementGauge(state.delta), |value| {
                    MetricOperation::SetGauge(value + state.delta)
                }),
        )
    }

    fn adjust(&self, delta: f64) {
        let mut state = lock(&self.0);
        state.delta += delta;
        state.dirty = true;
    }
}

impl metrics::GaugeFn for GaugeCell {
    fn increment(&self, value: f64) {
        self.adjust(value);
    }

    fn decrement(&self, value: f64) {
        self.adjust(-value);
    }

    fn set(&self, value: f64) {
        *lock(&self.0) = GaugeState {
            set: Some(value),
            delta: 0.0,
            dirty: true,
        };
    }
}

/// Histogram samples accumulated between flushes.
#[derive(Debug, Default)]
struct HistogramCell(Mutex<Vec<f64>>);

impl HistogramCell {
    fn take(&self) -> Option<MetricOperation> {
        let samples = mem::take(&mut *lock(&self.0));
        (!samples.is_empty()).then_some(MetricOperation::RecordHistogramBatch(samples))
    }
}

impl metrics::HistogramFn for HistogramCell {
    fn record(&self, value: f64) {
        lock(&self.0).push(value);
    }
}

type CellMap<T> = RwLock<HashMap<metrics::Key, Arc<T>>>;

fn cell<T: Default>(map: &CellMap<T>, key: &metrics::Key) -> Arc<T> {
    if let Some(cell) = map.read().unwrap_or_else(PoisonError::into_inner).get(key) {
        return cell.clone();
    }
    map.write()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(key.clone())
        .or_default()
        .clone()
}

fn drain<T>(
    map: &CellMap<T>,
    take: impl Fn(&T) -> Vec<MetricOperation>,
    events: &mut Vec<MetricEvent>,
) {
    let map = map.read().unwrap_or_else(PoisonError::into_inner);
    for (key, cell) in map.iter() {
        events.extend(
            take(cell)
                .into_iter()
                .map(|operation| MetricEvent::Metric(metric_data(key, operation))),
        );
    }
}

/// Metric cells for batching mode, shared by handles and the writer.
#[derive(Debug, Default)]
struct BatchRegistry {
    counters: CellMap<CounterCell>,
    gauges: CellMap<GaugeCell>,
    histograms: CellMap<HistogramCell>,
}

impl BatchRegistry {
    fn drain(&self) -> Vec<MetricEvent> {
        let mut events = Vec::new();
        drain(&self.counters, |c| c.take().collect(), &mut events);
        drain(
            &self.gauges,
            |g| g.take().into_iter().collect(),
            &mut events,
        );
        drain(
            &self.histograms,
            |h| h.take().into_iter().collect(),
            &mut events,
        );
        events
    }
}

/// The recorder-facing side: turns `metrics` calls into queued events or cell
/// updates.
#[derive(Debug, Clone)]
pub struct Core {
    sink: Sink,
    batch: Option<Arc<BatchRegistry>>,
}

impl Core {
    pub fn describe(
        &self,
        key_name: &metrics::KeyName,
        kind: MetricKind,
        unit: Option<metrics::Unit>,
        description: &metrics::SharedString,
    ) {
        self.sink.send(MetricEvent::Metadata(MetricMetadata {
            name: key_name.as_str().to_string(),
            kind,
            unit: unit.map(|u| u.as_str().to_string()),
            description: description.to_string(),
        }));
    }

    fn immediate(&self, key: &metrics::Key) -> Arc<ImmediateHandle> {
        Arc::new(ImmediateHandle {
            key: key.clone(),
            sink: self.sink.clone(),
        })
    }

    pub fn counter(&self, key: &metrics::Key) -> metrics::Counter {
        if let Some(batch) = &self.batch {
            return metrics::Counter::from_arc(cell(&batch.counters, key));
        }
        metrics::Counter::from_arc(self.immediate(key))
    }

    pub fn gauge(&self, key: &metrics::Key) -> metrics::Gauge {
        if let Some(batch) = &self.batch {
            return metrics::Gauge::from_arc(cell(&batch.gauges, key));
        }
        metrics::Gauge::from_arc(self.immediate(key))
    }

    pub fn histogram(&self, key: &metrics::Key) -> metrics::Histogram {
        if let Some(batch) = &self.batch {
            return metrics::Histogram::from_arc(cell(&batch.histograms, key));
        }
        metrics::Histogram::from_arc(self.immediate(key))
    }
}

/// Implements [`metrics::Recorder`] for a type with a `core: Core` field.
macro_rules! delegate_recorder {
    ($recorder:ty) => {
        impl metrics::Recorder for $recorder {
            fn describe_counter(
                &self,
                key_name: metrics::KeyName,
                unit: Option<metrics::Unit>,
                description: metrics::SharedString,
            ) {
                self.core.describe(
                    &key_name,
                    $crate::events::MetricKind::Counter,
                    unit,
                    &description,
                );
            }

            fn describe_gauge(
                &self,
                key_name: metrics::KeyName,
                unit: Option<metrics::Unit>,
                description: metrics::SharedString,
            ) {
                self.core.describe(
                    &key_name,
                    $crate::events::MetricKind::Gauge,
                    unit,
                    &description,
                );
            }

            fn describe_histogram(
                &self,
                key_name: metrics::KeyName,
                unit: Option<metrics::Unit>,
                description: metrics::SharedString,
            ) {
                self.core.describe(
                    &key_name,
                    $crate::events::MetricKind::Histogram,
                    unit,
                    &description,
                );
            }

            fn register_counter(
                &self,
                key: &metrics::Key,
                _meta: &metrics::Metadata<'_>,
            ) -> metrics::Counter {
                self.core.counter(key)
            }

            fn register_gauge(
                &self,
                key: &metrics::Key,
                _meta: &metrics::Metadata<'_>,
            ) -> metrics::Gauge {
                self.core.gauge(key)
            }

            fn register_histogram(
                &self,
                key: &metrics::Key,
                _meta: &metrics::Metadata<'_>,
            ) -> metrics::Histogram {
                self.core.histogram(key)
            }
        }
    };
}
pub(crate) use delegate_recorder;

fn batch_for(config: RecorderConfig) -> Option<Arc<BatchRegistry>> {
    config
        .flush_interval
        .map(|_| Arc::new(BatchRegistry::default()))
}

/// Encodes and buffers an event, logging and skipping ones that cannot be encoded.
fn encode_into(event: &MetricEvent, out: &mut Vec<u8>) {
    match framing::encode(event) {
        Ok(frame) => out.extend_from_slice(&frame),
        Err(e) => log::error!("Failed to encode metric event: {e}"),
    }
}

/// Builds a recorder core that writes with blocking IO.
///
/// Immediate-mode updates are written on the caller's thread. In batching mode
/// a flusher thread writes the batch every interval, and once more when the
/// last recorder clone is dropped.
///
pub fn sync_core<W: Write + Send + 'static>(writer: W, config: RecorderConfig) -> Core {
    let batch = batch_for(config);
    let shared = Arc::new(SyncShared {
        writer: InlineWriter {
            writer: Mutex::new(Box::new(writer)),
            failures: AtomicU64::new(0),
        },
        batch: batch.clone(),
    });

    if let Some(interval) = config.flush_interval {
        let shared = Arc::downgrade(&shared);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(interval);
                // Stop once every recorder clone is gone; the final flush
                // happens in `SyncShared::drop`.
                let Some(shared) = shared.upgrade() else {
                    break;
                };
                shared.flush();
            }
        });
    }

    Core {
        sink: Sink::Inline(shared),
        batch,
    }
}

/// Starts a writer as a Tokio task using async IO.
#[cfg(feature = "tokio")]
pub fn spawn_task_writer<W>(
    runtime: &tokio::runtime::Handle,
    writer: W,
    config: RecorderConfig,
) -> Core
where
    W: tokio::io::AsyncWrite + Unpin + Send + 'static,
{
    let (sender, receiver) =
        tokio::sync::mpsc::channel(config.queue_capacity.unwrap_or(DEFAULT_QUEUE_CAPACITY));
    let batch = batch_for(config);
    let writer_batch = batch.clone();

    runtime.spawn(async move {
        if let Err(e) = run_task_writer(writer, receiver, writer_batch, config).await {
            log::error!("Failed to write metric events: {e}");
        }
    });

    Core {
        sink: Sink::Queue(EventQueue::new(sender)),
        batch,
    }
}

#[cfg(feature = "tokio")]
async fn run_task_writer<W>(
    mut writer: W,
    mut receiver: tokio::sync::mpsc::Receiver<MetricEvent>,
    batch: Option<Arc<BatchRegistry>>,
    config: RecorderConfig,
) -> std::io::Result<()>
where
    W: tokio::io::AsyncWrite + Unpin,
{
    use tokio::io::AsyncWriteExt;

    let mut ticker = config.flush_interval.map(|interval| {
        let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        ticker
    });
    let mut out = Vec::new();

    loop {
        out.clear();
        let closed = tokio::select! {
            event = receiver.recv() => {
                let closed = event.is_none();
                if let Some(event) = event {
                    encode_into(&event, &mut out);
                    // Write whatever else is already waiting in one go.
                    while let Ok(event) = receiver.try_recv() {
                        encode_into(&event, &mut out);
                    }
                }
                closed
            },
            () = tick(ticker.as_mut()) => {
                if let Some(batch) = &batch {
                    for event in batch.drain() {
                        encode_into(&event, &mut out);
                    }
                }
                false
            }
        };

        if closed && let Some(batch) = &batch {
            for event in batch.drain() {
                encode_into(&event, &mut out);
            }
        }
        if !out.is_empty() {
            // No flush: nothing is buffered, see `framing::write_all_blocking`.
            writer.write_all(&out).await?;
        }
        if closed {
            return Ok(());
        }
    }
}

#[cfg(feature = "tokio")]
async fn tick(ticker: Option<&mut tokio::time::Interval>) {
    match ticker {
        Some(ticker) => {
            ticker.tick().await;
        }
        None => std::future::pending().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics::{CounterFn, GaugeFn, HistogramFn};

    #[test]
    fn counter_cell_sends_absolute_then_later_increments() {
        let cell = CounterCell::default();
        cell.increment(5);
        cell.absolute(100);
        cell.increment(2);
        cell.increment(3);

        let ops: Vec<_> = cell.take().collect();
        assert!(matches!(
            ops.as_slice(),
            [
                MetricOperation::SetCounter(100),
                MetricOperation::IncrementCounter(5)
            ]
        ));
        assert_eq!(cell.take().count(), 0, "cell should reset after a flush");
    }

    #[test]
    fn gauge_cell_folds_updates_into_one_operation() {
        let cell = GaugeCell::default();
        assert!(cell.take().is_none(), "untouched gauges send nothing");

        cell.increment(4.0);
        cell.decrement(1.0);
        assert!(
            matches!(cell.take(), Some(MetricOperation::IncrementGauge(v)) if (v - 3.0).abs() < f64::EPSILON)
        );

        cell.increment(10.0);
        cell.set(2.0);
        cell.increment(0.5);
        assert!(
            matches!(cell.take(), Some(MetricOperation::SetGauge(v)) if (v - 2.5).abs() < f64::EPSILON)
        );
    }

    #[test]
    fn histogram_cell_batches_samples() {
        let cell = HistogramCell::default();
        cell.record(1.0);
        cell.record(2.0);
        assert!(matches!(
            cell.take(),
            Some(MetricOperation::RecordHistogramBatch(ref v)) if v == &[1.0, 2.0]
        ));
        assert!(cell.take().is_none());
    }

    #[test]
    fn batching_registry_reuses_cells_per_key() {
        let registry = BatchRegistry::default();
        let key = metrics::Key::from_parts("requests", vec![metrics::Label::new("route", "/")]);
        cell(&registry.counters, &key).increment(1);
        cell(&registry.counters, &key).increment(2);

        let events = registry.drain();
        assert_eq!(events.len(), 1);
        let MetricEvent::Metric(metric) = &events[0] else {
            panic!("expected a metric event");
        };
        assert_eq!(metric.labels.get("route").map(String::as_str), Some("/"));
        assert!(matches!(
            metric.operation,
            MetricOperation::IncrementCounter(3)
        ));
    }

    #[cfg(feature = "tokio")]
    #[test]
    fn full_queue_drops_instead_of_blocking() {
        let (sender, _receiver) = tokio::sync::mpsc::channel(1);
        let queue = EventQueue::new(sender);
        let handle = ImmediateHandle {
            key: metrics::Key::from_name("requests"),
            sink: Sink::Queue(queue.clone()),
        };

        for _ in 0..3 {
            metrics::CounterFn::increment(&handle, 1);
        }
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 2);
    }
}
