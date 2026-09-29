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
- **Two transports**: a local socket (`IPCSocketCollector` / `IPCSocketRecorderBuilder`)
  or unnamed pipes (`IPCPipeCollector` / `IPCPipeRecorder`). Pipes can be built
  from handles created elsewhere, for example file descriptors passed to a child
  process by a supervisor.
- **Per-source labels**: `with_label("worker", "3")` on a collector adds the
  label to every metric it receives, overriding the sender's value.
- **Batching**: `flush_interval(...)` on a recorder builder merges updates
  in-process and sends them periodically, instead of one IPC message per update.
- **Shutdown**: `start_collecting()` returns a `CollectorHandle` with `stop()`,
  `join()` and `shutdown()`.
- **Async Support**: Enable the `tokio` feature flag to use async tasks for metric collection. When enabled, all collector operations run on Tokio tasks and require a Tokio runtime. Enable with:
  ```toml
  [dependencies]
  metrics_ipc_collector = { version = "...", features = ["tokio"] }
  ```
  If the `tokio` feature is not enabled, the collector uses threads and blocking IO. Async examples require the feature to be enabled and a Tokio runtime.

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
use metrics_ipc_collector::IPCSocketRecorderBuilder;

fn main() {
    // Create an IPCSocketRecorderBuilder and configure the socket path.
    let builder = IPCSocketRecorderBuilder::default().socket("my_metrics.sock");

    // Attempt to build the IPC recorder and set it as the global recorder.
    if let Err(e) = builder.build() {
        eprintln!("Failed to set up IPC recorder: {}", e);
        return;
    }

    // Record some example metrics.
    counter!("example_counter").increment(1);
    gauge!("example_gauge").set(3.14);
    histogram!("example_histogram").record(42.0);

    println!("Metrics recorded and sent to the IPC socket.");
}
```

### Unnamed Pipe Example

The parent creates one pipe per worker, tags each collector with the worker's
id, and hands the sending end to the worker as a raw handle. See
[`examples/unnamed_pipe.rs`](examples/unnamed_pipe.rs) for a runnable version.

```rust
use metrics_ipc_collector::{IPCPipeCollector, IPCPipeRecorder, PipeSender};
use std::{os::fd::OwnedFd, time::Duration};

// Parent process
let (collector, sender) = IPCPipeCollector::new()?;
let handle = collector.with_label("worker", "1").start_collecting()?;
let fd: OwnedFd = sender.into(); // pass to the child

// Child process
IPCPipeRecorder::builder(PipeSender::from(fd))
    .flush_interval(Duration::from_secs(1)) // batch updates on hot paths
    .build()?;
metrics::counter!("requests").increment(1);

// Parent process, on shutdown
handle.stop();
```

A pipe created elsewhere can be wrapped with
`IPCPipeCollector::from_receiver(PipeReceiver::from(fd))`.

## Delivery and blocking

- **Blocking IO (default):** each update is written before the call returns,
  and blocks if the collector falls behind.
- **`tokio` feature, pipe recorder:** updates go onto a bounded queue
  (`queue_capacity`, default 8192) drained by a task. When the queue is full,
  updates are dropped rather than blocking the caller.
- **Batching (`flush_interval`):** updates only touch in-process state and are
  sent every interval, and again when the recorder is dropped. A global
  recorder is never dropped, so the last interval before exit may be lost.

## Wire format

Each event is a big-endian `u32` length followed by a MessagePack payload.
Collectors and recorders must use the same crate version: 0.5 cannot talk to
0.4, which used newline-delimited frames. Those broke whenever an encoded
value contained a `0x0a` byte.

## License

This project is licensed under the Apache-2.0 License. See the
[LICENSE](LICENSE) file for details.
