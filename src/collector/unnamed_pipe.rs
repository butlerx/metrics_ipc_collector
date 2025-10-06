use super::handlers::{handle_metadata_event, handle_metric_event};
use crate::{error::MetricsError, events::MetricEvent};
#[cfg(feature = "tokio")]
use interprocess::unnamed_pipe::tokio::{Recver, Sender, pipe};
#[cfg(not(feature = "tokio"))]
use interprocess::unnamed_pipe::{Recver, Sender, pipe};
#[cfg(not(feature = "tokio"))]
use std::{
    io::{BufRead, BufReader},
    thread,
};
#[cfg(feature = "tokio")]
use tokio::{
    io::{AsyncBufReadExt, BufReader},
    task,
};

#[cfg(not(feature = "tokio"))]
pub type PipeSender = interprocess::unnamed_pipe::Sender;
#[cfg(feature = "tokio")]
pub type PipeSender = interprocess::unnamed_pipe::tokio::Sender;

pub struct IPCPipeCollector {
    receiver: Recver,
}

impl IPCPipeCollector {
    /// Creates a new collector with the receiving end of an unnamed pipe.
    /// Returns both the collector and the sending handle that should be transferred
    /// to child processes.
    ///
    /// # Example
    /// ```
    /// let (collector, sender_handle) = metrics_ipc_collector::IPCPipeCollector::new().unwrap();
    /// // Pass sender_handle to child process via inheritance or serialization
    /// collector.start_collecting().unwrap();
    /// ```
    ///
    /// # Errors
    /// Returns an error if pipe creation fails.
    pub fn new() -> Result<(Self, Sender), MetricsError> {
        let (sender, receiver) = pipe()?;

        Ok((Self { receiver }, sender))
    }

    /// Starts collecting metrics from the unnamed pipe.
    /// This consumes the receiver and spawns a thread/task to process incoming metrics.
    ///
    /// # Errors
    /// Returns an error if the receiver has already been consumed or if spawning fails.
    pub fn start_collecting(self) -> Result<(), MetricsError> {
        let receiver = self.receiver;

        #[cfg(not(feature = "tokio"))]
        thread::spawn(move || {
            if let Err(e) = run_collector(receiver) {
                log::error!("Metrics collector error: {e}");
            }
        });

        #[cfg(feature = "tokio")]
        task::spawn(async move {
            if let Err(e) = run_collector(receiver).await {
                log::error!("Metrics collector error: {e}");
            }
        });

        Ok(())
    }
}

#[cfg(not(feature = "tokio"))]
fn run_collector(receiver: Recver) -> Result<(), MetricsError> {
    let mut reader = BufReader::new(receiver);
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer) {
            Ok(0) => {
                // EOF - sender closed
                log::info!("Metrics sender closed, stopping collector");
                break;
            }
            Ok(_) => match MetricEvent::try_from(&buffer) {
                Ok(MetricEvent::Metadata(metadata)) => handle_metadata_event(metadata),
                Ok(MetricEvent::Metric(metric)) => handle_metric_event(metric),
                Err(e) => log::trace!("Failed to parse metric event: {e}"),
            },
            Err(e) => {
                log::error!("Error reading from pipe: {e}");
                break;
            }
        }
    }
    Ok(())
}

#[cfg(feature = "tokio")]
async fn run_collector(receiver: Recver) -> Result<(), MetricsError> {
    // Convert to tokio async pipe
    let receiver = receiver;
    let mut reader = BufReader::new(receiver);
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        buffer.clear();
        match reader.read_until(b'\n', &mut buffer).await {
            Ok(0) => {
                // EOF - sender closed
                log::info!("Metrics sender closed, stopping collector");
                break;
            }
            Ok(_) => match MetricEvent::try_from(&buffer) {
                Ok(MetricEvent::Metadata(metadata)) => handle_metadata_event(metadata),
                Ok(MetricEvent::Metric(metric)) => handle_metric_event(metric),
                Err(e) => log::trace!("Failed to parse metric event: {e}"),
            },
            Err(e) => {
                log::error!("Error reading from pipe: {e}");
                break;
            }
        }
    }
    Ok(())
}
