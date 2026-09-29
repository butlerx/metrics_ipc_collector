use crate::error::MetricsError;
#[cfg(not(feature = "tokio"))]
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
};
#[cfg(feature = "tokio")]
use tokio::{sync::watch, task::JoinHandle};

/// Signal shared between a [`CollectorHandle`] and the collector it controls.
#[cfg(not(feature = "tokio"))]
#[derive(Debug, Clone, Default)]
pub struct StopSignal(Arc<AtomicBool>);

#[cfg(not(feature = "tokio"))]
impl StopSignal {
    pub fn is_stopped(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    fn stop(&self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Signal shared between a [`CollectorHandle`] and the collector it controls.
#[cfg(feature = "tokio")]
#[derive(Debug, Clone)]
pub struct StopSignal(watch::Receiver<bool>);

#[cfg(feature = "tokio")]
impl StopSignal {
    /// Resolves once a stop has been requested. Never resolves if the handle
    /// was dropped without stopping, since that detaches the collector.
    pub async fn stopped(&mut self) {
        if self.0.wait_for(|stopped| *stopped).await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}

/// A handle to a running collector.
///
/// Dropping the handle leaves the collector running in the background, like
/// dropping a [`std::thread::JoinHandle`].
///
/// # Blocking mode
/// Without the `tokio` feature, reads are blocking. After [`stop`](Self::stop)
/// the listener stops accepting new connections within ~50ms, but a pipe or
/// connection reader only notices the request when its next event arrives or
/// its sender closes. Events received after a stop are discarded.
#[derive(Debug)]
pub struct CollectorHandle {
    #[cfg(not(feature = "tokio"))]
    stop: StopSignal,
    #[cfg(feature = "tokio")]
    stop: watch::Sender<bool>,
    join: JoinHandle<()>,
}

impl CollectorHandle {
    #[cfg(not(feature = "tokio"))]
    pub(crate) fn spawn(f: impl FnOnce(StopSignal) + Send + 'static) -> std::io::Result<Self> {
        let stop = StopSignal::default();
        let signal = stop.clone();
        let join = std::thread::Builder::new()
            .name("metrics-ipc-collector".into())
            .spawn(move || f(signal))?;
        Ok(Self { stop, join })
    }

    #[cfg(feature = "tokio")]
    pub(crate) fn spawn<F>(
        runtime: &tokio::runtime::Handle,
        f: impl FnOnce(StopSignal) -> F,
    ) -> Self
    where
        F: Future<Output = ()> + Send + 'static,
    {
        let (stop, signal) = watch::channel(false);
        let join = runtime.spawn(f(StopSignal(signal)));
        Self { stop, join }
    }

    /// Asks the collector to stop. Returns immediately; use
    /// [`join`](Self::join) to wait for it to finish.
    pub fn stop(&self) {
        #[cfg(not(feature = "tokio"))]
        self.stop.stop();
        #[cfg(feature = "tokio")]
        self.stop.send_replace(true);
    }

    /// Returns `true` once the collector has finished.
    #[must_use]
    pub fn is_finished(&self) -> bool {
        self.join.is_finished()
    }

    /// Waits for the collector to finish.
    ///
    /// The collector finishes on its own when its senders close, or after
    /// [`stop`](Self::stop) is called.
    ///
    /// # Errors
    /// Returns [`MetricsError::CollectorPanicked`] if the collector panicked.
    #[cfg(not(feature = "tokio"))]
    pub fn join(self) -> Result<(), MetricsError> {
        self.join
            .join()
            .map_err(|_| MetricsError::CollectorPanicked)
    }

    /// Waits for the collector to finish.
    ///
    /// The collector finishes on its own when its senders close, or after
    /// [`stop`](Self::stop) is called.
    ///
    /// # Errors
    /// Returns [`MetricsError::CollectorPanicked`] if the collector panicked.
    #[cfg(feature = "tokio")]
    pub async fn join(self) -> Result<(), MetricsError> {
        self.join.await.map_err(|_| MetricsError::CollectorPanicked)
    }

    /// Stops the collector and waits for it to finish.
    ///
    /// # Errors
    /// Returns [`MetricsError::CollectorPanicked`] if the collector panicked.
    #[cfg(not(feature = "tokio"))]
    pub fn shutdown(self) -> Result<(), MetricsError> {
        self.stop();
        self.join()
    }

    /// Stops the collector and waits for it to finish.
    ///
    /// # Errors
    /// Returns [`MetricsError::CollectorPanicked`] if the collector panicked.
    #[cfg(feature = "tokio")]
    pub async fn shutdown(self) -> Result<(), MetricsError> {
        self.stop();
        self.join().await
    }
}
