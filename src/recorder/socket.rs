use super::{
    core::{Core, RecorderConfig, delegate_recorder, sync_core},
    transport::{PlainTransport, Transport},
};
use crate::{error::MetricsError, framing, socket_addr::SocketAddr};
use interprocess::local_socket::prelude::*;
use std::{
    io,
    path::PathBuf,
    time::{Duration, Instant},
};

/// First delay before retrying a failed connection.
const MIN_RECONNECT_DELAY: Duration = Duration::from_millis(100);
/// Longest delay between connection attempts.
const MAX_RECONNECT_DELAY: Duration = Duration::from_secs(5);

/// An IPC recorder.
///
/// Each update is written to the socket before the call returns, and blocks if
/// the collector is not keeping up. Use
/// [`flush_interval`](IPCSocketRecorderBuilder::flush_interval) to replace
/// per-update IO with periodic batches.
///
/// A recorder built with [`IPCSocketRecorderBuilder`] reconnects when the
/// collector goes away, backing off from 100ms up to 5s between attempts.
/// Updates made while disconnected are dropped.
#[derive(Debug, Clone)]
pub struct IPCSocketRecorder {
    core: Core,
}

delegate_recorder!(IPCSocketRecorder);

impl IPCSocketRecorder {
    /// Creates a socket recorder backed by an established local socket stream,
    /// using default options.
    #[must_use]
    ///
    /// The recorder cannot reconnect, since it does not know the address; use
    /// [`IPCSocketRecorderBuilder`] for that.
    pub fn new(stream: LocalSocketStream) -> Self {
        Self {
            core: sync_core(Box::new(PlainTransport(stream)), &RecorderConfig::default()),
        }
    }
}

/// A socket transport that reconnects with exponential backoff.
struct ReconnectingSocket {
    addr: SocketAddr,
    stream: Option<LocalSocketStream>,
    hello: Option<Vec<u8>>,
    delay: Duration,
    retry_at: Instant,
    reconnects: u64,
}

impl ReconnectingSocket {
    fn new(addr: SocketAddr, stream: LocalSocketStream) -> Self {
        Self {
            addr,
            stream: Some(stream),
            hello: None,
            delay: MIN_RECONNECT_DELAY,
            retry_at: Instant::now(),
            reconnects: 0,
        }
    }

    fn connect(&self) -> io::Result<LocalSocketStream> {
        let mut stream = LocalSocketStream::connect(self.addr.to_name()?)?;
        if let Some(hello) = &self.hello {
            framing::write_all_blocking(&mut stream, hello)?;
        }
        Ok(stream)
    }

    fn ensure_connected(&mut self) -> io::Result<()> {
        if self.stream.is_some() {
            return Ok(());
        }
        let now = Instant::now();
        if now < self.retry_at {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                format!(
                    "waiting to reconnect to the metrics collector on {}",
                    self.addr
                ),
            ));
        }
        match self.connect() {
            Ok(stream) => {
                log::info!("Reconnected to the metrics collector on {}", self.addr);
                self.stream = Some(stream);
                self.reconnects += 1;
                self.delay = MIN_RECONNECT_DELAY;
                Ok(())
            }
            Err(e) => {
                self.retry_at = now + self.delay;
                self.delay = (self.delay * 2).min(MAX_RECONNECT_DELAY);
                Err(e)
            }
        }
    }
}

impl Transport for ReconnectingSocket {
    fn start(&mut self, hello: Option<Vec<u8>>) -> io::Result<()> {
        self.hello = hello;
        match (&mut self.stream, &self.hello) {
            (Some(stream), Some(hello)) => framing::write_all_blocking(stream, hello),
            _ => Ok(()),
        }
    }

