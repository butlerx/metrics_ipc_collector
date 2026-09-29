use crate::events::{MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation};
use std::{
    collections::{BTreeMap, HashMap},
    sync::Arc,
};

/// Labels a collector adds to every metric it receives.
pub type ExtraLabels = Arc<[(String, String)]>;

/// Handles cached per stream and metric kind before the cache is reset. Bounds
/// memory when a sender creates an unbounded number of series.
const MAX_CACHED_HANDLES: usize = 10_000;

/// Metric name and sender-provided labels, as decoded from the wire.
type SeriesKey = (String, BTreeMap<String, String>);

/// Options shared by every stream of one collector.
#[derive(Debug, Clone)]
pub struct CollectorOptions {
    pub labels: ExtraLabels,
    pub internal_metrics: bool,
}

impl CollectorOptions {
    pub fn new(labels: Vec<(String, String)>, internal_metrics: bool) -> Self {
        Self {
            labels: labels.into(),
            internal_metrics,
        }
    }
}

/// The collector's own metrics for one stream.
struct InternalMetrics {
    events: metrics::Counter,
    decode_errors: metrics::Counter,
    stream_errors: metrics::Counter,
    connections: metrics::Gauge,
}

impl InternalMetrics {
    fn new(labels: &ExtraLabels) -> Self {
        let labels = labels.to_vec();
        let metrics = Self {
            events: metrics::counter!("metrics_ipc_collector_events_total", &labels),
            decode_errors: metrics::counter!("metrics_ipc_collector_decode_errors_total", &labels),
            stream_errors: metrics::counter!("metrics_ipc_collector_stream_errors_total", &labels),
            connections: metrics::gauge!("metrics_ipc_collector_connections", &labels),
        };
        metrics.connections.increment(1.0);
        metrics
    }
}

impl Drop for InternalMetrics {
    fn drop(&mut self) {
        self.connections.decrement(1.0);
    }
}

/// State for one pipe or socket connection: the sender's hello labels and
/// cached metric handles.
pub struct StreamState {
    options: CollectorOptions,
    hello: BTreeMap<String, String>,
    counters: HashMap<SeriesKey, metrics::Counter>,
    gauges: HashMap<SeriesKey, metrics::Gauge>,
    histograms: HashMap<SeriesKey, metrics::Histogram>,
    internal: Option<InternalMetrics>,
    decode_errors: u64,
}

impl StreamState {
    pub fn new(options: CollectorOptions) -> Self {
        let internal = options
            .internal_metrics
            .then(|| InternalMetrics::new(&options.labels));
        Self {
            options,
            hello: BTreeMap::new(),
            counters: HashMap::new(),
            gauges: HashMap::new(),
            histograms: HashMap::new(),
            internal,
            decode_errors: 0,
        }
    }

    pub fn handle_frame(&mut self, buffer: &[u8]) {
        match MetricEvent::try_from(buffer) {
            Ok(event) => {
                if let Some(internal) = &self.internal {
                    internal.events.increment(1);
                }
                match event {
                    MetricEvent::Metadata(metadata) => handle_metadata_event(metadata),
                    MetricEvent::Metric(metric) => self.handle_metric(metric),
                    MetricEvent::Hello(hello) => self.set_hello(hello.labels),
                }
            }
            Err(e) => {
                self.decode_errors += 1;
                if let Some(internal) = &self.internal {
                    internal.decode_errors.increment(1);
                }
                if self.decode_errors == 1 {
                    log::warn!("Failed to parse metric event, skipping it: {e}");
                } else {
                    log::debug!("Failed to parse metric event, skipping it: {e}");
                }
            }
        }
    }

    /// Records that the stream ended because of a read or wire error.
    pub fn stream_error(&self) {
        if let Some(internal) = &self.internal {
            internal.stream_errors.increment(1);
        }
    }

    fn set_hello(&mut self, labels: BTreeMap<String, String>) {
        self.hello = labels;
        // Cached handles were built with the old labels.
        self.counters.clear();
        self.gauges.clear();
        self.histograms.clear();
    }

