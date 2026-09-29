use super::{
    handle::{CollectorHandle, StopSignal},
    handlers::{CollectorOptions, StreamState},
};
use crate::{error::MetricsError, framing, socket_addr::SocketAddr};
use interprocess::local_socket::ListenerOptions;
#[cfg(feature = "tokio")]
use interprocess::local_socket::tokio::{Listener, prelude::*};
#[cfg(not(feature = "tokio"))]
use interprocess::local_socket::{Listener, ListenerNonblockingMode, Stream, prelude::*};
use std::path::PathBuf;
#[cfg(not(feature = "tokio"))]
use std::{io::BufReader, thread, time::Duration};
#[cfg(feature = "tokio")]
use tokio::{io::BufReader, runtime::Handle as TokioHandle, task};

/// How long the blocking listener sleeps between polls for new connections.
#[cfg(not(feature = "tokio"))]
const ACCEPT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Collects metrics sent by [`IPCSocketRecorder`](crate::IPCSocketRecorder)s
/// over a local socket.
///
/// # Security
/// Any process that can connect to the socket can create, change and overwrite
/// metrics. Which processes can connect depends on the address:
///
/// - [`socket`](Self::socket) names on Linux live in the abstract namespace,
///   which has no permission checks: every process in the same network
///   namespace can connect.
/// - [`socket`](Self::socket) names on other Unix systems are files in
///   `/run/user/<uid>` if it exists, otherwise in the shared `/tmp`.
/// - [`path`](Self::path) sockets follow normal file permissions, so put them
///   in a directory only the intended users can access.
///
/// If you spawn the senders yourself, prefer [`IPCPipeCollector`](crate::IPCPipeCollector):
/// only processes holding the pipe handle can write to it. See the README for
/// more detail.
#[derive(Default)]
pub struct IPCSocketCollector {
    addr: SocketAddr,
    labels: Vec<(String, String)>,
    internal_metrics: bool,
}

impl IPCSocketCollector {
    /// Listens on a namespaced socket name. Defaults to `metrics_collector.sock`.
    ///
    /// - **Linux and Android:** the abstract socket namespace. No file is
    ///   created.
    /// - **Other Unix:** a socket file named `name` in `/run/user/<uid>` if
    ///   that directory exists, otherwise in `/tmp`.
    /// - **Windows:** the named pipe `\\.\pipe\<name>`.
    ///
    /// The recorder must use the same name with
    /// [`IPCSocketRecorderBuilder::socket`](crate::IPCSocketRecorderBuilder::socket).
    #[must_use]
    pub fn socket(mut self, name: impl Into<String>) -> Self {
        self.addr = SocketAddr::Namespaced(name.into());
        self
    }

    /// Listens on a socket file at `path`, replacing [`socket`](Self::socket).
    ///
    /// On Unix this is a Unix domain socket file; access follows the file and
    /// directory permissions. On Windows the path must be a named pipe path
    /// such as `\\.\pipe\metrics`.
    ///
    /// The recorder must use the same path with
    /// [`IPCSocketRecorderBuilder::path`](crate::IPCSocketRecorderBuilder::path).
    #[must_use]
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.addr = SocketAddr::Path(path.into());
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

    /// Also records the collector's own metrics, labelled with the collector
    /// labels. The connections gauge counts open connections: `metrics_ipc_collector_events_total`,
    /// `metrics_ipc_collector_decode_errors_total`,
    /// `metrics_ipc_collector_stream_errors_total` and the
    /// `metrics_ipc_collector_connections` gauge. Off by default.
    #[must_use]
    pub const fn internal_metrics(mut self, enabled: bool) -> Self {
        self.internal_metrics = enabled;
        self
    }

    /// Starts listening and spawns a thread/task that processes metric events
    /// from every connection.
    /// The metrics collected can then be exported using any of the regular metric export crates.
    ///
    /// On Unix, a stale socket file left by a collector that did not shut down
    /// cleanly is replaced. With [`path`](Self::path), anything at the path
    /// that is not a socket is left alone and reported as an error. The socket
    /// file is removed when the collector stops.
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
    /// Returns an error if the socket cannot be created, for example because
    /// another collector is already listening on it. With the `tokio` feature
    /// enabled, this also returns an error when called outside a Tokio runtime.
    pub fn start_collecting(self) -> Result<CollectorHandle, MetricsError> {
        ensure_not_listening(&self.addr)?;
        // Nobody answered, so an existing socket at the address is stale.
        let options = ListenerOptions::new().name(self.addr.to_name()?);
        let options = match &self.addr {
            SocketAddr::Path(path) => {
                remove_stale_socket(path)?;
                options
            }
            // Namespaced names only become files on non-Linux Unix, in a
            // location chosen by interprocess, so let it replace them.
            SocketAddr::Namespaced(_) => options.try_overwrite(true),
        };
        let collector_options = CollectorOptions::new(self.labels, self.internal_metrics);

        #[cfg(not(feature = "tokio"))]
        {
            let listener = options.create_sync()?;
            // Non-blocking accepts let the loop notice stop requests.
            listener.set_nonblocking(ListenerNonblockingMode::Accept)?;
            CollectorHandle::spawn(move |stop| run_collector(&listener, &collector_options, &stop))
                .map_err(Into::into)
        }

        #[cfg(feature = "tokio")]
        {
            let runtime =
                TokioHandle::try_current().map_err(|_| MetricsError::TokioRuntimeRequired)?;
            let listener = {
                let _guard = runtime.enter();
                options.create_tokio()?
            };
            Ok(CollectorHandle::spawn(&runtime, move |stop| {
                run_collector(listener, collector_options, stop)
            }))
        }
    }
}

