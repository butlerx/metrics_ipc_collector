//! This example demonstrates collecting metrics from a worker over an unnamed pipe.
//!
//! The pipe's sending end is converted to a raw handle, as it would be when
//! handing it to a child process, then turned back into a `PipeSender` by the
//! worker. The collector tags everything it receives with `worker="1"`, and the
//! worker batches its updates, sending them once a second.
//!
//! The metrics are available on `0.0.0.0:9000` for inspection.

use metrics_ipc_collector::{IPCPipeCollector, IPCPipeRecorder, PipeSender};
use std::{os, time::Duration};

#[cfg(windows)]
type PipeHandle = os::windows::io::OwnedHandle;
#[cfg(unix)]
type PipeHandle = os::unix::io::OwnedFd;

fn start_worker(handle: PipeHandle) {
    let sender = PipeSender::from(handle);

    // This example runs the worker in the collector's process, which already
    // has the Prometheus recorder installed globally, so the worker uses a
    // local recorder. A real child process would call `.build()` instead to
    // install the IPC recorder globally.
    let recorder = match IPCPipeRecorder::builder(sender)
        .flush_interval(Duration::from_secs(1))
        .build_recorder()
    {
        Ok(recorder) => recorder,
        Err(e) => {
            eprintln!("Failed to set up IPC recorder: {e}");
            return;
        }
    };

    // Now use metrics normally
    metrics::with_local_recorder(&recorder, || {
        metrics::counter!("requests").increment(1);
        metrics::gauge!("queue_size").set(42.0);
    });
}

#[cfg(not(feature = "tokio"))]
fn main() {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install()
        .expect("Failed to install Prometheus recorder");

    let (collector, sender) = IPCPipeCollector::new().expect("Failed to create pipe");
    let collector = collector
        .with_label("worker", "1")
        .start_collecting()
        .expect("Failed to start metrics collector");

    let handle: PipeHandle = sender.into();
    std::thread::spawn(move || start_worker(handle));

    let (stop_tx, stop_rx) = std::sync::mpsc::channel();
    ctrlc::set_handler(move || {
        let _ = stop_tx.send(());
    })
    .expect("Error setting Ctrl-C handler");

    println!("Metrics listener is running. Press Ctrl+C to exit.");
    let _ = stop_rx.recv();

    println!("Shutting down metrics listener.");
    // In blocking mode the pipe reader exits on its next event or when the
    // worker closes the pipe, so only request the stop here.
    collector.stop();
}

#[cfg(feature = "tokio")]
#[tokio::main]
async fn main() {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install()
        .expect("Failed to install Prometheus recorder");

    let (collector, sender) = IPCPipeCollector::new().expect("Failed to create pipe");
    let collector = collector
        .with_label("worker", "1")
        .start_collecting()
        .expect("Failed to start metrics collector");

    let handle: PipeHandle = sender.into();
    tokio::spawn(async move { start_worker(handle) });

    println!("Metrics listener is running. Press Ctrl+C to exit.");
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl-c signal");

    println!("Shutting down metrics listener.");
    if let Err(e) = collector.shutdown().await {
        eprintln!("Metrics collector failed: {e}");
    }
}
