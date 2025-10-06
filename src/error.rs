use crate::recorder::{socket, unnamed_pipe};
use thiserror::Error;

#[derive(Error, Debug)]
pub enum MetricsError {
    #[error("IO error setting up metrics reporter {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to set IPCSocketRecorder: {0}")]
    SocketRecorder(#[from] metrics::SetRecorderError<socket::IPCSocketRecorder>),
    #[error("failed to set IPCPipeRecorder: {0}")]
    PipeRecorder(#[from] metrics::SetRecorderError<unnamed_pipe::IPCPipeRecorder>),
    #[error("couldnt serialize event: {0}")]
    Serialization(#[from] rmp_serde::encode::Error),
    #[error("failed to deserialize event: {0}")]
    Deserialization(#[from] rmp_serde::decode::Error),
    #[error("Receiver already consumed")]
    PipeCollectorConsumed,
}