/// Fails if a live collector already answers on `addr`.
///
/// `try_overwrite` deletes an existing socket file without checking whether
/// its listener is still alive, which would silently take over another
/// collector's socket.
fn ensure_not_listening(addr: &SocketAddr) -> std::io::Result<()> {
    use interprocess::local_socket::traits::Stream as _;

    match interprocess::local_socket::Stream::connect(addr.to_name()?) {
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::AddrInUse,
            format!("another metrics collector is already listening on {addr}"),
        )),
        Err(_) => Ok(()),
    }
}

/// Removes a leftover socket file at `path`, refusing to touch anything that
/// is not a socket.
fn remove_stale_socket(path: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;

        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => std::fs::remove_file(path),
            Ok(_) => Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("{} exists and is not a socket", path.display()),
            )),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
    // Named pipes disappear with their last handle, so nothing is left over.
    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

/// Logs why a connection ended. Wire errors are worth surfacing; ordinary
/// disconnects are not.
fn log_connection_error(e: &std::io::Error) {
    if e.kind() == std::io::ErrorKind::InvalidData {
        log::warn!("Dropping metrics connection: {e}");
    } else {
        log::debug!("Dropping metrics connection: {e}");
    }
}

#[cfg(not(feature = "tokio"))]
fn run_collector(listener: &Listener, options: &CollectorOptions, stop: &StopSignal) {
    while !stop.is_stopped() {
        match listener.accept() {
            Ok(stream) => {
                // On macOS and the BSDs, accepted sockets inherit the
                // listener's non-blocking flag; the reader thread needs
                // blocking reads.
                if let Err(e) = stream.set_nonblocking(false) {
                    log::debug!("Dropping metrics connection: {e}");
                    continue;
                }
                let options = options.clone();
                let stop = stop.clone();
                thread::spawn(move || read_connection(stream, options, &stop));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                thread::sleep(ACCEPT_POLL_INTERVAL);
            }
            Err(e) => log::debug!("Failed to accept metrics connection: {e}"),
        }
    }
}

#[cfg(not(feature = "tokio"))]
fn read_connection(stream: Stream, options: CollectorOptions, stop: &StopSignal) {
    let mut reader = BufReader::new(stream);
    let mut buffer: Vec<u8> = Vec::new();
    let mut state = StreamState::new(options);

    loop {
        match framing::read_frame(&mut reader, &mut buffer) {
            Ok(_) if stop.is_stopped() => break,
            Ok(true) => state.handle_frame(&buffer),
            Ok(false) => break,
            Err(e) => {
                log_connection_error(&e);
                state.stream_error();
                break;
            }
        }
    }
}

#[cfg(feature = "tokio")]
async fn run_collector(listener: Listener, options: CollectorOptions, mut stop: StopSignal) {
    let mut connections = task::JoinSet::new();

    loop {
        tokio::select! {
            biased;
            () = stop.stopped() => break,
            conn = listener.accept() => match conn {
                Ok(stream) => {
                    connections.spawn(read_connection(stream, options.clone(), stop.clone()));
                }
                Err(e) => log::debug!("Failed to accept metrics connection: {e}"),
            },
            // Reap finished connections so the set does not grow without bound.
            Some(_) = connections.join_next(), if !connections.is_empty() => {}
        }
    }

    connections.shutdown().await;
}

#[cfg(feature = "tokio")]
async fn read_connection(
    stream: LocalSocketStream,
    options: CollectorOptions,
    mut stop: StopSignal,
) {
    let mut reader = BufReader::new(stream);
    let mut buffer: Vec<u8> = Vec::new();
    let mut state = StreamState::new(options);

    loop {
        tokio::select! {
            biased;
            () = stop.stopped() => break,
            result = framing::read_frame_async(&mut reader, &mut buffer) => match result {
                Ok(true) => state.handle_frame(&buffer),
                Ok(false) => break,
                Err(e) => {
                    log_connection_error(&e);
                    state.stream_error();
                    break;
                }
            },
        }
    }
}
