//! End-to-end tests: metrics sent through a real pipe or socket and rendered by
//! the Prometheus exporter on the collector side.

use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusHandle};
use metrics_ipc_collector::{
    IPCPipeCollector, IPCPipeRecorder, IPCSocketCollector, IPCSocketRecorderBuilder, PipeReceiver,
    PipeSender,
};
use std::{
    sync::OnceLock,
    time::{Duration, Instant},
};

const TIMEOUT: Duration = Duration::from_secs(5);

/// The collector replays events through the global recorder, so every test in
/// this binary shares one Prometheus recorder and uses distinct metric names.
fn prometheus() -> &'static PrometheusHandle {
    static HANDLE: OnceLock<PrometheusHandle> = OnceLock::new();
    HANDLE.get_or_init(|| {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        metrics::set_global_recorder(recorder).expect("global recorder should install once");
        handle
    })
}

fn wait_for(expected: &[&str]) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let rendered = prometheus().render();
        if expected.iter().all(|line| rendered.contains(line)) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected:?} in:\n{rendered}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn record_immediate(sender: PipeSender, f: impl FnOnce()) {
    let recorder = IPCPipeRecorder::builder(sender)
        .build_recorder()
        .expect("recorder should build");
    metrics::with_local_recorder(&recorder, f);
}

fn record_values_that_contain_newlines() {
    // 10 encodes as 0x0a, which newline-delimited framing split in two.
    metrics::counter!("e2e_newline_total").absolute(10);
    metrics::gauge!("e2e_newline_gauge", "route" => "/").set(10.0);
    metrics::counter!("e2e_newline_total").increment(5);
}

fn record_batched_updates() {
    for _ in 0..1000 {
        metrics::counter!("e2e_batched_total").increment(1);
        metrics::gauge!("e2e_batched_inflight").increment(1.0);
    }
    for _ in 0..400 {
        metrics::gauge!("e2e_batched_inflight").decrement(1.0);
    }
    for sample in [1.0, 2.0, 3.0] {
        metrics::histogram!("e2e_batched_latency").record(sample);
    }
}

const NEWLINE_LINES: &[&str] = &[
    r#"e2e_newline_total{worker="1"} 15"#,
    r#"e2e_newline_gauge{route="/",worker="1"} 10"#,
];

const BATCHED_LINES: &[&str] = &[
    r#"e2e_batched_total{host="h",worker="2"} 1000"#,
    r#"e2e_batched_inflight{host="h",worker="2"} 600"#,
    r#"e2e_batched_latency_count{host="h",worker="2"} 3"#,
    r#"e2e_batched_latency_sum{host="h",worker="2"} 6"#,
];

#[cfg(unix)]
type PipeHandle = std::os::fd::OwnedFd;
#[cfg(windows)]
type PipeHandle = std::os::windows::io::OwnedHandle;

#[cfg(not(feature = "tokio"))]
mod blocking {
    use super::*;

    #[test]
    fn pipe_delivers_values_containing_newlines() {
        prometheus();
        let (collector, sender) = IPCPipeCollector::new().unwrap();
        let handle = collector
            .with_label("worker", "1")
            .start_collecting()
            .unwrap();

        record_immediate(sender, record_values_that_contain_newlines);
        wait_for(NEWLINE_LINES);
        // Dropping the recorder closed the pipe, which ends the collector.
        handle.join().unwrap();
    }

    #[test]
    fn batched_pipe_from_raw_handles() {
        prometheus();
        let (sender, receiver) = interprocess::unnamed_pipe::pipe().unwrap();
        // Round-trip through raw handles, as a process supervisor would.
        let receiver = PipeReceiver::from(PipeHandle::from(receiver));
        let sender = PipeSender::from(PipeHandle::from(sender));

        let collector = IPCPipeCollector::from_receiver(receiver)
            .with_label("worker", "2")
            .start_collecting()
            .unwrap();
        let recorder = IPCPipeRecorder::builder(sender)
            .flush_interval(Duration::from_millis(50))
            .with_label("host", "h")
            .build_recorder()
            .unwrap();
        metrics::with_local_recorder(&recorder, record_batched_updates);
        // Dropping the recorder flushes the batch and closes the pipe.
        drop(recorder);

        wait_for(BATCHED_LINES);
        collector.join().unwrap();
    }

    #[test]
    fn pipe_collector_finishes_when_sender_closes() {
        let (collector, sender) = IPCPipeCollector::new().unwrap();
        let handle = collector.start_collecting().unwrap();
        drop(sender);
        handle.join().expect("collector should exit cleanly on EOF");
    }