    fn handle_metric(&mut self, metric: MetricData) {
        let key = (metric.name, metric.labels);
        let extra = (&self.hello, &self.options.labels);

        match metric.operation {
            MetricOperation::IncrementCounter(value) => {
                cached(&mut self.counters, key, extra, counter).increment(value);
            }
            MetricOperation::SetCounter(value) => {
                cached(&mut self.counters, key, extra, counter).absolute(value);
            }
            MetricOperation::IncrementGauge(value) => {
                cached(&mut self.gauges, key, extra, gauge).increment(value);
            }
            MetricOperation::DecrementGauge(value) => {
                cached(&mut self.gauges, key, extra, gauge).decrement(value);
            }
            MetricOperation::SetGauge(value) => {
                cached(&mut self.gauges, key, extra, gauge).set(value);
            }
            MetricOperation::RecordHistogram(value) => {
                cached(&mut self.histograms, key, extra, histogram).record(value);
            }
            MetricOperation::RecordHistogramBatch(values) => {
                let histogram = cached(&mut self.histograms, key, extra, histogram);
                for value in values {
                    histogram.record(value);
                }
            }
        }
    }
}

fn counter(name: String, labels: &[(String, String)]) -> metrics::Counter {
    metrics::counter!(name, labels)
}

fn gauge(name: String, labels: &[(String, String)]) -> metrics::Gauge {
    metrics::gauge!(name, labels)
}

fn histogram(name: String, labels: &[(String, String)]) -> metrics::Histogram {
    metrics::histogram!(name, labels)
}

/// Returns the cached handle for `key`, registering it on first use.
///
/// Labels are merged only on a cache miss: sender labels, then hello labels,
/// then collector labels, later ones winning.
fn cached<'a, H>(
    cache: &'a mut HashMap<SeriesKey, H>,
    key: SeriesKey,
    (hello, collector): (&BTreeMap<String, String>, &ExtraLabels),
    register: impl FnOnce(String, &[(String, String)]) -> H,
) -> &'a H {
    if cache.len() >= MAX_CACHED_HANDLES && !cache.contains_key(&key) {
        cache.clear();
    }
    cache.entry(key).or_insert_with_key(|(name, labels)| {
        let mut merged = labels.clone();
        merged.extend(hello.iter().map(|(k, v)| (k.clone(), v.clone())));
        merged.extend(collector.iter().cloned());
        register(name.clone(), &merged.into_iter().collect::<Vec<_>>())
    })
}

