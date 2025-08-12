#![warn(clippy::pedantic, clippy::nursery, clippy::cargo, clippy::perf)]

mod collector;
mod error;
mod events;
mod recorder;

pub use collector::IPCCollector;
pub use error::MetricsError;
pub use recorder::{IPCRecorder, IPCRecorderBuilder};
