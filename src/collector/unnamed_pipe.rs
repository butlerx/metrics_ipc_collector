use super::{
    handle::{CollectorHandle, StopSignal},
    handlers::{ExtraLabels, handle_frame},
};
use crate::{error::MetricsError, framing};
use interprocess::unnamed_pipe::pipe;
#[cfg(feature = "tokio")]
use tokio::runtime::Handle as TokioHandle;

/// The sending end of an unnamed pipe, passed to [`IPCPipeRecorder`](crate::IPCPipeRecorder).
///
/// This is a plain blocking handle regardless of the `tokio` feature, so it can
/// be created and moved between processes without a Tokio runtime. Convert it
/// to and from `OwnedFd` (Unix) or `OwnedHandle` (Windows) with `From`.
pub type PipeSender = interprocess::unnamed_pipe::Sender;

/// The receiving end of an unnamed pipe, passed to [`IPCPipeCollector::from_receiver`].
///
/// Like [`PipeSender`], this is a plain handle that does not need a runtime.
pub type PipeReceiver = interprocess::unnamed_pipe::Recver;

#[cfg(all(feature = "tokio", unix))]
type OwnedPipeHandle = std::os::fd::OwnedFd;
#[cfg(all(feature = "tokio", windows))]
type OwnedPipeHandle = std::os::windows::io::OwnedHandle;

/// Collects metrics sent by an [`IPCPipeRecorder`](crate::IPCPipeRecorder)
/// over an unnamed pipe.
///
/// Use one collector per pipe. Tag each with [`with_label`](Self::with_label)
/// to tell senders apart.
pub struct IPCPipeCollector {
    receiver: PipeReceiver,
    labels: Vec<(String, String)>,
}

impl IPCPipeCollector {
    /// Creates a new unnamed pipe and a collector for its receiving end.
    /// Returns both the collector and the sending handle that should be transferred
    /// to child processes.
    ///
    /// A Tokio runtime is not needed to create the pipe, only to start collecting.
    ///
    /// # Example
    /// ```no_run
    /// let (collector, sender_handle) = metrics_ipc_collector::IPCPipeCollector::new().unwrap();
    /// // Pass sender_handle to child process via inheritance or serialization
    /// collector.start_collecting().unwrap();
    /// ```
    ///
    /// # Errors
    /// Returns an error if pipe creation fails.
    pub fn new() -> Result<(Self, PipeSender), MetricsError> {
        let (sender, receiver) = pipe()?;
        Ok((Self::from_receiver(receiver), sender))
    }

    /// Creates a collector for the receiving end of an existing pipe.
    ///
    /// Use this when something else created the pipe, for example a process
    /// supervisor that hands out raw file descriptors.
    ///
    /// # Example
    /// ```no_run
    /// # #[cfg(unix)]
    /// # fn wrap(fd: std::os::fd::OwnedFd) {
    /// use metrics_ipc_collector::{IPCPipeCollector, PipeReceiver};
    ///
    /// let collector = IPCPipeCollector::from_receiver(PipeReceiver::from(fd)).with_label("worker", "1");
    /// # }
    /// ```
    #[must_use]
    pub const fn from_receiver(receiver: PipeReceiver) -> Self {
        Self {
            receiver,
            labels: Vec::new(),
        }
    }

    /// Adds a label to every metric received on this pipe.
    ///
    /// Collector labels override labels of the same name set by the sender.
    #[must_use]
    pub fn with_label(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        let key = key.into();
        self.labels.retain(|(existing, _)| *existing != key);
        self.labels.push((key, value.into()));
        self
    }

    /// Adds several labels to every metric received on this pipe.
    #[must_use]
    pub fn with_labels<K, V>(self, labels: impl IntoIterator<Item = (K, V)>) -> Self
    where
        K: Into<String>,
        V: Into<String>,
    {
        labels.into_iter().fold(self, |collector, (key, value)| {
            collector.with_label(key, value)
        })
    }

    /// Starts collecting metrics from the unnamed pipe on a background thread,
    /// or a Tokio task when the `tokio` feature is enabled.
    ///
    /// The collector stops when every sender has closed, or when
    /// [`CollectorHandle::stop`] is called.
    ///
    /// # Errors
    /// Returns an error if the collector cannot be spawned. With the `tokio`
    /// feature enabled, this also returns an error when called outside a Tokio
    /// runtime.
    pub fn start_collecting(self) -> Result<CollectorHandle, MetricsError> {
        let labels: ExtraLabels = self.labels.into();

        #[cfg(not(feature = "tokio"))]
        return CollectorHandle::spawn(move |stop| run_collector(self.receiver, &labels, &stop))
            .map_err(Into::into);

        #[cfg(feature = "tokio")]
        {
            let runtime =
                TokioHandle::try_current().map_err(|_| MetricsError::TokioRuntimeRequired)?;
            let receiver = {
                let _guard = runtime.enter();
                interprocess::unnamed_pipe::tokio::Recver::try_from(OwnedPipeHandle::from(
                    self.receiver,
                ))?
            };
            Ok(CollectorHandle::spawn(&runtime, move |stop| {
                run_collector(receiver, labels, stop)
            }))
        }
    }
}

#[cfg(not(feature = "tokio"))]
fn run_collector(receiver: PipeReceiver, labels: &ExtraLabels, stop: &StopSignal) {
    let mut reader = std::io::BufReader::new(receiver);
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        match framing::read_frame(&mut reader, &mut buffer) {
            Ok(_) if stop.is_stopped() => break,
            Ok(true) => handle_frame(&buffer, labels),
            Ok(false) => {
                log::info!("Metrics sender closed, stopping collector");
                break;
            }
            Err(e) => {
                log::error!("Error reading from pipe: {e}");
                break;
            }
        }
    }
}

#[cfg(feature = "tokio")]
async fn run_collector(
    receiver: interprocess::unnamed_pipe::tokio::Recver,
    labels: ExtraLabels,
    mut stop: StopSignal,
) {
    let mut reader = tokio::io::BufReader::new(receiver);
    let mut buffer: Vec<u8> = Vec::new();

    loop {
        tokio::select! {
            biased;
            () = stop.stopped() => break,
            result = framing::read_frame_async(&mut reader, &mut buffer) => match result {
                Ok(true) => handle_frame(&buffer, &labels),
                Ok(false) => {
                    log::info!("Metrics sender closed, stopping collector");
                    break;
                }
                Err(e) => {
                    log::error!("Error reading from pipe: {e}");
                    break;
                }
            },
        }
    }
}
