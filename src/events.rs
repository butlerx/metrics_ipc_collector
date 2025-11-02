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
}

impl TryFrom<&Vec<u8>> for MetricEvent {
    type Error = MetricsError;

    fn try_from(buffer: &Vec<u8>) -> Result<Self, Self::Error> {
        rmp_serde::from_slice(buffer).map_err(MetricsError::from)
    }
}

impl TryFrom<MetricEvent> for Vec<u8> {
    type Error = MetricsError;

    fn try_from(event: MetricEvent) -> Result<Self, Self::Error> {
        rmp_serde::to_vec(&event).map_err(MetricsError::from)
    }
}
