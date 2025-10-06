#![warn(clippy::pedantic, clippy::nursery, clippy::cargo, clippy::perf)]

mod collector;
mod error;
mod events;
mod recorder;

pub use collector::{
    socket::IPCSocketCollector,
    unnamed_pipe::{IPCPipeCollector, PipeSender},
};
pub use error::MetricsError;
pub use recorder::{
    socket::{IPCSocketRecorder, IPCSocketRecorderBuilder},
    unnamed_pipe::IPCPipeRecorder,
};
