use crate::recorder::IPCRecorder;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MetricsError {
    #[error("IO error setting up metrics reporter {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to set IPCRecorder: {0}")]
    Recorder(#[from] metrics::SetRecorderError<IPCRecorder>),
    #[error("couldnt serialize event: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    #[error("failed to deserialize event: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
}
