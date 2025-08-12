//! This example demonstrates how to set up the IPCCollector for gathering metrics and the Prometheus exporter for exposing them.
//!
//! The IPCCollector listens for metrics data from other processes via an IPC socket, while the Prometheus exporter makes these metrics accessible via an HTTP endpoint for scraping by Prometheus or other compatible monitoring systems. The example also gracefully handles termination signals (Ctrl+C) to ensure clean shutdown.
//!  the metric are available on `0.0.0.0:9000` for inspection

use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_ipc_collector::IPCCollector;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

fn main() {
    // Set up the Prometheus exporter.
    PrometheusBuilder::new()
        .install()
        .expect("Failed to install Prometheus recorder");

    // Set up the IPCCollector.
    let collector = IPCCollector::default();
    if let Err(e) = collector.start_collecting() {
        eprintln!("Failed to start metrics collector: {}", e);
    }

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
