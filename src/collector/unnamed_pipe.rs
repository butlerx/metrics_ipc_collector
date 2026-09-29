use super::{
    handle::{CollectorHandle, StopSignal},
    handlers::{CollectorOptions, StreamState},
};
use crate::{error::MetricsError, events::Source, framing};
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
    internal_metrics: bool,
    source: Option<Source>,
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
            internal_metrics: false,
            source: None,
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

    /// Also records the collector's own metrics, labelled with the collector
    /// labels: `metrics_ipc_collector_events_total`,
    /// `metrics_ipc_collector_decode_errors_total`,
    /// `metrics_ipc_collector_stream_errors_total` and the
    /// `metrics_ipc_collector_connections` gauge. Off by default.
    #[must_use]
    pub const fn internal_metrics(mut self, enabled: bool) -> Self {
        self.internal_metrics = enabled;
        self
    }

    /// Marks this pipe as generation `generation` of the sender `name`, for
    /// when one sender replaces another, such as a respawned worker.
    ///
    /// Collectors in the same process that share a `name` coordinate:
    ///
    /// - Only the newest generation may set gauges. Gauge updates from older
    ///   generations, such as a worker still draining after its replacement
    ///   started, are ignored.
    /// - Counters and histograms from every generation are kept, since that
    ///   work really happened.
    /// - When the newest generation's pipe closes, the gauges it set are
    ///   zeroed, so a crashed sender's values do not linger.
    ///
    /// Use one pipe per sender process. Frames from several processes
    /// writing to one pipe can interleave and cannot be told apart.
    ///
    /// This overrides a source the sender declares with
    /// [`IPCPipeRecorderBuilder::source`](crate::IPCPipeRecorderBuilder::source).
    #[must_use]
    pub fn source(mut self, name: impl Into<String>, generation: u64) -> Self {
        self.source = Some(Source {
            name: name.into(),
            generation,
        });
        self
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
        let mut options = CollectorOptions::new(self.labels, self.internal_metrics);
        options.clear_gauges_on_end = self.source.is_some();
        options.source = self.source;

        #[cfg(not(feature = "tokio"))]
        return CollectorHandle::spawn(move |stop| run_collector(self.receiver, options, &stop))
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
                run_collector(receiver, options, stop)
            }))
        }
    }
}

#[cfg(not(feature = "tokio"))]
fn run_collector(receiver: PipeReceiver, options: CollectorOptions, stop: &StopSignal) {
    let mut reader = std::io::BufReader::new(receiver);
    let mut buffer: Vec<u8> = Vec::new();
    let mut state = StreamState::new(options);

    loop {
        match framing::read_frame(&mut reader, &mut buffer) {
            Ok(_) if stop.is_stopped() => break,
            Ok(true) => state.handle_frame(&buffer),
            Ok(false) => {
                log::info!("Metrics sender closed, stopping collector");
                state.end_of_stream();
                break;
            }
            Err(e) => {
                log::error!("Error reading from pipe: {e}");
                state.stream_error();
                state.end_of_stream();
                break;
            }
        }
    }
}

#[cfg(feature = "tokio")]
async fn run_collector(
    receiver: interprocess::unnamed_pipe::tokio::Recver,
    options: CollectorOptions,
    mut stop: StopSignal,
) {
    let mut reader = tokio::io::BufReader::new(receiver);
    let mut buffer: Vec<u8> = Vec::new();
    let mut state = StreamState::new(options);

    loop {
        tokio::select! {
            biased;
            () = stop.stopped() => break,
            result = framing::read_frame_async(&mut reader, &mut buffer) => match result {
                Ok(true) => state.handle_frame(&buffer),
                Ok(false) => {
                    log::info!("Metrics sender closed, stopping collector");
                    state.end_of_stream();
                    break;
                }
                Err(e) => {
                    log::error!("Error reading from pipe: {e}");
                    state.stream_error();
                    state.end_of_stream();
                    break;
                }
            },
        }
    }
}
