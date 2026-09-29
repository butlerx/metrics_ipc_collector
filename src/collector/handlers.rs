use crate::events::{MetricData, MetricEvent, MetricKind, MetricMetadata, MetricOperation};
use std::sync::Arc;

/// Labels a collector adds to every metric it receives.
pub type ExtraLabels = Arc<[(String, String)]>;

pub fn handle_frame(buffer: &[u8], extra_labels: &ExtraLabels) {
    match MetricEvent::try_from(buffer) {
        Ok(MetricEvent::Metadata(metadata)) => handle_metadata_event(metadata),
        Ok(MetricEvent::Metric(metric)) => handle_metric_event(metric, extra_labels),
        Err(e) => log::trace!("Failed to parse metric event: {e}"),
    }
}

pub fn handle_metric_event(mut metric: MetricData, extra_labels: &ExtraLabels) {
    // Collector labels win over sender labels, so a sender cannot claim to be
    // a different source.
    for (key, value) in extra_labels.iter() {
        metric.labels.insert(key.clone(), value.clone());
    }
    let labels = metric.labels.into_iter().collect::<Vec<_>>();

    match metric.operation {
        MetricOperation::IncrementCounter(value) => {
            metrics::counter!(metric.name, &labels).increment(value);
        }
        MetricOperation::SetCounter(value) => {
            metrics::counter!(metric.name, &labels).absolute(value);
        }
        MetricOperation::IncrementGauge(value) => {
            metrics::gauge!(metric.name, &labels).increment(value);
        }
        MetricOperation::DecrementGauge(value) => {
            metrics::gauge!(metric.name, &labels).decrement(value);
        }
        MetricOperation::SetGauge(value) => {
            metrics::gauge!(metric.name, &labels).set(value);
        }
        MetricOperation::RecordHistogram(value) => {
            metrics::histogram!(metric.name, &labels).record(value);
        }
        MetricOperation::RecordHistogramBatch(values) => {
            let histogram = metrics::histogram!(metric.name, &labels);
            for value in values {
                histogram.record(value);
            }
        }
    }
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
    use metrics_exporter_prometheus::PrometheusBuilder;
    use std::collections::BTreeMap;

    #[test]
    fn collector_labels_override_sender_labels() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let extra: ExtraLabels = vec![("worker".to_string(), "2".to_string())].into();

        metrics::with_local_recorder(&recorder, || {
            handle_metric_event(
                MetricData {
                    name: "requests".into(),
                    labels: BTreeMap::from([
                        ("worker".to_string(), "9".to_string()),
                        ("route".to_string(), "/".to_string()),
                    ]),
                    operation: MetricOperation::IncrementCounter(4),
                },
                &extra,
            );
        });

        let rendered = handle.render();
        assert!(
            rendered.contains(r#"requests{route="/",worker="2"} 4"#),
            "{rendered}"
        );
    }

    #[test]
    fn histogram_batches_record_every_sample() {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();

        metrics::with_local_recorder(&recorder, || {
            handle_metric_event(
                MetricData {
                    name: "latency".into(),
                    labels: BTreeMap::new(),
                    operation: MetricOperation::RecordHistogramBatch(vec![1.0, 2.0, 3.0]),
                },
                &ExtraLabels::from([]),
            );
        });

        let rendered = handle.render();
        assert!(rendered.contains("latency_count 3"), "{rendered}");
        assert!(rendered.contains("latency_sum 6"), "{rendered}");
    }
}