pub fn handle_metadata_event(metadata: MetricMetadata) {
    let unit = metadata
        .unit
        .clone()
        .and_then(|ref u| metrics::Unit::from_string(u));

    match metadata.kind {
        MetricKind::Counter => {
            if let Some(unit) = unit {
                metrics::describe_counter!(metadata.name, unit, metadata.description);
            } else {
                metrics::describe_counter!(metadata.name, metadata.description);
            }
        }
        MetricKind::Gauge => {
            if let Some(unit) = unit {
                metrics::describe_gauge!(metadata.name, unit, metadata.description);
            } else {
                metrics::describe_gauge!(metadata.name, metadata.description);
            }
        }
        MetricKind::Histogram => {
            if let Some(unit) = unit {
                metrics::describe_histogram!(metadata.name, unit, metadata.description);
            } else {
                metrics::describe_histogram!(metadata.name, metadata.description);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::Hello;
    use metrics_exporter_prometheus::{PrometheusBuilder, PrometheusRecorder};

    fn metric(name: &str, labels: &[(&str, &str)], operation: MetricOperation) -> Vec<u8> {
        let event = MetricEvent::Metric(MetricData {
            name: name.into(),
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            operation,
        });
        (&event).try_into().unwrap()
    }

    fn hello(labels: &[(&str, &str)]) -> Vec<u8> {
        let event = MetricEvent::Hello(Hello {
            labels: labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
        });
        (&event).try_into().unwrap()
    }

    fn render(options: CollectorOptions, frames: &[Vec<u8>]) -> (String, PrometheusRecorder) {
        let recorder = PrometheusBuilder::new().build_recorder();
        metrics::with_local_recorder(&recorder, || {
            let mut state = StreamState::new(options);
            for frame in frames {
                state.handle_frame(frame);
            }
        });
        (recorder.handle().render(), recorder)
    }

    fn options(labels: &[(&str, &str)]) -> CollectorOptions {
        CollectorOptions::new(
            labels
                .iter()
                .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
                .collect(),
            false,
        )
    }

    #[test]
    fn label_precedence_is_sender_then_hello_then_collector() {
        let (rendered, _) = render(
            options(&[("worker", "2")]),
            &[
                hello(&[("worker", "9"), ("host", "a"), ("route", "hello")]),
                metric(
                    "requests",
                    &[("worker", "7"), ("route", "/"), ("host", "b")],
                    MetricOperation::IncrementCounter(4),
                ),
            ],
        );
        assert!(
            rendered.contains(r#"requests{host="a",route="hello",worker="2"} 4"#),
            "{rendered}"
        );
    }

    #[test]
    fn cached_handles_accumulate() {
        let frames: Vec<_> = (0..5)
            .map(|_| {
                metric(
                    "hits",
                    &[("route", "/")],
                    MetricOperation::IncrementCounter(2),
                )
            })
            .collect();
        let (rendered, _) = render(options(&[]), &frames);
        assert!(rendered.contains(r#"hits{route="/"} 10"#), "{rendered}");
    }

    #[test]
    fn a_new_hello_relabels_later_metrics() {
        let (rendered, _) = render(
            options(&[]),
            &[
                hello(&[("worker", "1")]),
                metric("jobs", &[], MetricOperation::IncrementCounter(1)),
                hello(&[("worker", "2")]),
                metric("jobs", &[], MetricOperation::IncrementCounter(1)),
            ],
        );
        assert!(rendered.contains(r#"jobs{worker="1"} 1"#), "{rendered}");
        assert!(rendered.contains(r#"jobs{worker="2"} 1"#), "{rendered}");
    }

    #[test]
    fn histogram_batches_record_every_sample() {
        let (rendered, _) = render(
            options(&[]),
            &[metric(
                "latency",
                &[],
                MetricOperation::RecordHistogramBatch(vec![1.0, 2.0, 3.0]),
            )],
        );
        assert!(rendered.contains("latency_count 3"), "{rendered}");
        assert!(rendered.contains("latency_sum 6"), "{rendered}");
    }

    #[test]
    fn internal_metrics_count_events_errors_and_connections() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let options = CollectorOptions::new(vec![("worker".into(), "1".into())], true);
        let rendered_open = metrics::with_local_recorder(&recorder, || {
            let mut state = StreamState::new(options);
            state.handle_frame(&metric("x", &[], MetricOperation::IncrementCounter(1)));
            state.handle_frame(b"not msgpack");
            state.stream_error();
            let rendered = recorder.handle().render();
            drop(state);
            rendered
        });
        let rendered_closed = recorder.handle().render();

        for line in [
            r#"metrics_ipc_collector_events_total{worker="1"} 1"#,
            r#"metrics_ipc_collector_decode_errors_total{worker="1"} 1"#,
            r#"metrics_ipc_collector_stream_errors_total{worker="1"} 1"#,
            r#"metrics_ipc_collector_connections{worker="1"} 1"#,
        ] {
            assert!(
                rendered_open.contains(line),
                "{line} missing from:\n{rendered_open}"
            );
        }
        assert!(
            rendered_closed.contains(r#"metrics_ipc_collector_connections{worker="1"} 0"#),
            "{rendered_closed}"
        );
    }

    #[test]
    fn handle_cache_is_bounded() {
        let recorder = PrometheusBuilder::new().build_recorder();
        metrics::with_local_recorder(&recorder, || {
            let mut state = StreamState::new(options(&[]));
            for i in 0..=MAX_CACHED_HANDLES {
                let id = i.to_string();
                state.handle_frame(&metric(
                    "series",
                    &[("id", &id)],
                    MetricOperation::IncrementCounter(1),
                ));
            }
            assert!(state.counters.len() <= MAX_CACHED_HANDLES);
        });
    }
}
