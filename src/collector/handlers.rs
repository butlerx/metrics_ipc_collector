use crate::events::{MetricData, MetricKind, MetricMetadata, MetricOperation};

pub fn handle_metric_event(metric: MetricData) {
    let labels = metric.labels.into_iter().collect::<Vec<_>>();

    match metric.operation {
        MetricOperation::IncrementCounter(value) => {
            if labels.is_empty() {
                metrics::counter!(metric.name).increment(value);
            } else {
                metrics::counter!(metric.name, &labels).increment(value);
            }
        }
        MetricOperation::SetCounter(value) => {
            if labels.is_empty() {
                metrics::counter!(metric.name).absolute(value);
            } else {
                metrics::counter!(metric.name, &labels).absolute(value);
            }
        }
        MetricOperation::IncrementGauge(value) => {
            if labels.is_empty() {
                metrics::gauge!(metric.name).increment(value);
            } else {
                metrics::gauge!(metric.name, &labels).increment(value);
            }
        }
        MetricOperation::DecrementGauge(value) => {
            if labels.is_empty() {
                metrics::gauge!(metric.name).decrement(value);
            } else {
                metrics::gauge!(metric.name, &labels).decrement(value);
            }
        }
        MetricOperation::SetGauge(value) => {
            if labels.is_empty() {
                metrics::gauge!(metric.name).set(value);
            } else {
                metrics::gauge!(metric.name, &labels).set(value);
            }
        }
        MetricOperation::RecordHistogram(value) => {
            if labels.is_empty() {
                metrics::histogram!(metric.name).record(value);
            } else {
                metrics::histogram!(metric.name, &labels).record(value);
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
