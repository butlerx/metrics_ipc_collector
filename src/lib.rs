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

pub use collector::IPCCollector;
pub use error::MetricsError;
pub use recorder::{IPCRecorder, IPCRecorderBuilder};
