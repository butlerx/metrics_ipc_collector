use super::handlers::{handle_metadata_event, handle_metric_event};
use crate::{error::MetricsError, events::MetricEvent};
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

pub struct IPCSocketCollector {
    socket_path: String,
}

impl Default for IPCSocketCollector {
    fn default() -> Self {
        Self {
            socket_path: "metrics_collector.sock".into(),
        }
    }
}

impl IPCSocketCollector {
    /// Sets the path for the IPC socket file.
    #[must_use]
    pub fn socket(mut self, socket_path: &str) -> Self {
        self.socket_path = socket_path.to_string();
        self
    }

    /// Sets up the IPC collector to start collecting metrics from the specified socket.
    /// This function spawns a thread/task that listens for incoming connections on the socket and
    /// processes metric events.
    /// The metrics collected can then be exported using any of the regular metric export crates.
    /// If the socket file already exists, it will be removed before starting the collector.
    ///
    /// # Example
    /// ```
    /// let collector = metrics_ipc_collector::IPCSocketCollector::default();
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
