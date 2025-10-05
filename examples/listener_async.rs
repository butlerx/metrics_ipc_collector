//! This example demonstrates how to set up the IPCCollector for gathering metrics and the Prometheus exporter for exposing them.
//!
//! The IPCCollector listens for metrics data from other processes via an IPC socket, while the Prometheus exporter makes these metrics accessible via an HTTP endpoint for scraping by Prometheus or other compatible monitoring systems. The example also gracefully handles termination signals (Ctrl+C) to ensure clean shutdown.
//! The metrics are available on `0.0.0.0:9000` for inspection

#[cfg(not(feature = "tokio"))]
fn main() {
    eprintln!("This example is not available when the Tokio feature is disabled.");
}

#[cfg(feature = "tokio")]
#[tokio::main]
async fn main() {
    // Set up the Prometheus exporter.
    metrics_exporter_prometheus::PrometheusBuilder::new()
        .install()
        .expect("Failed to install Prometheus recorder");

    // Set up the IPCCollector.
    let collector = metrics_ipc_collector::IPCCollector::default();
    if let Err(e) = collector.start_collecting() {
        eprintln!("Failed to start metrics collector: {}", e);
    }

    println!("Metrics listener is running. Press Ctrl+C to exit.");

    // Wait for Ctrl+C signal
    tokio::signal::ctrl_c()
        .await
        .expect("Failed to listen for ctrl-c signal");

    println!("Shutting down metrics listener.");
}
