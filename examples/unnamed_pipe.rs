use metrics_ipc_collector::{IPCPipeCollector, IPCPipeRecorder, PipeSender};
use std::{os, sync::mpsc};
#[cfg(not(feature = "tokio"))]
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

#[cfg(windows)]
type PipeHandle = os::windows::io::OwnedHandle;
#[cfg(unix)]
type PipeHandle = os::unix::io::OwnedFd;

#[cfg(not(feature = "tokio"))]
fn main() {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install()
        .expect("Failed to install Prometheus recorder");

    let (collector, sender) = IPCPipeCollector::new().expect("Failed to create IPC collector");
    let (handle_tx, handle_rx) = mpsc::sync_channel(1);

    if let Err(e) = collector.start_collecting() {
        eprintln!("Failed to start metrics collector: {}", e);
    }

    let sender_handle: PipeHandle = sender.into();
    handle_tx.send(sender_handle).unwrap();

    thread::spawn(move || {
        let handle = handle_rx.recv().unwrap();
        let sender = PipeSender::from(handle);

        if let Err(e) = IPCPipeRecorder::build(sender) {
            eprintln!("Failed to set up IPC recorder: {}", e);
            return;
        }

        // Now use metrics normally
        metrics::counter!("requests").increment(1);
        metrics::gauge!("queue_size").set(42.0);
    });

    let running = Arc::new(AtomicBool::new(true));
    let r = running.clone();

    // Handle Ctrl+C to exit gracefully.
    ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    })
    .expect("Error setting Ctrl-C handler");

    println!("Metrics listener is running. Press Ctrl+C to exit.");

    // Keep the program running to expose metrics.
    while running.load(Ordering::SeqCst) {
        thread::sleep(Duration::from_secs(1));
    }

    println!("Shutting down metrics listener.");
}

#[cfg(feature = "tokio")]
#[tokio::main]
async fn main() {
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install()
        .expect("Failed to install Prometheus recorder");

    let (collector, sender) = IPCPipeCollector::new().expect("Failed to create IPC collector");
    let (handle_tx, handle_rx) = mpsc::sync_channel(1);

    if let Err(e) = collector.start_collecting() {
        eprintln!("Failed to start metrics collector: {}", e);
    }

    let sender_handle: PipeHandle = sender.try_into().unwrap();
    handle_tx.send(sender_handle).unwrap();

    tokio::task::spawn(async move {
        let handle = handle_rx.recv().unwrap();
        let sender = PipeSender::try_from(handle).unwrap();

        if let Err(e) = IPCPipeRecorder::build(sender) {
            eprintln!("Failed to set up IPC recorder: {}", e);
            return;
        }

        // Now use metrics normally
        metrics::counter!("requests").increment(1);
        metrics::gauge!("queue_size").set(42.0);
    });

    println!("Metrics listener is running. Press Ctrl+C to exit.");

    // Wait for Ctrl+C signal
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl-c signal");

    println!("Shutting down metrics listener.");
}
