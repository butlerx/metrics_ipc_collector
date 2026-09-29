use crate::error::MetricsError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// The kind of metric being recorded.
///
/// Used to distinguish between counters, gauges, and histograms.
///
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum MetricKind {
    Counter,
    Gauge,
    Histogram,
}

/// Metadata describing a metric.
///
/// Includes the metric name, kind, description, and optional unit.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricMetadata {
    pub name: String,
    pub kind: MetricKind,
    pub description: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
}

/// Data for a single metric event.
///
/// Contains the metric name, labels, and the operation performed.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricData {
    pub name: String,
    pub labels: BTreeMap<String, String>,
    #[serde(flatten)]
    pub operation: MetricOperation,
}

/// Different operations that can be performed on a metric.
///
/// Includes increment/set for counters and gauges, and record for histograms.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "operation", content = "value")]
#[serde(rename_all = "snake_case")]
pub enum MetricOperation {
    IncrementCounter(u64),
    SetCounter(u64),
    IncrementGauge(f64),
    DecrementGauge(f64),
    SetGauge(f64),
    RecordHistogram(f64),
    /// Several histogram samples flushed together by a batching recorder.
    RecordHistogramBatch(Vec<f64>),
}

/// A named sender and its generation. When several senders claim the same
/// source, only the one with the highest generation may set its gauges.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Source {
    pub name: String,
    pub generation: u64,
}

/// Sent by a recorder at the start of each connection to identify itself.
///
/// The collector adds these labels to every metric on that connection.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Hello {
    pub labels: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

/// An event sent over IPC, representing either metric metadata or metric data.
///
/// Used for communication between processes and the collector.
///
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MetricEvent {
    /// Metadata describing the metric (name, kind, description, unit).
    Metadata(MetricMetadata),
    /// Data for a single metric event (name, labels, operation).
    Metric(MetricData),
    /// Labels identifying the sender, sent once per connection.
    Hello(Hello),
}

impl TryFrom<&[u8]> for MetricEvent {
    type Error = MetricsError;

    fn try_from(buffer: &[u8]) -> Result<Self, Self::Error> {
        rmp_serde::from_slice(buffer).map_err(MetricsError::from)
    }
}

impl TryFrom<&MetricEvent> for Vec<u8> {
    type Error = MetricsError;

    fn try_from(event: &MetricEvent) -> Result<Self, Self::Error> {
        rmp_serde::to_vec(event).map_err(MetricsError::from)
    }
}
