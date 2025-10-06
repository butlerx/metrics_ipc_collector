use crate::events::{MetricData, MetricKind, MetricMetadata, MetricOperation};

pub fn handle_metric_event(metric: MetricData) {
    match metric.operation {
        MetricOperation::IncrementCounter(value) => {
            if metric.labels.is_empty() {
                metrics::counter!(metric.name).increment(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::counter!(metric.name, &labels).increment(value);
            }
        }
        MetricOperation::SetCounter(value) => {
            if metric.labels.is_empty() {
                metrics::counter!(metric.name).absolute(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::counter!(metric.name, &labels).absolute(value);
            }
        }
        MetricOperation::IncrementGauge(value) => {
            if metric.labels.is_empty() {
                metrics::gauge!(metric.name).increment(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::gauge!(metric.name, &labels).increment(value);
            }
        }
        MetricOperation::DecrementGauge(value) => {
            if metric.labels.is_empty() {
                metrics::gauge!(metric.name).decrement(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::gauge!(metric.name, &labels).decrement(value);
            }
        }
        MetricOperation::SetGauge(value) => {
            if metric.labels.is_empty() {
                metrics::gauge!(metric.name).set(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
                metrics::gauge!(metric.name, &labels).set(value);
            }
        }
        MetricOperation::RecordHistogram(value) => {
            if metric.labels.is_empty() {
                metrics::histogram!(metric.name).record(value);
            } else {
                let labels: Vec<_> = metric
                    .labels
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();
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
