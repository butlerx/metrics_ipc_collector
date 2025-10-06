use crate::{
    error::MetricsError,
    events::{MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation},
};
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, prelude::*};
use std::{
    collections::BTreeMap,
    io::Write,
    sync::{Arc, Mutex},
};

fn write_event(
    stream: &Arc<Mutex<LocalSocketStream>>,
    event: MetricEvent,
) -> Result<(), MetricsError> {
    let bytes: Vec<u8> = event.try_into()?;
    let mut stream = stream.lock().unwrap();
    stream.write_all(&bytes)?;
    stream.write_all(b"\n")?;
    stream.flush().map_err(Into::into)
}

#[derive(Debug)]
struct Handle {
    key: metrics::Key,
    stream: Arc<Mutex<LocalSocketStream>>,
}

impl Handle {
    const fn new(key: metrics::Key, stream: Arc<Mutex<LocalSocketStream>>) -> Self {
        Self { key, stream }
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
        let _ = write_event(&self.stream.clone(), MetricEvent::Metric(metric));
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

/// An IPC recorder.
#[derive(Debug, Clone)]
pub struct IPCSocketRecorder {
    stream: Arc<Mutex<LocalSocketStream>>,
}

impl IPCSocketRecorder {
    pub fn new(stream: LocalSocketStream) -> Self {
        Self {
            stream: Arc::new(Mutex::new(stream)),
        }
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
        let _ = write_event(&self.stream.clone(), MetricEvent::Metadata(metadata));
    }
}

impl metrics::Recorder for IPCSocketRecorder {
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
        metrics::Counter::from_arc(Arc::new(Handle::new(key.clone(), self.stream.clone())))
    }

    fn register_gauge(&self, key: &metrics::Key, _meta: &metrics::Metadata<'_>) -> metrics::Gauge {
        metrics::Gauge::from_arc(Arc::new(Handle::new(key.clone(), self.stream.clone())))
    }

    fn register_histogram(
        &self,
        key: &metrics::Key,
        _meta: &metrics::Metadata<'_>,
    ) -> metrics::Histogram {
        metrics::Histogram::from_arc(Arc::new(Handle::new(key.clone(), self.stream.clone())))
    }
}

#[derive(Debug)]
pub struct IPCSocketRecorderBuilder {
    socket_path: String,
}

impl Default for IPCSocketRecorderBuilder {
    fn default() -> Self {
        Self {
            socket_path: "metrics_collector.sock".into(),
        }
    }
}

impl IPCSocketRecorderBuilder {
    /// Sets the path for the IPC socket file.
    #[must_use]
    pub fn socket(mut self, socket_path: &str) -> Self {
        self.socket_path = socket_path.to_string();
        self
    }

    /// Builds the IPC recorder and sets it as the global recorder.
    /// This function connects to the IPC socket specified by `socket_path` and sets up the recorder.
    /// All metrics recorded after this call will be sent to the IPC socket.
    ///
    /// # Example
    /// ```rust
    /// use metrics_ipc_collector::IPCSocketRecorderBuilder;
    /// let builder = IPCSocketRecorderBuilder::default().socket("my_metrics.sock");
    /// if let Err(e) = builder.build() {
    ///     eprintln!("Failed to set up IPC recorder: {}", e);
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// Returns an error if the IPC connection cannot be established or if the recorder cannot be set.
    pub fn build(self) -> Result<(), MetricsError> {
        let socket_name = if GenericNamespaced::is_supported() {
            self.socket_path.to_ns_name::<GenericNamespaced>()?
        } else {
            let socket_path = self.socket_path;
            format!("/tmp/{socket_path}").to_fs_name::<GenericFilePath>()?
        };

        let stream = LocalSocketStream::connect(socket_name)?;
        stream.set_nonblocking(true)?;
        let recorder = IPCSocketRecorder::new(stream);
        metrics::set_global_recorder(recorder).map_err(Into::into)
    }
}
