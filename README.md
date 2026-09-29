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
  label to every metric it receives. The same method on a recorder builder
  labels everything that sender records; the labels are sent once per
  connection, not with every update.
- **Reconnect**: socket recorders built with `IPCSocketRecorderBuilder`
  reconnect when the collector restarts.
- **Internal metrics**: `internal_metrics(true)` on collectors and recorders
  reports dropped events, decode errors, reconnects and open connections.
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

The listener sets up the `IPCSocketCollector` to gather metrics from IPC sockets and
uses the Prometheus exporter to expose them via an HTTP endpoint. The example
also handles termination signals (e.g., Ctrl+C) for clean shutdown.

```rust
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_ipc_collector::IPCSocketCollector;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

fn main() {
    // Set up the Prometheus exporter.
    PrometheusBuilder::new()
        .install()
        .expect("Failed to install Prometheus recorder");

    // Set up the IPCSocketCollector.
    let collector = IPCSocketCollector::default();
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
and histograms, and send them to an IPC socket using `IPCSocketRecorderBuilder`.

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
- **Socket reconnect:** when a write fails, a builder-made socket recorder
  reconnects straight away and resends that write. If the collector is still
  unreachable, it retries with backoff from 100ms up to 5s and drops updates
  in the meantime. `IPCSocketRecorder::new(stream)` does not know the
  address, so it cannot reconnect.

## Labels

A metric's labels are merged in this order, later ones winning:

1. labels on the metric itself, e.g. `counter!("hits", "route" => "/")`
2. recorder labels from `IPCPipeRecorderBuilder::with_label` or
   `IPCSocketRecorderBuilder::with_label`
3. collector labels from `IPCPipeCollector::with_label` or
   `IPCSocketCollector::with_label`

Recorder labels come from the sender and can be anything it chooses. Use
collector labels when the value must not be spoofable, such as a worker id
assigned by the parent process.

## Internal metrics

With `internal_metrics(true)`, the collector records, labelled with its
collector labels:

| metric | kind | meaning |
|--------|------|---------|
| `metrics_ipc_collector_connections` | gauge | open pipes or socket connections |
| `metrics_ipc_collector_events_total` | counter | events received |
| `metrics_ipc_collector_decode_errors_total` | counter | frames that could not be decoded and were skipped |
| `metrics_ipc_collector_stream_errors_total` | counter | streams dropped because of a read or wire-format error |

Recorders with `internal_metrics(true)` send, labelled with their recorder
labels and a `pid` label so recorders never share a series:

| metric | kind | meaning |
|--------|------|---------|
| `metrics_ipc_recorder_dropped_events_total` | counter | updates that could not be delivered |
| `metrics_ipc_recorder_reconnects_total` | counter | socket reconnections |

A recorder reports its counters with its next successful write, so drops
during an outage show up once the collector is reachable again.

## Senders that exit

The collector cannot tell the exporter to forget a series, so metrics from a
sender that exits keep their last value indefinitely. With the Prometheus
exporter, expire series that stop updating:

```rust
use metrics_exporter_prometheus::PrometheusBuilder;
use metrics_util::MetricKindMask; // add `metrics-util` to your dependencies
use std::time::Duration;

PrometheusBuilder::new()
    .idle_timeout(MetricKindMask::ALL, Some(Duration::from_secs(300)))
    .install()?;
```

The timeout also removes series from live senders that simply have not
changed. In batching mode, an unchanged gauge or counter is not resent, so
choose a timeout well above the time between updates. Alternatively, have
senders set their gauges periodically, or restrict the mask, for example to
`MetricKindMask::GAUGE`.

When a sender restarts, counters it sets with `absolute()` start again from
zero. Prometheus treats that as a counter reset, and `rate()` handles it.

## Socket addresses

Collectors and recorders must use the same address:

- `.socket(name)` (default `metrics_collector.sock`) is a namespaced name:
  - Linux and Android: the abstract socket namespace, with no file.
  - Other Unix: a socket file in `/run/user/<uid>` if it exists, otherwise
    in `/tmp`.
  - Windows: the named pipe `\\.\pipe\<name>`.
- `.path(path)` is a socket file at an explicit path. On Windows it must be a
  named pipe path.

On startup, the collector refuses to start if another collector is already
answering on the address. It replaces stale socket files left by a crash. For
`.path()` addresses it only ever removes socket files: anything else at the
path is reported as an error.

## Wire format

Each event is framed as:

| bytes | meaning                                           |
|-------|---------------------------------------------------|
| 0     | `0xC1`, a byte MessagePack never produces         |
| 1     | wire version, currently `1`                       |
| 2..6  | big-endian `u32` payload length                   |
| 6..   | MessagePack-encoded event                         |

A collector drops a stream whose header does not match, with an error
logged. Senders from 0.4 and earlier, which used newline-delimited frames,
are reported as "the sender may be using an older version". Upgrade
collectors and senders together.

## Security

Everything a sender writes is trusted. Any process that can reach the
collector can create, change or overwrite metrics, and can grow memory use by
creating many distinct metrics or label values. Collector labels set with
`with_label` override the sender's value for that label only.

Who can reach the collector depends on the transport:

- **Unnamed pipes (`IPCPipeCollector`)**: only processes holding the pipe's
  sending handle. This is the safest option when you spawn the senders
  yourself.
- **Linux namespaced sockets (`.socket(name)`)**: the abstract namespace has
  no permission checks. Every process in the same network namespace can
  connect, and any process can claim the name first if the collector is not
  yet running.
- **Other Unix namespaced sockets**: a socket file in `/run/user/<uid>`, which
  is private to the user, or in the shared `/tmp`, where access follows the
  socket file's permissions (set by the process umask) and other users can
  claim the name first.
- **`.path(path)` sockets**: normal file permissions. Put the socket in a
  directory only the intended users can access, for example a `0700`
  directory owned by the service user.
- **Windows named pipes**: the pipe is created with the default named pipe
  security descriptor. Check that it matches your requirements before
  relying on it across user accounts.

Frames larger than 16 MiB are rejected, so a single malformed or hostile
frame cannot force a large allocation.

## License

This project is licensed under the Apache-2.0 License. See the
[LICENSE](LICENSE) file for details.
