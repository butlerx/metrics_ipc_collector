#![warn(clippy::pedantic, clippy::nursery, clippy::cargo, clippy::perf)]

//! # `metrics_ipc_collector`
//!
//! A metrics collector using IPC for multi-process metrics aggregation.
//!
//! ## Async Support
//!
//! Async support is available via the `tokio` feature flag. When enabled, all collector operations use async tasks and require a Tokio runtime. Enable with:
//!
//! ```toml
//! [dependencies]
//! metrics_ipc_collector = { version = "...", features = ["tokio"] }
//! ```
//!
//! If the `tokio` feature is not enabled, the collector uses threads and blocking IO.
//!
//! See README and examples for details.

mod collector;
mod error;
mod events;
mod recorder;

#[deprecated(note = "use IPCSocketCollector")]
pub use collector::socket::IPCSocketCollector as IPCCollector;
pub use collector::{
    socket::IPCSocketCollector,
    unnamed_pipe::{IPCPipeCollector, PipeSender},
};
pub use error::MetricsError;
#[deprecated(note = "use IPCSocketRecorder")]
pub use recorder::socket::IPCSocketRecorder as IPCRecorder;
#[deprecated(note = "use IPCSocketRecorderBuilder")]
pub use recorder::socket::IPCSocketRecorderBuilder as IPCRecorderBuilder;
pub use recorder::{
    socket::{IPCSocketRecorder, IPCSocketRecorderBuilder},
    unnamed_pipe::IPCPipeRecorder,
};
