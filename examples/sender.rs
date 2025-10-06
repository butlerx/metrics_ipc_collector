//! This example demonstrates how to set up the IPCRecorderBuilder for sending metrics to an IPC socket.
//!
//! The IPCRecorderBuilder establishes a connection to a specified IPC socket and configures it as the global metrics recorder. This allows the application to record metrics such as counters, gauges, and histograms, which are then sent to the IPC socket for collection by a compatible listener.
//!
//! The example includes recording a counter, a gauge, and a histogram metric to illustrate the usage of the IPCRecorderBuilder.

fn main() {
    // Create an IPCRecorderBuilder and configure the socket path.
    let builder = metrics_ipc_collector::IPCSocketRecorderBuilder::default();

    // Attempt to build the IPC recorder and set it as the global recorder.
    if let Err(e) = builder.build() {
        eprintln!("Failed to set up IPC recorder: {}", e);
        return;
    }

    // Record some example metrics.
    metrics::counter!("example_counter").increment(1);
    metrics::gauge!("example_gauge").set(std::f32::consts::PI);
    metrics::histogram!("example_histogram").record(42.0);

    println!("Metrics recorded and sent to the IPC socket.");
}
