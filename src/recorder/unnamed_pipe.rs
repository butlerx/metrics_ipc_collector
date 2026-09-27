use crate::{
    PipeSender,
    error::MetricsError,
    events::{MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation},
};
use std::{collections::BTreeMap, sync::Arc};
#[cfg(not(feature = "tokio"))]
use std::{io::Write, sync::Mutex};
#[cfg(feature = "tokio")]
use tokio::{
    io::AsyncWriteExt,
    runtime::Handle as TokioHandle,
    sync::mpsc::{self, UnboundedSender},
};

#[cfg(not(feature = "tokio"))]
type EventSender = Arc<Mutex<PipeSender>>;
#[cfg(feature = "tokio")]
type EventSender = UnboundedSender<MetricEvent>;

#[cfg(not(feature = "tokio"))]
fn write_event(sender: &EventSender, event: MetricEvent) -> Result<(), MetricsError> {
    let bytes: Vec<u8> = event.try_into()?;
    let mut sender = sender
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    sender.write_all(&bytes)?;
    sender.write_all(b"\n")?;
    sender.flush().map_err(Into::into)
}

#[cfg(not(feature = "tokio"))]
fn send_event(sender: &EventSender, event: MetricEvent) {
    if let Err(error) = write_event(sender, event) {
        log::error!("Failed to write metric event to pipe: {error}");
    }
}

#[cfg(feature = "tokio")]
fn send_event(sender: &EventSender, event: MetricEvent) {
    if sender.send(event).is_err() {
        log::error!("Failed to queue metric event: pipe writer stopped");
    }
}

#[cfg(feature = "tokio")]
fn spawn_writer(mut sender: PipeSender) -> Result<EventSender, MetricsError> {
    let runtime = TokioHandle::try_current().map_err(|_| MetricsError::TokioRuntimeRequired)?;
    let (event_sender, mut event_receiver) = mpsc::unbounded_channel::<MetricEvent>();

    runtime.spawn(async move {
        while let Some(event) = event_receiver.recv().await {
            let result = async {
                let bytes: Vec<u8> = event.try_into()?;
                sender.write_all(&bytes).await?;
                sender.write_all(b"\n").await?;
                sender.flush().await.map_err(MetricsError::from)
            }
            .await;

            if let Err(error) = result {
                log::error!("Failed to write metric event to pipe: {error}");
                break;
            }
        }
    });

    Ok(event_sender)
}

#[derive(Debug)]
struct Handle {
    key: metrics::Key,
    sender: EventSender,
}

impl Handle {
    const fn new(key: metrics::Key, sender: EventSender) -> Self {
        Self { key, sender }
    }

    fn push_metric(&self, key: &metrics::Key, op: MetricOperation) {
        let metric = MetricData {
            name: key.name().to_string(),
            labels: key
                .labels()
                .map(|label| (label.key().to_owned(), label.value().to_owned()))
                .collect::<BTreeMap<_, _>>(),
            operation: op,
        };
        send_event(&self.sender, MetricEvent::Metric(metric));
    }
}

impl metrics::CounterFn for Handle {
    fn increment(&self, value: u64) {
        self.push_metric(&self.key, MetricOperation::IncrementCounter(value));
    }

    fn absolute(&self, value: u64) {
        self.push_metric(&self.key, MetricOperation::SetCounter(value));
    }
}

impl metrics::GaugeFn for Handle {
    fn increment(&self, value: f64) {
        self.push_metric(&self.key, MetricOperation::IncrementGauge(value));
    }

    fn decrement(&self, value: f64) {
        self.push_metric(&self.key, MetricOperation::DecrementGauge(value));
    }

    fn set(&self, value: f64) {
        self.push_metric(&self.key, MetricOperation::SetGauge(value));
    }
}

impl metrics::HistogramFn for Handle {
    fn record(&self, value: f64) {
        self.push_metric(&self.key, MetricOperation::RecordHistogram(value));
    }
}

/// An IPC recorder using unnamed pipes.
#[derive(Debug, Clone)]
pub struct IPCPipeRecorder {
    sender: EventSender,
}

impl IPCPipeRecorder {
    /// Builds the IPC recorder and sets it as the global recorder.
    /// Creates a new recorder from a raw pipe handle.
    /// This is typically called in a child process after receiving the handle
    /// from the parent process.
    ///
    /// # Example
    /// ```no_run
    /// # fn main() -> Result<(), metrics_ipc_collector::MetricsError> {
    /// use metrics_ipc_collector::{IPCPipeCollector, IPCPipeRecorder};
    ///
    /// let (collector, sender) = IPCPipeCollector::new()?;
    /// collector.start_collecting()?;
    /// IPCPipeRecorder::build(sender)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns an error if the recorder cannot be set as the global recorder.
    /// With the `tokio` feature enabled, this also returns an error when called
    /// outside a Tokio runtime.
    pub fn build(sender: PipeSender) -> Result<(), MetricsError> {
        #[cfg(not(feature = "tokio"))]
        let sender = Arc::new(Mutex::new(sender));
        #[cfg(feature = "tokio")]
        let sender = spawn_writer(sender)?;

        let recorder = Self { sender };
        metrics::set_global_recorder(recorder).map_err(Into::into)
    }

