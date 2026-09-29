//! Transport-independent recorder internals.
//!
//! In immediate mode every metric update becomes one event. With blocking IO
//! the event is written on the caller's thread before the call returns, so
//! short-lived processes do not lose updates. With the Tokio pipe writer the
//! event goes onto a bounded queue drained by a task.
//!
//! In batching mode updates only touch in-process cells, which a flusher
//! drains on an interval and once more when the recorder is dropped.

use super::transport::Transport;
use crate::{
    events::{Hello, MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation, Source},
    framing,
};
use std::{
    collections::{BTreeMap, HashMap},
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

/// Name of the counter of events the recorder could not deliver.
pub const DROPPED_EVENTS_METRIC: &str = "metrics_ipc_recorder_dropped_events_total";
/// Name of the counter of transport reconnections.
pub const RECONNECTS_METRIC: &str = "metrics_ipc_recorder_reconnects_total";

/// Options shared by every recorder builder.
#[derive(Debug, Clone, Default)]
pub struct RecorderConfig {
    #[cfg(feature = "tokio")]
    pub queue_capacity: Option<usize>,
    pub flush_interval: Option<Duration>,
    /// Labels sent once per connection in a hello frame.
    pub labels: BTreeMap<String, String>,
    /// Source and generation sent in the hello frame.
    pub source: Option<Source>,
    /// Report the recorder's own counters alongside the application's metrics.
    pub internal_metrics: bool,
}

impl RecorderConfig {
    /// Adds or replaces a hello label.
    pub fn set_label(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.labels.insert(key.into(), value.into());
    }

    /// Encodes the hello frame, if there are labels or a source to send.
    pub fn hello_frame(&self) -> Option<Vec<u8>> {
        if self.labels.is_empty() && self.source.is_none() {
            return None;
        }
        let hello = MetricEvent::Hello(Hello {
            labels: self.labels.clone(),
            source: self.source.clone(),
        });
        framing::encode(&hello)
            .inspect_err(|e| log::error!("Failed to encode hello frame: {e}"))
            .ok()
    }
}

/// One of the recorder's own counters.
///
/// Labelled with the process id: these are sent as absolute values, so two
/// recorders sharing a series would overwrite each other's counts.
fn counter_event(name: &str, value: u64) -> MetricEvent {
    MetricEvent::Metric(MetricData {
        name: name.to_string(),
        labels: BTreeMap::from([("pid".to_string(), std::process::id().to_string())]),
        operation: MetricOperation::SetCounter(value),
    })
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

/// Mutable state of a blocking recorder, guarded by one lock.
struct InlineState {
    transport: Box<dyn Transport>,
    /// Events that could not be written.
    dropped: u64,
    /// `(dropped, reconnects)` as last reported to the collector.
    reported: (u64, u64),
    /// Whether the last write failed, so failures are logged once per outage.
    failing: bool,
}

/// A blocking transport shared by every handle, written under a lock.
struct InlineWriter {
    state: Mutex<InlineState>,
    internal_metrics: bool,
}

impl std::fmt::Debug for InlineWriter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InlineWriter").finish_non_exhaustive()
    }
}

