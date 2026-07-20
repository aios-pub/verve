//! Data models shared between the oha driver, the scenario runner, and the UI.

use serde::{Deserialize, Serialize};

/// What to stress: a single API request (oha path) or an existing autotest
/// TestCase (scenario path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum StressTarget {
    /// Target a single request by id (must exist in the active project).
    Request(String),
    /// Target an existing TestCase by id (multi-step scenario).
    TestCase(String),
}

/// User-configured stress parameters.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StressConfig {
    pub target: StressTarget,
    /// Concurrent workers (oha `-c`).
    pub concurrency: u32,
    /// Duration in seconds (oha `-z`).
    pub duration_secs: u32,
    /// Optional QPS cap (oha `-q`). None = unlimited.
    pub qps: Option<u32>,
    /// Optional think time in ms between scenario iterations (scenario runner only).
    #[serde(default)]
    pub think_ms: u64,
}

impl Default for StressConfig {
    fn default() -> Self {
        Self {
            target: StressTarget::Request(String::new()),
            concurrency: 50,
            duration_secs: 10,
            qps: None,
            think_ms: 0,
        }
    }
}

/// One live-progress snapshot streamed to the UI while a run is in progress.
#[derive(Debug, Clone)]
pub struct StressSnapshot {
    /// Elapsed time in seconds since start.
    pub elapsed_secs: f64,
    /// Approximate completed iterations (requests / scenario loops).
    pub completed: u64,
    /// A human-readable status line.
    pub status: String,
}

/// A single-run result. For oha this comes from JSON; for the scenario runner
/// it's aggregated from per-step measurements.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StressResult {
    /// Total wall-clock time (seconds).
    pub total_secs: f64,
    /// Total requests / iterations issued.
    pub total: u64,
    /// Total successful (2xx/3xx or scenario passed).
    pub success: u64,
    /// Total failed (error or non-2xx or scenario failed).
    pub failed: u64,
    /// Requests per second (mean).
    pub rps: f64,
    /// Latency percentiles in milliseconds (p50, p90, p95, p99, p999).
    pub latency_ms: LatencyPercentiles,
    /// Per-status-code histogram (status code → count).
    pub status_distribution: std::collections::BTreeMap<u16, u64>,
    /// Per-error histogram (error string → count).
    pub errors: std::collections::BTreeMap<String, u64>,
}

/// Latency percentile breakdown; each value is in milliseconds.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LatencyPercentiles {
    pub p50: f64,
    pub p90: f64,
    pub p95: f64,
    pub p99: f64,
    pub p999: f64,
    pub min: f64,
    pub max: f64,
    pub mean: f64,
}

/// The subset of oha's JSON output we consume. Other fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct OhaJsonReport {
    #[serde(rename = "summary")]
    pub summary: OhaSummary,
    #[serde(rename = "metrics")]
    pub metrics: OhaMetrics,
    #[serde(rename = "latencyPercentiles")]
    pub latency_percentiles: std::collections::BTreeMap<String, f64>,
    #[serde(rename = "statusCodeDistribution")]
    pub status_code_distribution: std::collections::BTreeMap<String, usize>,
    #[serde(rename = "errorDistribution")]
    pub error_distribution: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OhaSummary {
    #[serde(rename = "successRate")]
    pub success_rate: f64,
    pub total: f64,
    #[serde(rename = "requestsPerSec")]
    pub requests_per_sec: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OhaMetrics {
    #[serde(rename = "latency_ms")]
    pub latency_ms: OhaLatency,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OhaLatency {
    pub min: f64,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub p99: f64,
    pub max: f64,
}

impl From<OhaJsonReport> for StressResult {
    fn from(r: OhaJsonReport) -> Self {
        let lat = &r.metrics.latency_ms;
        let p = &r.latency_percentiles;
        Self {
            total_secs: r.summary.total,
            total: r.summary.total as u64,
            success: (r.summary.success_rate * r.summary.total) as u64,
            failed: ((1.0 - r.summary.success_rate) * r.summary.total) as u64,
            rps: r.summary.requests_per_sec,
            latency_ms: LatencyPercentiles {
                p50: lat.p50,
                p90: p.get("p90").copied().unwrap_or(lat.p95),
                p95: lat.p95,
                p99: lat.p99,
                p999: p.get("p99.9").copied().unwrap_or(lat.p99),
                min: lat.min,
                max: lat.max,
                mean: lat.mean,
            },
            status_distribution: r
                .status_code_distribution
                .iter()
                .filter_map(|(k, v)| k.parse::<u16>().ok().map(|code| (code, *v as u64)))
                .collect(),
            errors: r
                .error_distribution
                .iter()
                .map(|(k, v)| (k.clone(), *v as u64))
                .collect(),
        }
    }
}