    fn register_metric(
        &self,
        key_name: &metrics::KeyName,
        kind: MetricKind,
        unit: Option<metrics::Unit>,
        description: &metrics::SharedString,
    ) {
        let metadata = MetricMetadata {
            name: key_name.as_str().to_string(),
            kind,
            unit: unit.map(|u| u.as_str().to_string()),
            description: description.to_string(),
        };
        send_event(&self.sender, MetricEvent::Metadata(metadata));
    }
}

impl metrics::Recorder for IPCPipeRecorder {
    fn describe_counter(
        &self,
        key_name: metrics::KeyName,
        unit: Option<metrics::Unit>,
        description: metrics::SharedString,
    ) {
        self.register_metric(&key_name, MetricKind::Counter, unit, &description);
    }

    fn describe_gauge(
        &self,
        key_name: metrics::KeyName,
        unit: Option<metrics::Unit>,
        description: metrics::SharedString,
    ) {
        self.register_metric(&key_name, MetricKind::Gauge, unit, &description);
    }

    fn describe_histogram(
        &self,
        key_name: metrics::KeyName,
        unit: Option<metrics::Unit>,
        description: metrics::SharedString,
    ) {
        self.register_metric(&key_name, MetricKind::Histogram, unit, &description);
    }

    fn register_counter(
        &self,
        key: &metrics::Key,
        _meta: &metrics::Metadata<'_>,
    ) -> metrics::Counter {
        metrics::Counter::from_arc(Arc::new(Handle::new(key.clone(), self.sender.clone())))
    }

    fn register_gauge(&self, key: &metrics::Key, _meta: &metrics::Metadata<'_>) -> metrics::Gauge {
        metrics::Gauge::from_arc(Arc::new(Handle::new(key.clone(), self.sender.clone())))
    }

    fn register_histogram(
        &self,
        key: &metrics::Key,
        _meta: &metrics::Metadata<'_>,
    ) -> metrics::Histogram {
        metrics::Histogram::from_arc(Arc::new(Handle::new(key.clone(), self.sender.clone())))
    }
}

#[cfg(all(test, feature = "tokio"))]
mod tests {
    use super::*;
    use interprocess::unnamed_pipe::tokio::pipe;
    use std::time::Duration;
    use tokio::{
        io::{AsyncBufReadExt, BufReader},
        time::timeout,
    };

    #[tokio::test]
    async fn recorder_handle_writes_events_from_sync_callbacks() {
        let (sender, receiver) = pipe().expect("pipe should be created");
        let event_sender = spawn_writer(sender).expect("writer should start inside Tokio runtime");
        let handle = Handle::new(metrics::Key::from_name("requests"), event_sender);

        metrics::CounterFn::increment(&handle, 7);

        let mut reader = BufReader::new(receiver);
        let mut buffer = Vec::new();
        timeout(
            Duration::from_secs(1),
            reader.read_until(b'\n', &mut buffer),
        )
        .await
        .expect("writer should not stall")
        .expect("pipe should remain readable");

        let event = MetricEvent::try_from(&buffer).expect("event should deserialize");
        let MetricEvent::Metric(metric) = event else {
            panic!("expected a metric event");
        };
        assert_eq!(metric.name, "requests");
        assert!(metric.labels.is_empty());
        assert!(matches!(
            metric.operation,
            MetricOperation::IncrementCounter(7)
        ));
    }
}

#[cfg(all(test, not(feature = "tokio")))]
mod tests {
    use super::*;
    use interprocess::unnamed_pipe::pipe;
    use std::io::{BufRead, BufReader};

    #[test]
    fn recorder_handle_writes_events_from_sync_callbacks() {
        let (sender, receiver) = pipe().expect("pipe should be created");
        let event_sender = Arc::new(Mutex::new(sender));
        let handle = Handle::new(metrics::Key::from_name("requests"), event_sender);

        metrics::CounterFn::increment(&handle, 7);

        let mut reader = BufReader::new(receiver);
        let mut buffer = Vec::new();
        reader
            .read_until(b'\n', &mut buffer)
            .expect("pipe should remain readable");

        let event = MetricEvent::try_from(&buffer).expect("event should deserialize");
        let MetricEvent::Metric(metric) = event else {
            panic!("expected a metric event");
        };
        assert_eq!(metric.name, "requests");
        assert!(metric.labels.is_empty());
        assert!(matches!(
            metric.operation,
            MetricOperation::IncrementCounter(7)
        ));
    }
}
