use crate::{
    PipeSender,
    error::MetricsError,
    events::{MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation},
};
#[cfg(not(feature = "tokio"))]
use std::io::Write;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
#[cfg(feature = "tokio")]
use tokio::io::AsyncWriteExt;

#[cfg(not(feature = "tokio"))]
fn write_event(sender: &Arc<Mutex<PipeSender>>, event: MetricEvent) -> Result<(), MetricsError> {
    let bytes: Vec<u8> = event.try_into()?;
    let mut sender = sender.lock().unwrap();
    sender.write_all(&bytes)?;
    sender.write_all(b"\n")?;
    sender.flush().map_err(Into::into)
}

#[cfg(feature = "tokio")]
async fn write_event(
    sender: &Arc<Mutex<PipeSender>>,
    event: MetricEvent,
) -> Result<(), MetricsError> {
    let bytes: Vec<u8> = event.try_into()?;
    let mut sender = sender.lock().unwrap();
    sender.write_all(&bytes).await?;
    sender.write_all(b"\n").await?;
    sender.flush().await?;
    Ok(())
}

#[derive(Debug)]
struct Handle {
    key: metrics::Key,
    sender: Arc<Mutex<PipeSender>>,
}

impl Handle {
    const fn new(key: metrics::Key, sender: Arc<Mutex<PipeSender>>) -> Self {
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
        let _ = write_event(&self.sender, MetricEvent::Metric(metric));
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
    sender: Arc<Mutex<PipeSender>>,
}

impl IPCPipeRecorder {
    /// Builds the IPC recorder and sets it as the global recorder.
    /// Creates a new recorder from a raw pipe handle.
    /// This is typically called in a child process after receiving the handle
    /// from the parent process.
    ///
    /// # Example
    /// ```
    /// use metrics_ipc_collector::{PipeSender,IPCPipeRecorder};
    /// use std::sync::mpsc;
    ///
    /// let (_, handle_rx) = mpsc::sync_channel(1);
    /// let handle = handle_rx.recv().unwrap();
    /// let sender = PipeSender::from(handle);
    ///
    /// if let Err(e) = IPCPipeRecorder::build(sender) {
    ///     eprintln!("Failed to set up IPC recorder: {}", e);
    /// }
    /// ```
    ///
    /// # Errors
    /// Returns an error if the recorder cannot be set as the global recorder.
    #[must_use]
    pub fn build(sender: PipeSender) -> Result<(), MetricsError> {
        let recorder = Self {
            sender: Arc::new(Mutex::new(sender)),
        };
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
        let _ = write_event(&self.sender, MetricEvent::Metadata(metadata));
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
