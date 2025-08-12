use crate::{
    error::MetricsError,
    events::{MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation},
};
use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerNonblockingMode, ListenerOptions, Stream,
    prelude::*,
};
use std::{
    io::{BufRead, BufReader},
    path::PathBuf,
    thread,
};

pub struct IPCCollector {
    socket_path: String,
}

impl Default for IPCCollector {
    fn default() -> Self {
        Self {
            socket_path: "metrics_collector.sock".into(),
        }
    }
}

impl IPCCollector {
    /// Sets the path for the IPC socket file.
    #[must_use]
    pub fn socket(mut self, socket_path: &str) -> Self {
        self.socket_path = socket_path.to_string();
        self
    }

    /// Sets up the IPC collector to start collecting metrics from the specified socket.
    /// This function spawns a thread that listens for incoming connections on the socket and
    /// processes metric events.
    /// The metrics collected can then be exported using any of the regular metric export crates.
    /// If the socket file already exists, it will be removed before starting the collector.
    ///
    /// # Example
    /// ```rust
    /// use metrics_ipc_collector::IPCCollector;
    /// let collector = IPCCollector::default();
    /// if let Err(e) = collector.start_collecting() {
    ///     eprintln!("Failed to start metrics collector: {}", e);
    /// }
    /// ```
    ///
    /// # Errors
    /// This function will return an error if it fails to create the socket file or if there are issues
    /// with the IPC communication.
    pub fn start_collecting(self) -> Result<(), MetricsError> {
        let socket_path = self.socket_path;
        let socket_file: PathBuf = format!("/tmp/{socket_path}").into();
        if socket_file.exists() {
            std::fs::remove_file(&socket_file)?;
        }

        thread::spawn(move || {
            if let Err(e) = run_collector(socket_path.clone()) {
                log::error!("Metrics collector error: {e}");
            }
            // Clean up socket file on shutdown
            let _ = std::fs::remove_file(&socket_file);
        });

        Ok(())
    }
}

fn handle_error(conn: std::io::Result<Stream>) -> Option<Stream> {
    match conn {
        Ok(c) => Some(c),
        // If the connection fails, log the error and return None
        Err(e) => {
            log::warn!("Failed to accept connection: {e}");
            None
        }
    }
}

fn line_filter(line: Result<String, std::io::Error>) -> Option<String> {
    match line {
        Ok(l) if !l.trim().is_empty() => Some(l),
        _ => None,
    }
}

fn run_collector(socket_path: String) -> Result<(), MetricsError> {
    let socket_name = if GenericNamespaced::is_supported() {
        socket_path.to_ns_name::<GenericNamespaced>()?
    } else {
        format!("/tmp/{socket_path}").to_fs_name::<GenericFilePath>()?
    };

    let listener = ListenerOptions::new().name(socket_name).create_sync()?;
    listener.set_nonblocking(ListenerNonblockingMode::Both)?;

    for stream in listener.incoming().filter_map(handle_error) {
        thread::spawn(move || {
            let reader = BufReader::new(stream);
            for line in reader.lines().filter_map(line_filter) {
                match serde_json::from_str::<MetricEvent>(&line) {
                    Ok(MetricEvent::Metadata(metadata)) => handle_metadata_event(metadata),
                    Ok(MetricEvent::Metric(metric)) => handle_metric_event(metric),
                    Err(e) => {
                        log::error!("Failed to deserialize metric event: {e} (line: {line})");
                    }
                }
            }
        });
    }
    Ok(())
}

fn handle_metric_event(metric: MetricData) {
    match metric.operation {
        MetricOperation::IncrementCounter(value) => {
            if metric.labels.is_empty() {
                metrics::counter!(metric.name).increment(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::counter!(metric.name, &labels).increment(value);
            }
        }
        MetricOperation::SetCounter(value) => {
            if metric.labels.is_empty() {
                metrics::counter!(metric.name).absolute(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::counter!(metric.name, &labels).absolute(value);
            }
        }
        MetricOperation::IncrementGauge(value) => {
            if metric.labels.is_empty() {
                metrics::gauge!(metric.name).increment(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::gauge!(metric.name, &labels).increment(value);
            }
        }
        MetricOperation::DecrementGauge(value) => {
            if metric.labels.is_empty() {
                metrics::gauge!(metric.name).decrement(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::gauge!(metric.name, &labels).decrement(value);
            }
        }
        MetricOperation::SetGauge(value) => {
            if metric.labels.is_empty() {
                metrics::gauge!(metric.name).set(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::gauge!(metric.name, &labels).set(value);
            }
        }
        MetricOperation::RecordHistogram(value) => {
            if metric.labels.is_empty() {
                metrics::histogram!(metric.name).record(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::histogram!(metric.name, &labels).record(value);
            }
        }
    }
}

fn handle_metadata_event(metadata: MetricMetadata) {
    let unit = metadata
        .unit
        .clone()
        .and_then(|ref u| metrics::Unit::from_string(u));

    match metadata.kind {
        MetricKind::Counter => {
            if let Some(unit) = unit {
                metrics::describe_counter!(metadata.name, unit, metadata.description);
            } else {
                metrics::describe_counter!(metadata.name, metadata.description);
            }
        }
        MetricKind::Gauge => {
            if let Some(unit) = unit {
                metrics::describe_gauge!(metadata.name, unit, metadata.description);
            } else {
                metrics::describe_gauge!(metadata.name, metadata.description);
            }
        }
        MetricKind::Histogram => {
            if let Some(unit) = unit {
                metrics::describe_histogram!(metadata.name, unit, metadata.description);
            } else {
                metrics::describe_histogram!(metadata.name, metadata.description);
            }
        }
    }
}
