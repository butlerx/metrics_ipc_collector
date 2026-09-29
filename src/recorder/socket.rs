use super::core::{Core, RecorderConfig, delegate_recorder, sync_core};
use crate::{error::MetricsError, socket_addr::SocketAddr};
use interprocess::local_socket::prelude::*;
use std::{path::PathBuf, time::Duration};

/// An IPC recorder.
///
/// Each update is written to the socket before the call returns, and blocks if
/// the collector is not keeping up. Use
/// [`flush_interval`](IPCSocketRecorderBuilder::flush_interval) to replace
/// per-update IO with periodic batches.
#[derive(Debug, Clone)]
pub struct IPCSocketRecorder {
    core: Core,
}

delegate_recorder!(IPCSocketRecorder);

impl IPCSocketRecorder {
    /// Creates a socket recorder backed by an established local socket stream,
    /// using default options.
    #[must_use]
    pub fn new(stream: LocalSocketStream) -> Self {
        Self::with_config(stream, RecorderConfig::default())
    }

    fn with_config(stream: LocalSocketStream, config: RecorderConfig) -> Self {
        Self {
            core: sync_core(stream, config),
        }
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

    /// Connects to the socket and builds the recorder without installing it
    /// globally.
    ///
    /// # Errors
    /// Returns an error if the IPC connection cannot be established.
    pub fn build_recorder(self) -> Result<IPCSocketRecorder, MetricsError> {
        let stream = LocalSocketStream::connect(self.addr.to_name()?)?;
        Ok(IPCSocketRecorder::with_config(stream, self.config))
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
