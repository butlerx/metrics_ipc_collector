use crate::recorder::IPCRecorder;
use thiserror::Error;

/// Errors that can occur when setting up or using metrics IPC.
///
/// This error type covers IO errors, recorder setup errors, and serialization/deserialization errors for metric events.
///
/// # See Also
/// - [`IPCRecorder`](crate::recorder::IPCRecorder)
/// - [`IPCCollector`](crate::collector::IPCCollector)
///
#[derive(Error, Debug)]
pub enum MetricsError {
    /// IO error setting up metrics reporter.
    #[error("IO error setting up metrics reporter {0}")]
    Io(#[from] std::io::Error),
    /// Failed to set `IPCRecorder` as the global recorder.
    #[error("failed to set IPCRecorder: {0}")]
    Recorder(#[from] metrics::SetRecorderError<IPCRecorder>),
    /// Could not serialize metric event.
    #[error("couldnt serialize event: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    /// Failed to deserialize metric event.
    #[error("failed to deserialize event: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
}
