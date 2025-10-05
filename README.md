# Metrics IPC Collector

A metrics collector that uses interprocess communication (IPC) to collect
metrics from multiple processes.

## Description

`metrics_ipc_collector` is a Rust library designed for gathering metrics from
multiple processes using IPC, and exposing those metrics from a single exporter
for observability. It provides an easy-to-use interface for both sending and
collecting metrics, making it suitable for multi-process applications.

## Features

- Supports all formats provided by the metrics crate.
- Supports multiple platforms
- Async Support. When the `tokio` feature flag is set the listener will
  automatically use a tokio task rather then a spawn. Note: this needs to be
  started in a tokio process

## Installation

Add the following to your `Cargo.toml`:

```toml
[dependencies]
metrics_ipc_collector = "0.1.0"
```

## Examples

### Listener Example

The listener sets up the `IPCCollector` to gather metrics from IPC sockets and
uses the Prometheus exporter to expose them via an HTTP endpoint. The example
also handles termination signals (e.g., Ctrl+C) for clean shutdown.

```rust
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_ipc_collector::IPCCollector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

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
```

### Sender Example

The sender example demonstrates how to record metrics such as counters, gauges,
and histograms, and send them to an IPC socket using `IPCRecorderBuilder`.

```rust
use metrics::{counter, gauge, histogram};
use metrics_ipc_collector::IPCRecorderBuilder;

fn main() {
    // Create an IPCRecorderBuilder and configure the socket path.
    let builder = IPCRecorderBuilder::default().socket("my_metrics.sock");

    // Attempt to build the IPC recorder and set it as the global recorder.
    if let Err(e) = builder.build() {
        eprintln!("Failed to set up IPC recorder: {}", e);
        return;
    }

    // Record some example metrics.
    counter!("example_counter", 1, &[]);
    gauge!("example_gauge", 3.14, &[]);
    histogram!("example_histogram", 42.0, &[]);

    println!("Metrics recorded and sent to the IPC socket.");
}
```

## License

This project is licensed under the Apache-2.0 License. See the
[LICENSE](LICENSE) file for details.