    #[test]
    fn socket_collector_stops_accepting_after_stop() {
        prometheus();
        let handle = IPCSocketCollector::default()
            .socket("e2e_blocking.sock")
            .with_label("worker", "socket")
            .start_collecting()
            .unwrap();

        let recorder = connect_with_retry("e2e_blocking.sock");
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("e2e_socket_blocking_total").absolute(10);
        });
        wait_for(&[r#"e2e_socket_blocking_total{worker="socket"} 10"#]);

        let started = Instant::now();
        handle.shutdown().expect("listener should stop cleanly");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    fn connect_with_retry(socket: &str) -> metrics_ipc_collector::IPCSocketRecorder {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match IPCSocketRecorderBuilder::default()
                .socket(socket)
                .build_recorder()
            {
                Ok(recorder) => return recorder,
                Err(_) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => panic!("could not connect to collector: {e}"),
            }
        }
    }
}

#[cfg(feature = "tokio")]
mod async_io {
    use super::*;

    #[tokio::test(flavor = "multi_thread")]
    async fn pipe_delivers_values_containing_newlines() {
        prometheus();
        // Creating the pipe does not need a runtime; starting it does.
        let (collector, sender) = std::thread::spawn(IPCPipeCollector::new)
            .join()
            .unwrap()
            .unwrap();
        let handle = collector
            .with_label("worker", "1")
            .start_collecting()
            .unwrap();

        record_immediate(sender, record_values_that_contain_newlines);
        tokio::task::spawn_blocking(|| wait_for(NEWLINE_LINES))
            .await
            .unwrap();

        // The sender is still open, so only a stop request ends the collector.
        tokio::time::timeout(TIMEOUT, handle.shutdown())
            .await
            .expect("stop should end a collector with an open sender")
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn batched_pipe_from_raw_handles() {
        prometheus();
        let (sender, receiver) = interprocess::unnamed_pipe::pipe().unwrap();
        let receiver = PipeReceiver::from(PipeHandle::from(receiver));
        let sender = PipeSender::from(PipeHandle::from(sender));

        let collector = IPCPipeCollector::from_receiver(receiver)
            .with_label("worker", "2")
            .start_collecting()
            .unwrap();
        let recorder = IPCPipeRecorder::builder(sender)
            .flush_interval(Duration::from_millis(50))
            .with_label("host", "h")
            .build_recorder()
            .unwrap();
        metrics::with_local_recorder(&recorder, record_batched_updates);

        tokio::task::spawn_blocking(|| wait_for(BATCHED_LINES))
            .await
            .unwrap();
        collector.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn pipe_collector_finishes_when_sender_closes() {
        let (collector, sender) = IPCPipeCollector::new().unwrap();
        let handle = collector.start_collecting().unwrap();
        drop(sender);
        tokio::time::timeout(TIMEOUT, handle.join())
            .await
            .expect("collector should exit on EOF")
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn dropping_the_handle_detaches_the_collector() {
        prometheus();
        let (collector, sender) = IPCPipeCollector::new().unwrap();
        drop(collector.start_collecting().unwrap());
        // Still running: the pipe must have a reader, so writes succeed.
        let recorder = IPCPipeRecorder::builder(sender).build_recorder().unwrap();
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("e2e_detached_total", "worker" => "d").increment(10);
        });
        tokio::task::spawn_blocking(|| wait_for(&[r#"e2e_detached_total{worker="d"} 10"#]))
            .await
            .unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn socket_collector_shuts_down_with_open_connections() {
        prometheus();
        let handle = IPCSocketCollector::default()
            .socket("e2e_tokio.sock")
            .with_label("worker", "socket")
            .start_collecting()
            .unwrap();

        let recorder = connect_with_retry("e2e_tokio.sock").await;
        metrics::with_local_recorder(&recorder, || {
            metrics::counter!("e2e_socket_tokio_total").absolute(10);
        });
        tokio::task::spawn_blocking(|| {
            wait_for(&[r#"e2e_socket_tokio_total{worker="socket"} 10"#]);
        })
        .await
        .unwrap();

        tokio::time::timeout(TIMEOUT, handle.shutdown())
            .await
            .expect("stop should end open connections")
            .unwrap();
    }

    async fn connect_with_retry(socket: &str) -> metrics_ipc_collector::IPCSocketRecorder {
        let deadline = Instant::now() + TIMEOUT;
        loop {
            match IPCSocketRecorderBuilder::default()
                .socket(socket)
                .build_recorder()
            {
                Ok(recorder) => return recorder,
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                Err(e) => panic!("could not connect to collector: {e}"),
            }
        }
    }
}

/// Socket address handling, run in both blocking and Tokio modes.
#[cfg(unix)]
mod socket_addresses {
    use super::*;
    use std::path::PathBuf;

    /// Runs `f` with a Tokio runtime entered when the `tokio` feature is on.
    fn with_runtime<T>(f: impl FnOnce() -> T) -> T {
        #[cfg(feature = "tokio")]
        {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            let _guard = runtime.enter();
            f()
        }
        #[cfg(not(feature = "tokio"))]
        f()
    }

    fn temp_socket(name: &str) -> PathBuf {
        let mode = if cfg!(feature = "tokio") {
            "tokio"
        } else {
            "blocking"
        };
        let path =
            std::env::temp_dir().join(format!("mipc-{}-{mode}-{name}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        path
    }

    #[test]
    fn path_sockets_round_trip() {
        prometheus();
        let path = temp_socket("round-trip");
        with_runtime(|| {
            let _handle = IPCSocketCollector::default()
                .path(&path)
                .start_collecting()
                .unwrap();
            let recorder = IPCSocketRecorderBuilder::default()
                .path(&path)
                .build_recorder()
                .unwrap();
            metrics::with_local_recorder(&recorder, || {
                metrics::counter!("e2e_path_total").absolute(10);
            });
            wait_for(&["e2e_path_total 10"]);
            // A second event after the reader has drained the socket catches
            // readers that were left in non-blocking mode.
            metrics::with_local_recorder(&recorder, || {
                metrics::counter!("e2e_path_total").increment(5);
            });
            wait_for(&["e2e_path_total 15"]);
        });
    }

    #[test]
    fn stale_socket_files_are_replaced() {
        let path = temp_socket("stale");
        // A std listener leaves its socket file behind when dropped, like a
        // collector that crashed.
        drop(std::os::unix::net::UnixListener::bind(&path).unwrap());
        assert!(path.exists());

        with_runtime(|| {
            IPCSocketCollector::default()
                .path(&path)
                .start_collecting()
                .expect("a stale socket file should be replaced");
        });
    }

    #[test]
    fn live_collectors_are_not_taken_over() {
        let path = temp_socket("live");
        with_runtime(|| {
            let _first = IPCSocketCollector::default()
                .path(&path)
                .start_collecting()
                .unwrap();
            let err = IPCSocketCollector::default()
                .path(&path)
                .start_collecting()
                .expect_err("a second collector should not take over a live socket");
            assert!(err.to_string().contains("already listening"), "{err}");
        });
    }

    #[test]
    fn regular_files_are_not_deleted() {
        let path = temp_socket("regular-file");
        std::fs::write(&path, b"not a socket").unwrap();

        let result = with_runtime(|| IPCSocketCollector::default().path(&path).start_collecting());
        let still_there = std::fs::read(&path).ok();
        let _ = std::fs::remove_file(&path);
        assert_eq!(still_there.as_deref(), Some(&b"not a socket"[..]));
        assert!(result.is_err());
    }

    #[test]
    fn recorder_labels_are_sent_once_and_applied() {
        prometheus();
        let path = temp_socket("hello");
        with_runtime(|| {
            let _handle = IPCSocketCollector::default()
                .path(&path)
                .with_label("role", "collector")
                .start_collecting()
                .unwrap();
            let recorder = IPCSocketRecorderBuilder::default()
                .path(&path)
                .with_label("worker", "5")
                .with_label("role", "spoofed")
                .build_recorder()
                .unwrap();
            metrics::with_local_recorder(&recorder, || {
                metrics::counter!("e2e_hello_total").increment(3);
            });
            wait_for(&[r#"e2e_hello_total{role="collector",worker="5"} 3"#]);
        });
    }

    #[test]
    fn recorder_reconnects_after_collector_restart() {
        prometheus();
        let path = temp_socket("reconnect");
        with_runtime(|| {
            let first = IPCSocketCollector::default()
                .path(&path)
                .with_label("generation", "1")
                .start_collecting()
                .unwrap();
            let recorder = IPCSocketRecorderBuilder::default()
                .path(&path)
                .internal_metrics(true)
                .build_recorder()
                .unwrap();
            let bump = || {
                metrics::with_local_recorder(&recorder, || {
                    metrics::counter!("e2e_reconnect_total").increment(1);
                });
            };
            bump();
            wait_for(&[r#"e2e_reconnect_total{generation="1"} 1"#]);

            #[cfg(feature = "tokio")]
            tokio::runtime::Handle::current()
                .block_on(first.shutdown())
                .unwrap();
            #[cfg(not(feature = "tokio"))]
            first.shutdown().unwrap();

            let _second = IPCSocketCollector::default()
                .path(&path)
                .with_label("generation", "2")
                .start_collecting()
                .unwrap();

            // The old connection was shut down when the first collector
            // stopped, so the very first update reconnects and is delivered.
            bump();
            wait_for(&[r#"e2e_reconnect_total{generation="2"} 1"#]);

            // Internal counters ride along with the next successful write.
            bump();
            let reconnects = format!(
                r#"metrics_ipc_recorder_reconnects_total{{generation="2",pid="{}"}} 1"#,
                std::process::id()
            );
            wait_for(&[&reconnects, r#"e2e_reconnect_total{generation="2"} 2"#]);
        });
    }
}
