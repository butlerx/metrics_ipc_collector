use crate::recorder::{socket, unnamed_pipe};
use thiserror::Error;

/// Errors that can occur when setting up or using metrics IPC.
///
/// This error type covers IO errors, recorder setup errors, and serialization/deserialization errors for metric events.
///
/// # See Also
/// - [`IPCSocketRecorder`](crate::recorder::socket::IPCSocketRecorder)
/// - [`IPCSocketCollector`](crate::collector::socket::IPCSocketCollector)
///
#[derive(Error, Debug)]
pub enum MetricsError {
    /// IO error setting up metrics reporter.
    #[error("IO error setting up metrics reporter {0}")]
    Io(#[from] std::io::Error),
    /// Failed to set `IPCSocketRecorder` as the global recorder.
    #[error("failed to set IPCSocketRecorder: {0}")]
    SocketRecorder(#[from] metrics::SetRecorderError<socket::IPCSocketRecorder>),
    /// Failed to set `IPCPipeRecorder` as the global recorder.
    #[error("failed to set IPCPipeRecorder: {0}")]
    PipeRecorder(#[from] metrics::SetRecorderError<unnamed_pipe::IPCPipeRecorder>),
    /// Could not serialize metric event.
    #[error("could not serialize event: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    /// Failed to deserialize metric event.
    #[error("failed to deserialize event: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
    /// The Tokio-backed pipe recorder was created outside a Tokio runtime.
    #[cfg(feature = "tokio")]
    #[error("Tokio runtime required for the async pipe recorder")]
    TokioRuntimeRequired,
    /// The pipe collector's receiver was already consumed.
    #[error("receiver already consumed")]
    PipeCollectorConsumed,
}
