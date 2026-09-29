use super::core::{Core, RecorderConfig, delegate_recorder, sync_core};
use crate::error::MetricsError;
use interprocess::local_socket::{GenericFilePath, GenericNamespaced, prelude::*};
use std::time::Duration;

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

#[derive(Debug)]
pub struct IPCSocketRecorderBuilder {
    socket_path: String,
    config: RecorderConfig,
}

impl Default for IPCSocketRecorderBuilder {
    fn default() -> Self {
        Self {
            socket_path: "metrics_collector.sock".into(),
            config: RecorderConfig::default(),
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
        let socket_name = if GenericNamespaced::is_supported() {
            self.socket_path.to_ns_name::<GenericNamespaced>()?
        } else {
            let socket_path = self.socket_path;
            format!("/tmp/{socket_path}").to_fs_name::<GenericFilePath>()?
        };

        let stream = LocalSocketStream::connect(socket_name)?;
        Ok(IPCSocketRecorder::with_config(stream, self.config))
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
        metrics::set_global_recorder(self.build_recorder()?).map_err(Into::into)
    }
}