impl InlineWriter {
    fn write_events(&self, events: impl IntoIterator<Item = MetricEvent>) {
        let mut out = Vec::new();
        let mut count = 0;
        for event in events {
            if encode_into(&event, &mut out) {
                count += 1;
            }
        }
        if out.is_empty() {
            return;
        }

        let mut state = lock(&self.state);
        let snapshot = (state.dropped, state.transport.reconnects());
        let report = self.internal_metrics && snapshot != state.reported;
        if report {
            encode_into(&counter_event(DROPPED_EVENTS_METRIC, snapshot.0), &mut out);
            encode_into(&counter_event(RECONNECTS_METRIC, snapshot.1), &mut out);
        }

        match state.transport.send(&out) {
            Ok(()) => {
                if state.failing {
                    log::info!("Metric events are being delivered again");
                    state.failing = false;
                }
                if report {
                    state.reported = snapshot;
                }
            }
            Err(e) => {
                state.dropped += count;
                if !state.failing {
                    log::warn!("Failed to write metric events, dropping them: {e}");
                    state.failing = true;
                }
            }
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
    /// The counter's local value, shared by every handle for the same key.
    /// Only set for counters.
    total: Option<Arc<AtomicU64>>,
}

impl ImmediateHandle {
    fn push(&self, operation: MetricOperation) {
        self.sink
            .send(MetricEvent::Metric(metric_data(&self.key, operation)));
    }
}

// Counters are always sent as increments. `absolute` follows the `metrics`
// meaning ("at least this value"), applied to the local total, and sends the
// difference. Collectors then only ever add, so counters from a restarted or
// overlapping sender accumulate instead of being stuck at the old maximum.
impl metrics::CounterFn for ImmediateHandle {
    fn increment(&self, value: u64) {
        if let Some(total) = &self.total {
            total.fetch_add(value, Ordering::Relaxed);
        }
        self.push(MetricOperation::IncrementCounter(value));
    }

    fn absolute(&self, value: u64) {
        let previous = self
            .total
            .as_ref()
            .map_or(0, |total| total.fetch_max(value, Ordering::Relaxed));
        if value > previous {
            self.push(MetricOperation::IncrementCounter(value - previous));
        }
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
    /// The counter's local value.
    total: u64,
    /// How much of `total` has been flushed.
    sent: u64,
}

/// Counter updates accumulated between flushes, sent as one increment.
#[derive(Debug, Default)]
struct CounterCell(Mutex<CounterState>);

impl CounterCell {
    fn take(&self) -> Option<MetricOperation> {
        let delta = {
            let mut state = lock(&self.0);
            let delta = state.total - state.sent;
            state.sent = state.total;
            delta
        };
        (delta > 0).then_some(MetricOperation::IncrementCounter(delta))
    }
}

impl metrics::CounterFn for CounterCell {
    fn increment(&self, value: u64) {
        let mut state = lock(&self.0);
        state.total = state.total.saturating_add(value);
    }

    fn absolute(&self, value: u64) {
        let mut state = lock(&self.0);
        state.total = state.total.max(value);
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
        drain(
            &self.counters,
            |c| c.take().into_iter().collect(),
            &mut events,
        );
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
    /// Local counter values for immediate mode, keyed by metric.
    counter_totals: Arc<CellMap<AtomicU64>>,
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
            total: None,
        })
    }

    pub fn counter(&self, key: &metrics::Key) -> metrics::Counter {
        if let Some(batch) = &self.batch {
            return metrics::Counter::from_arc(cell(&batch.counters, key));
        }
        metrics::Counter::from_arc(Arc::new(ImmediateHandle {
            key: key.clone(),
            sink: self.sink.clone(),
            total: Some(cell(&self.counter_totals, key)),
        }))
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

fn batch_for(config: &RecorderConfig) -> Option<Arc<BatchRegistry>> {
    config
        .flush_interval
        .map(|_| Arc::new(BatchRegistry::default()))
}

/// Encodes and buffers an event, logging and skipping ones that cannot be
/// encoded. Returns whether the event was buffered.
fn encode_into(event: &MetricEvent, out: &mut Vec<u8>) -> bool {
    match framing::encode(event) {
        Ok(frame) => {
            out.extend_from_slice(&frame);
            true
        }
        Err(e) => {
            log::error!("Failed to encode metric event: {e}");
            false
        }
    }
}

/// Builds a recorder core that writes with blocking IO.
///
/// Immediate-mode updates are written on the caller's thread. In batching mode
/// a flusher thread writes the batch every interval, and once more when the
/// last recorder clone is dropped.
///
pub fn sync_core(mut transport: Box<dyn Transport>, config: &RecorderConfig) -> Core {
    if let Err(e) = transport.start(config.hello_frame()) {
        log::warn!("Failed to send metrics hello frame: {e}");
    }
    let batch = batch_for(config);
    let shared = Arc::new(SyncShared {
        writer: InlineWriter {
            state: Mutex::new(InlineState {
                transport,
                dropped: 0,
                reported: (0, 0),
                failing: false,
            }),
            internal_metrics: config.internal_metrics,
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
        counter_totals: Arc::default(),
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
    let batch = batch_for(&config);
    let queue = EventQueue::new(sender);
    let task = TaskWriter {
        receiver,
        batch: batch.clone(),
        dropped: config.internal_metrics.then(|| queue.dropped.clone()),
        config,
    };

    runtime.spawn(async move {
        if let Err(e) = task.run(writer).await {
            log::error!("Failed to write metric events: {e}");
        }
    });

    Core {
        sink: Sink::Queue(queue),
        batch,
        counter_totals: Arc::default(),
    }
}

/// State of the Tokio writer task.
#[cfg(feature = "tokio")]
struct TaskWriter {
    receiver: tokio::sync::mpsc::Receiver<MetricEvent>,
    batch: Option<Arc<BatchRegistry>>,
    /// The queue's drop counter, when internal metrics are enabled.
    dropped: Option<Arc<AtomicU64>>,
    config: RecorderConfig,
}

#[cfg(feature = "tokio")]
impl TaskWriter {
    async fn run<W>(mut self, mut writer: W) -> std::io::Result<()>
    where
        W: tokio::io::AsyncWrite + Unpin,
    {
        use tokio::io::AsyncWriteExt;

        if let Some(hello) = self.config.hello_frame() {
            writer.write_all(&hello).await?;
        }

        let mut ticker = self.config.flush_interval.map(|interval| {
            let mut ticker =
                tokio::time::interval_at(tokio::time::Instant::now() + interval, interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            ticker
        });
        let mut out = Vec::new();
        let mut reported_dropped = 0;

        loop {
            out.clear();
            let closed = tokio::select! {
                event = self.receiver.recv() => {
                    let closed = event.is_none();
                    if let Some(event) = event {
                        encode_into(&event, &mut out);
                        // Write whatever else is already waiting in one go.
                        while let Ok(event) = self.receiver.try_recv() {
                            encode_into(&event, &mut out);
                        }
                    }
                    closed
                },
                () = tick(ticker.as_mut()) => {
                    self.drain_batch(&mut out);
                    false
                }
            };

            if closed {
                self.drain_batch(&mut out);
            }
            if let Some(dropped) = &self.dropped {
                let dropped = dropped.load(Ordering::Relaxed);
                if dropped != reported_dropped {
                    encode_into(&counter_event(DROPPED_EVENTS_METRIC, dropped), &mut out);
                    reported_dropped = dropped;
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

    fn drain_batch(&self, out: &mut Vec<u8>) {
        if let Some(batch) = &self.batch {
            for event in batch.drain() {
                encode_into(&event, out);
            }
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
    fn counter_cell_sends_the_local_change_as_one_increment() {
        let cell = CounterCell::default();
        cell.increment(5);
        cell.absolute(100);
        cell.increment(2);
        cell.increment(3);
        // Same as a local counter: max(5, 100) + 2 + 3.
        assert!(matches!(
            cell.take(),
            Some(MetricOperation::IncrementCounter(105))
        ));
        assert!(cell.take().is_none(), "nothing changed since the flush");

        cell.absolute(50);
        assert!(cell.take().is_none(), "absolute never lowers a counter");
        cell.absolute(110);
        assert!(matches!(
            cell.take(),
            Some(MetricOperation::IncrementCounter(5))
        ));
    }

    /// A transport that keeps everything written to it.
    struct Capture(Arc<Mutex<Vec<u8>>>);

    impl Transport for Capture {
        fn start(&mut self, hello: Option<Vec<u8>>) -> std::io::Result<()> {
            hello.map_or(Ok(()), |hello| self.send(&hello))
        }

        fn send(&mut self, frames: &[u8]) -> std::io::Result<()> {
            lock(&self.0).extend_from_slice(frames);
            Ok(())
        }
    }

    fn captured_operations(bytes: &[u8]) -> Vec<MetricOperation> {
        let mut reader = std::io::Cursor::new(bytes);
        let mut buffer = Vec::new();
        let mut ops = Vec::new();
        while framing::read_frame(&mut reader, &mut buffer).unwrap() {
            if let Ok(MetricEvent::Metric(metric)) = MetricEvent::try_from(buffer.as_slice()) {
                ops.push(metric.operation);
            }
        }
        ops
    }

    #[test]
    fn immediate_absolute_sends_the_difference() {
        let bytes = Arc::new(Mutex::new(Vec::new()));
        let core = sync_core(Box::new(Capture(bytes.clone())), &RecorderConfig::default());
        let key = metrics::Key::from_name("requests");
        core.counter(&key).absolute(10);
        // A fresh handle for the same key shares the total.
        core.counter(&key).absolute(4);
        core.counter(&key).increment(1);
        core.counter(&key).absolute(15);

        let ops = captured_operations(&lock(&bytes));
        assert!(
            matches!(
                ops.as_slice(),
                [
                    MetricOperation::IncrementCounter(10),
                    MetricOperation::IncrementCounter(1),
                    MetricOperation::IncrementCounter(4),
                ]
            ),
            "{ops:?}"
        );
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
            total: None,
        };

        for _ in 0..3 {
            metrics::CounterFn::increment(&handle, 1);
        }
        assert_eq!(queue.dropped.load(Ordering::Relaxed), 2);
    }
}
