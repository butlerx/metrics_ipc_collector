use super::{
    handle::{CollectorHandle, StopSignal},
    handlers::{ExtraLabels, handle_frame},
};
use crate::{error::MetricsError, framing};
#[cfg(feature = "tokio")]
use interprocess::local_socket::tokio::prelude::*;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, ListenerOptions};
#[cfg(not(feature = "tokio"))]
use interprocess::local_socket::{ListenerNonblockingMode, Stream, prelude::*};
use std::path::PathBuf;
#[cfg(not(feature = "tokio"))]
use std::{io::BufReader, thread, time::Duration};
#[cfg(feature = "tokio")]
use tokio::{io::BufReader, runtime::Handle as TokioHandle, task};

/// How long the blocking listener sleeps between polls for new connections.
#[cfg(not(feature = "tokio"))]
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub struct IPCSocketCollector {
    socket_path: String,
    labels: Vec<(String, String)>,
}

impl Default for IPCSocketCollector {
    fn default() -> Self {
        Self {
            socket_path: "metrics_collector.sock".into(),
            labels: Vec::new(),
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

    /// Adds a label to every metric received on this socket.
    ///
    /// Collector labels override labels of the same name set by the sender.
    #[must_use]
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        self.labels.retain(|(existing, _)| *existing != key);
        self.labels.push((key, value.into()));
        self
    }

    /// Adds several labels to every metric received on this socket.
    #[must_use]
    pub fn with_labels<K, V>(self, labels: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        labels.into_iter().fold(self, |collector, (key, value)| {
            collector.with_label(key, value)
        })
    }

    /// Sets up the IPC collector to start collecting metrics from the specified socket.
    /// This function spawns a thread/task that listens for incoming connections on the socket and
    /// processes metric events.
    /// The metrics collected can then be exported using any of the regular metric export crates.
    /// If the socket file already exists, it will be removed before starting the collector.
    ///
    /// The collector runs until [`CollectorHandle::stop`] is called.
    ///
    /// # Example
    /// ```no_run
    /// let collector = metrics_ipc_collector::IPCSocketCollector::default();
    /// if let Err(e) = collector.start_collecting() {
    ///     eprintln!("Failed to start metrics collector: {}", e);
    /// }
    /// ```
    ///
    /// # Errors
    /// This function will return an error if it fails to create the socket file or if there are issues
    /// with the IPC communication.
    pub fn start_collecting(self) -> Result<CollectorHandle, MetricsError> {
        #[cfg(feature = "tokio")]
        let runtime = TokioHandle::try_current().map_err(|_| MetricsError::TokioRuntimeRequired)?;

        let socket_path = self.socket_path;
        let socket_file: PathBuf = format!("/tmp/{socket_path}").into();
        if socket_file.exists() {
            std::fs::remove_file(&socket_file)?;
        }
        let labels: ExtraLabels = self.labels.into();

        #[cfg(not(feature = "tokio"))]
        let handle = CollectorHandle::spawn(move |stop| {
            if let Err(e) = run_collector(&socket_path, &labels, &stop) {
                log::error!("Metrics collector error: {e}");
            }
            // Clean up socket file on shutdown
            let _ = std::fs::remove_file(&socket_file);
        })?;

        #[cfg(feature = "tokio")]
        let handle = CollectorHandle::spawn(&runtime, move |stop| async move {
            if let Err(e) = run_collector(&socket_path, labels, stop).await {
                log::error!("Metrics collector error: {e}");
            }
            // Clean up socket file on shutdown
            let _ = std::fs::remove_file(&socket_file);
        });

        Ok(handle)
    }
}

fn socket_name(socket_path: &str) -> std::io::Result<interprocess::local_socket::Name<'static>> {
    if GenericNamespaced::is_supported() {
        socket_path.to_string().to_ns_name::<GenericNamespaced>()
    } else {
        format!("/tmp/{socket_path}").to_fs_name::<GenericFilePath>()
    }
}

#[cfg(not(feature = "tokio"))]
fn run_collector(
    socket_path: &str,
    labels: &ExtraLabels,
    stop: &StopSignal,
) -> Result<(), MetricsError> {
    let listener = ListenerOptions::new()
        .name(socket_name(socket_path)?)
        .create_sync()?;
    // Non-blocking accepts let the loop notice stop requests.
    listener.set_nonblocking(ListenerNonblockingMode::Accept)?;

    while !stop.is_stopped() {
        match listener.accept() {
            Ok(stream) => {
                let labels = labels.clone();
                let stop = stop.clone();
                thread::spawn(move || read_connection(stream, &labels, &stop));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(e) => log::debug!("Failed to accept metrics connection: {e}"),
        }
    }
    Ok(())
}

#[cfg(not(feature = "tokio"))]
fn read_connection(stream: Stream, labels: &ExtraLabels, stop: &StopSignal) {
    let mut reader = BufReader::new(stream);
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        match framing::read_frame(&mut reader, &mut buffer) {
            Ok(_) if stop.is_stopped() => break,
            Ok(true) => handle_frame(&buffer, labels),
            Ok(false) => break,
            Err(e) => {
                log::debug!("Dropping metrics connection: {e}");
                break;
            }
        }
    }
}

#[cfg(feature = "tokio")]
async fn run_collector(
    socket_path: &str,
    labels: ExtraLabels,
    mut stop: StopSignal,
) -> Result<(), MetricsError> {
    let listener = ListenerOptions::new()
        .name(socket_name(socket_path)?)
        .create_tokio()?;
    let mut connections = task::JoinSet::new();

    loop {
        tokio::select! {
            biased;
            () = stop.stopped() => break,
            conn = listener.accept() => match conn {
                Ok(stream) => {
                    connections.spawn(read_connection(stream, labels.clone(), stop.clone()));
                }
                Err(e) => log::debug!("Failed to accept metrics connection: {e}"),
            },
            // Reap finished connections so the set does not grow without bound.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }

    connections.shutdown().await;
    Ok(())
}

#[cfg(feature = "tokio")]
async fn read_connection(stream: LocalSocketStream, labels: ExtraLabels, mut stop: StopSignal) {
    let mut reader = BufReader::new(stream);
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        tokio::select! {
            biased;
            () = stop.stopped() => break,
            result = framing::read_frame_async(&mut reader, &mut buffer) => match result {
                Ok(true) => handle_frame(&buffer, &labels),
                Ok(false) => break,
                Err(e) => {
                    log::debug!("Dropping metrics connection: {e}");
                    break;
                }
            },
        }
    }
}