    fn send(&mut self, frames: &[u8]) -> io::Result<()> {
        // A failed write usually means the collector restarted, so reconnect
        // straight away once before giving up on these frames.
        let mut last_error = None;
        for _ in 0..2 {
            self.ensure_connected()?;
            let Some(stream) = self.stream.as_mut() else {
                continue;
            };
            match framing::write_all_blocking(stream, frames) {
                Ok(()) => return Ok(()),
                Err(e) => {
                    log::debug!("Lost the metrics collector on {}: {e}", self.addr);
                    self.stream = None;
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| io::ErrorKind::NotConnected.into()))
    }

    fn reconnects(&self) -> u64 {
        self.reconnects
    }
}

#[derive(Debug, Default)]
pub struct IPCSocketRecorderBuilder {
    addr: SocketAddr,
    config: RecorderConfig,
}

impl IPCSocketRecorderBuilder {
    /// Connects to a namespaced socket name. Defaults to `metrics_collector.sock`.
    ///
    /// Must match [`IPCSocketCollector::socket`](crate::IPCSocketCollector::socket),
    /// which describes how names map to each platform.
    #[must_use]
    pub fn socket(mut self, name: impl Into<String>) -> Self {
        self.addr = SocketAddr::Namespaced(name.into());
        self
    }

    /// Connects to a socket file at `path`, replacing [`socket`](Self::socket).
    ///
    /// Must match [`IPCSocketCollector::path`](crate::IPCSocketCollector::path).
    #[must_use]
    pub fn path(mut self, path: impl Into<PathBuf>) -> Self {
        self.addr = SocketAddr::Path(path.into());
        self
    }

    /// Batches metric updates locally and sends them every `interval`.
    ///
    /// Without this, every update is its own IPC message. With it, updates only
    /// touch an in-process cell: counter increments and gauge changes are
    /// merged, and histogram samples are sent together. Use this on hot paths.
    ///
    /// Pending updates are flushed when the recorder is dropped. A global
    /// recorder is never dropped, so updates made in the last interval before
    /// the process exits may be lost.
    #[must_use]
    pub const fn flush_interval(mut self, interval: Duration) -> Self {
        self.config.flush_interval = Some(interval);
        self
    }

    /// Labels every metric from this recorder with `key="value"`.
    ///
    /// The labels are sent once per connection rather than with every update.
    /// They override labels of the same name on individual metrics, and are
    /// overridden by labels set on the collector.
    #[must_use]
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.config.set_label(key, value);
        self
    }

    /// Adds several labels to every metric from this recorder. See
    /// [`with_label`](Self::with_label).
    #[must_use]
    pub fn with_labels<K, V>(self, labels: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        labels
            .into_iter()
            .fold(self, |builder, (key, value)| builder.with_label(key, value))
    }

    /// Declares this recorder as generation `generation` of the sender
    /// `name`, sent once per connection with the recorder labels.
    ///
    /// When several senders declare the same `name`, the collector only lets
    /// the highest generation set gauges; counters and histograms from every
    /// generation are kept. A parent process that knows the order of its
    /// children should set this on the collector instead, which the sender
    /// cannot override; see [`IPCPipeCollector::source`](crate::IPCPipeCollector::source).
    #[must_use]
    pub fn source(mut self, name: impl Into<String>, generation: u64) -> Self {
        self.config.source = Some(crate::events::Source {
            name: name.into(),
            generation,
        });
        self
    }

    /// Also reports the recorder's own counters to the collector:
    /// `metrics_ipc_recorder_dropped_events_total` and
    /// `metrics_ipc_recorder_reconnects_total`, labelled with the process id.
    /// Off by default.
    #[must_use]
    pub const fn internal_metrics(mut self, enabled: bool) -> Self {
        self.config.internal_metrics = enabled;
        self
    }

    /// Connects to the socket and builds the recorder without installing it
    /// globally.
    ///
    /// # Errors
    /// Returns an error if the first connection cannot be established. Later
    /// disconnections are handled by reconnecting.
    pub fn build_recorder(self) -> Result<IPCSocketRecorder, MetricsError> {
        let stream = LocalSocketStream::connect(self.addr.to_name()?)?;
        let transport = ReconnectingSocket::new(self.addr, stream);
        Ok(IPCSocketRecorder {
            core: sync_core(Box::new(transport), &self.config),
        })
    }

    /// Builds the IPC recorder and sets it as the global recorder.
    /// This function connects to the configured IPC socket and sets up the recorder.
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
        metrics::set_global_recorder(self.build_recorder()?).map_err(Into::into)
    }
}
