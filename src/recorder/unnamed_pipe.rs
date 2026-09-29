use super::core::{Core, RecorderConfig, delegate_recorder};
use crate::{PipeSender, error::MetricsError};
use std::time::Duration;
#[cfg(feature = "tokio")]
use tokio::runtime::Handle as TokioHandle;

#[cfg(all(feature = "tokio", unix))]
type OwnedPipeHandle = std::os::fd::OwnedFd;
#[cfg(all(feature = "tokio", windows))]
type OwnedPipeHandle = std::os::windows::io::OwnedHandle;

/// An IPC recorder using unnamed pipes.
///
/// Without the `tokio` feature, each update is written to the pipe before the
/// call returns, and blocks if the pipe is full. With the `tokio` feature,
/// updates go onto a bounded queue drained by a Tokio task and are dropped if
/// the queue fills up.
///
/// Either way, [`flush_interval`](IPCPipeRecorderBuilder::flush_interval)
/// replaces per-update IO with periodic batches.
#[derive(Debug, Clone)]
pub struct IPCPipeRecorder {
    core: Core,
}

delegate_recorder!(IPCPipeRecorder);

impl IPCPipeRecorder {
    /// Builds an IPC recorder with default options and sets it as the global
    /// recorder.
    ///
    /// This is typically called in a child process after receiving the handle
    /// from the parent process. Use [`IPCPipeRecorder::builder`] to change the
    /// defaults.
    ///
    /// # Example
    /// ```no_run
    /// # fn main() -> Result<(), metrics_ipc_collector::MetricsError> {
    /// use metrics_ipc_collector::{IPCPipeCollector, IPCPipeRecorder};
    ///
    /// let (collector, sender) = IPCPipeCollector::new()?;
    /// collector.start_collecting()?;
    /// IPCPipeRecorder::build(sender)?;
    /// # Ok(())
    /// # }
    /// ```
    ///
    /// # Errors
    /// Returns an error if the recorder cannot be set as the global recorder.
    /// With the `tokio` feature enabled, this also returns an error when called
    /// outside a Tokio runtime.
    pub fn build(sender: PipeSender) -> Result<(), MetricsError> {
        Self::builder(sender).build()
    }

    /// Returns a builder for a recorder that writes to `sender`.
    #[must_use]
    pub fn builder(sender: PipeSender) -> IPCPipeRecorderBuilder {
        IPCPipeRecorderBuilder {
            sender,
            config: RecorderConfig::default(),
        }
    }
}

/// Configures an [`IPCPipeRecorder`].
#[derive(Debug)]
pub struct IPCPipeRecorderBuilder {
    sender: PipeSender,
    config: RecorderConfig,
}

impl IPCPipeRecorderBuilder {
    /// Sets how many events can wait for the writer task before new ones are
    /// dropped. Defaults to 8192.
    #[cfg(feature = "tokio")]
    #[must_use]
    pub fn queue_capacity(mut self, capacity: usize) -> Self {
        self.config.queue_capacity = Some(capacity.max(1));
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

    /// Builds the recorder without installing it globally.
    ///
    /// Useful for layering or for `metrics::with_local_recorder`.
    ///
    /// # Errors
    /// With the `tokio` feature enabled, returns an error when called outside a
    /// Tokio runtime.
    #[cfg_attr(
        not(feature = "tokio"),
        allow(clippy::unnecessary_wraps, reason = "fallible with the tokio feature")
    )]
    pub fn build_recorder(self) -> Result<IPCPipeRecorder, MetricsError> {
        #[cfg(not(feature = "tokio"))]
        let core = super::core::sync_core(self.sender, self.config);

        #[cfg(feature = "tokio")]
        let core = {
            let runtime =
                TokioHandle::try_current().map_err(|_| MetricsError::TokioRuntimeRequired)?;
            let sender = interprocess::unnamed_pipe::tokio::Sender::try_from(
                OwnedPipeHandle::from(self.sender),
            )?;
            super::core::spawn_task_writer(&runtime, sender, self.config)
        };

        Ok(IPCPipeRecorder { core })
    }

    /// Builds the recorder and sets it as the global recorder.
    ///
    /// # Errors
    /// Returns an error if the recorder cannot be built or set as the global
    /// recorder.
    pub fn build(self) -> Result<(), MetricsError> {
        metrics::set_global_recorder(self.build_recorder()?).map_err(Into::into)
    }
}
