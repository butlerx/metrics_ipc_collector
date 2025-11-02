//! `IPCCollector` implementation for metrics aggregation via IPC.
//!
//! ## Async Support
//!
//! This module supports async metric collection when the `tokio` feature flag is enabled.
//! - With `tokio` enabled, all collector operations use async tasks and require a Tokio runtime.
//! - Without `tokio`, the collector uses threads and blocking IO.
//!
//! See crate-level docs and README for details.

use crate::{
    error::MetricsError,
    events::{MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation},
};
#[cfg(feature = "tokio")]
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions};
#[cfg(not(feature = "tokio"))]
use interprocess::local_socket::{ListenerNonblockingMode, Stream, prelude::*};
use std::path::PathBuf;
#[cfg(not(feature = "tokio"))]
use std::{
    io::{BufRead, BufReader},
    thread,
};
#[cfg(feature = "tokio")]
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    task,
};

/// Collects metrics from multiple processes via IPC.
///
/// The `IPCCollector` listens on a local socket for incoming metric events from other processes.
/// It supports both synchronous (threaded) and asynchronous (Tokio) operation, depending on the crate feature flags.
///
/// # Examples
///
/// Basic usage:
/// ```rust
/// use metrics_ipc_collector::IPCCollector;
/// let collector = IPCCollector::default();
/// collector.start_collecting()?;
/// ```
///
/// See [`start_collecting`](#method.start_collecting) for more details and error handling.
///
/// # Feature Flags
/// - `tokio`: Enables async support. Requires a Tokio runtime.
///
/// # Errors
/// Creating the collector may fail if the socket file cannot be created or removed.
///
/// # See Also
/// - [`IPCRecorderBuilder`](crate::recorder::IPCRecorderBuilder)
/// - [`MetricsError`](crate::error::MetricsError)
///
/// Collects metrics from multiple processes via IPC.
///
/// The `IPCCollector` listens on a local socket for incoming metric events from other processes.
/// It supports both synchronous (threaded) and asynchronous (Tokio) operation, depending on the crate feature flags.
///
/// # Examples
///
/// Basic usage:
/// ```rust
/// use metrics_ipc_collector::IPCCollector;
/// let collector = IPCCollector::default();
/// collector.start_collecting()?;
/// ```
///
/// See [`start_collecting`](#method.start_collecting) for more details and error handling.
///
/// # Feature Flags
/// - `tokio`: Enables async support. Requires a Tokio runtime.
///
/// # Errors
/// Creating the collector may fail if the socket file cannot be created or removed.
///
/// # See Also
/// - [`IPCRecorderBuilder`](crate::recorder::IPCRecorderBuilder)
/// - [`MetricsError`](crate::error::MetricsError)
///
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
    ///
    /// This function spawns a thread (default) or an async Tokio task (if the `tokio` feature is enabled)
    /// that listens for incoming connections on the socket and processes metric events.
    ///
    /// - **Async Support:** If the `tokio` feature is enabled, this function uses async tasks and requires a Tokio runtime.
    ///   Otherwise, it uses threads and blocking IO.
    ///
    /// The metrics collected can then be exported using any of the regular metric export crates.
    /// If the socket file already exists, it will be removed before starting the collector.
    ///
    /// # Example
    /// ```
    /// use metrics_ipc_collector::IPCCollector;
    /// let collector = IPCCollector::default();
    /// if let Err(e) = collector.start_collecting() {
    ///     eprintln!("Failed to start metrics collector: {}", e);
    /// }
    /// ```
    ///
    /// # Feature Flags
    /// - `tokio`: Enables async support. Requires a Tokio runtime.
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

        #[cfg(not(feature = "tokio"))]
        thread::spawn(move || {
            if let Err(e) = run_collector(socket_path.clone()) {
                log::error!("Metrics collector error: {e}");
            }
            // Clean up socket file on shutdown
            let _ = std::fs::remove_file(&socket_file);
        });

        #[cfg(feature = "tokio")]
        task::spawn(async move {
            if let Err(e) = run_collector(socket_path.clone()).await {
                log::error!("Metrics collector error: {e}");
            }
            // Clean up socket file on shutdown
            let _ = std::fs::remove_file(&socket_file);
        });

        Ok(())
    }
}

// We can safely filter out any errors from the incoming stream
#[cfg(not(feature = "tokio"))]
fn filter_streams(conn: std::io::Result<Stream>) -> Option<Stream> {
    conn.ok()
}

#[cfg(not(feature = "tokio"))]
fn run_collector(socket_path: String) -> Result<(), MetricsError> {
    let socket_name = if GenericNamespaced::is_supported() {
        socket_path.to_ns_name::<GenericNamespaced>()?
    } else {
        format!("/tmp/{socket_path}").to_fs_name::<GenericFilePath>()?
    };

    let listener = ListenerOptions::new().name(socket_name).create_sync()?;
    listener.set_nonblocking(ListenerNonblockingMode::Both)?;

    for stream in listener.incoming().filter_map(filter_streams) {
        thread::spawn(move || {
            let mut reader = BufReader::new(stream);
            let mut buffer: Vec<u8> = Vec::new();

            loop {
                buffer.clear();
                match reader.read_until(b'\n', &mut buffer) {
                    Ok(0) => break,
                    Ok(_) => match MetricEvent::try_from(&buffer) {
                        Ok(MetricEvent::Metadata(metadata)) => handle_metadata_event(metadata),
                        Ok(MetricEvent::Metric(metric)) => handle_metric_event(metric),
                        Err(e) => log::trace!("{e}"),
                    },
                    // If we encounter an error reading from the stream, we just skip it
                    Err(_) => {}
                }
            }
        });
    }
    Ok(())
}

#[cfg(feature = "tokio")]
async fn run_collector(socket_path: String) -> Result<(), MetricsError> {
    let socket_name = if GenericNamespaced::is_supported() {
        socket_path.to_ns_name::<GenericNamespaced>()?
    } else {
        format!("/tmp/{socket_path}").to_fs_name::<GenericFilePath>()?
    };

    let listener = ListenerOptions::new().name(socket_name).create_tokio()?;

    loop {
        if let Ok(stream) = listener.accept().await {
            task::spawn(async move {
                let mut reader = BufReader::new(stream);
                let mut buffer: Vec<u8> = Vec::new();

                loop {
                    buffer.clear();
                    match reader.read_until(b'\n', &mut buffer).await {
                        Ok(0) => break,
                        Ok(_) => match MetricEvent::try_from(&buffer) {
                            Ok(MetricEvent::Metadata(metadata)) => {
                                handle_metadata_event(metadata);
                            }
                            Ok(MetricEvent::Metric(metric)) => handle_metric_event(metric),
                            Err(e) => log::trace!("{e}"),
                        },
                        // If we encounter an error reading from the stream, we just skip it
                        Err(_) => {}
                    }
                }
            });
        }
    }
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
                    .map(|(k, v)| (k.clone(), v.clone()))
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
                    .map(|(k, v)| (k.clone(), v.clone()))
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
                    .map(|(k, v)| (k.clone(), v.clone()))
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
                    .map(|(k, v)| (k.clone(), v.clone()))
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
                    .map(|(k, v)| (k.clone(), v.clone()))
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
                    .map(|(k, v)| (k.clone(), v.clone()))
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
