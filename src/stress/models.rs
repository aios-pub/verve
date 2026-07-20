//! Data models shared between the oha driver, the scenario runner, and the UI.

use serde::{Deserialize, Deserializer, Serialize};

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
#[derive(Debug, Clone, Default)]
pub struct StressSnapshot {
    /// Elapsed time in seconds since start.
    pub elapsed_secs: f64,
    /// Approximate completed iterations (requests / scenario loops).
    pub completed: u64,
    /// Successful requests so far.
    pub success: u64,
    /// Failed requests so far.
    pub failed: u64,
    /// Recent requests-per-second over a short window.
    pub rps: f64,
    /// Latency histogram bins: (bucket upper bound in ms, count).
    pub latency_bins: Vec<(f64, u64)>,
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    /// A human-readable status line.
    pub status: String,
}

/// A single-run result. For the native runner this is aggregated from
/// per-request measurements; for oha-compat input it comes from JSON.
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
    /// Response-time histogram for final chart rendering: (bucket upper ms, count).
    #[serde(default, skip)]
    pub response_time_histogram: Vec<(f64, u64)>,
    /// RPS over time: (elapsed sec, rps) sampled during the run.
    #[serde(default, skip)]
    pub rps_timeline: Vec<(f64, f64)>,
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

// ---- serde helpers for null-safe f64 deserialization ------------------------

/// Deserialize an f64, treating JSON `null` (and absent fields) as `0.0`.
/// oha emits `null` for latency fields when zero requests succeed because
/// NaN/±Infinity serialize to null via serde_json.
fn null_as_zero_f64<'de, D>(deserializer: D) -> Result<f64, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<f64>::deserialize(deserializer)?;
    // Coerce NaN/inf to 0 as well — these are never useful latencies.
    Ok(opt.and_then(|v| if v.is_finite() { Some(v) } else { None }).unwrap_or(0.0))
}

/// Deserialize a `BTreeMap<String, f64>` treating any null values as 0.0.
fn null_map_as_zero<'de, D>(
    deserializer: D,
) -> Result<std::collections::BTreeMap<String, f64>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = std::collections::BTreeMap::<String, Option<f64>>::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .map(|(k, v)| {
            let v = v.and_then(|x| if x.is_finite() { Some(x) } else { None }).unwrap_or(0.0);
            (k, v)
        })
        .collect())
}

// ---- oha JSON schema (subset we consume) -----------------------------------

/// The subset of oha's JSON output we consume. Other fields are ignored.
#[derive(Debug, Clone, Deserialize)]
pub struct OhaJsonReport {
    #[serde(rename = "summary")]
    pub summary: OhaSummary,
    #[serde(rename = "metrics")]
    pub metrics: OhaMetrics,
    #[serde(
        rename = "latencyPercentiles",
        default,
        deserialize_with = "null_map_as_zero"
    )]
    pub latency_percentiles: std::collections::BTreeMap<String, f64>,
    #[serde(rename = "statusCodeDistribution", default)]
    pub status_code_distribution: std::collections::BTreeMap<String, usize>,
    #[serde(rename = "errorDistribution", default)]
    pub error_distribution: std::collections::BTreeMap<String, usize>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OhaSummary {
    #[serde(rename = "successRate", deserialize_with = "null_as_zero_f64")]
    pub success_rate: f64,
    #[serde(deserialize_with = "null_as_zero_f64")]
    pub total: f64,
    #[serde(rename = "requestsPerSec", deserialize_with = "null_as_zero_f64")]
    pub requests_per_sec: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OhaMetrics {
    #[serde(rename = "latency_ms")]
    pub latency_ms: OhaLatency,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OhaLatency {
    #[serde(deserialize_with = "null_as_zero_f64")]
    pub min: f64,
    #[serde(deserialize_with = "null_as_zero_f64")]
    pub mean: f64,
    #[serde(deserialize_with = "null_as_zero_f64")]
    pub p50: f64,
    #[serde(deserialize_with = "null_as_zero_f64")]
    pub p95: f64,
    #[serde(deserialize_with = "null_as_zero_f64")]
    pub p99: f64,
    #[serde(deserialize_with = "null_as_zero_f64")]
    pub max: f64,
}

impl From<OhaJsonReport> for StressResult {
    fn from(r: OhaJsonReport) -> Self {
        let lat = &r.metrics.latency_ms;
        let p = &r.latency_percentiles;
        let coerce = |v: f64| {
            if v.is_finite() && v >= 0.0 {
                v
            } else {
                0.0
            }
        };
        Self {
            total_secs: coerce(r.summary.total),
            total: coerce(r.summary.total) as u64,
            success: (coerce(r.summary.success_rate) * coerce(r.summary.total)) as u64,
            failed: ((1.0 - coerce(r.summary.success_rate)) * coerce(r.summary.total)) as u64,
            rps: coerce(r.summary.requests_per_sec),
            latency_ms: LatencyPercentiles {
                p50: coerce(lat.p50),
                p90: p.get("p90").copied().map(coerce).unwrap_or(coerce(lat.p95)),
                p95: coerce(lat.p95),
                p99: coerce(lat.p99),
                p999: p.get("p99.9").copied().map(coerce).unwrap_or(coerce(lat.p99)),
                min: coerce(lat.min),
                max: coerce(lat.max),
                mean: coerce(lat.mean),
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
            response_time_histogram: Vec::new(),
            rps_timeline: Vec::new(),
        }
    }
}

// ---- Histogram helper ------------------------------------------------------

/// Build a logarithmic-bucket histogram of latency values (in ms), matching
/// oha's general shape: covers 1ms .. ~60s with ~3 significant figures of
/// precision. Returns a Vec of `(upper_bound_ms, count)` sorted ascending.
///
/// Buckets are chosen as 1-2-5 steps on each decade (1,2,5,10,20,50,100,...ms)
/// which gives a good visual distribution on the bar chart without requiring
/// dynamic scaling.
pub fn histogram_bins(latencies_ms: &[u64]) -> Vec<(f64, u64)> {
    use std::collections::BTreeMap;

    if latencies_ms.is_empty() {
        return Vec::new();
    }

    // Upper bounds in ms: 1,2,5,10,20,50,100,200,500,... up to ~60_000ms.
    let mut bounds: Vec<u64> = Vec::new();
    let mut decade: u64 = 1;
    while decade <= 60_000 {
        for m in [1, 2, 5] {
            let b = decade * m;
            if b <= 60_000 {
                bounds.push(b);
            }
        }
        match decade.checked_mul(10) {
            Some(d) => decade = d,
            None => break,
        }
    }
    // Sentinel overflow bucket
    bounds.push(u64::MAX);

    let mut counts: BTreeMap<usize, u64> = BTreeMap::new();
    for &v in latencies_ms {
        let v = v.max(1); // bucket sub-millisecond into 1ms
        let idx = match bounds.binary_search(&v) {
            Ok(i) => i,
            // If v falls between bounds[i-1] and bounds[i], it belongs in bounds[i].
            Err(i) => i,
        };
        // Edge case: v == bounds[idx] exactly belongs in idx (not idx+1).
        *counts.entry(idx).or_insert(0) += 1;
    }

    // Emit non-empty buckets, collapsing the overflow bucket's label to ">60s".
    let mut out = Vec::new();
    for (idx, count) in counts {
        let bound = bounds[idx];
        let label = if bound == u64::MAX {
            60_001.0 // sentinel; UI will render as ">60s"
        } else {
            bound as f64
        };
        out.push((label, count));
    }
    out
}

/// Compute percentiles + min/max/mean from a sorted slice of latencies in ms.
pub fn percentiles_from_sorted(sorted: &[u64]) -> LatencyPercentiles {
    if sorted.is_empty() {
        return LatencyPercentiles::default();
    }
    let p = |pct: f64| -> f64 {
        let idx = ((sorted.len() as f64 - 1.0) * pct).round() as usize;
        sorted[idx.min(sorted.len() - 1)] as f64
    };
    let sum: u64 = sorted.iter().sum();
    LatencyPercentiles {
        p50: p(0.50),
        p90: p(0.90),
        p95: p(0.95),
        p99: p(0.99),
        p999: p(0.999),
        min: sorted[0] as f64,
        max: sorted[sorted.len() - 1] as f64,
        mean: sum as f64 / sorted.len() as f64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_null_fields_to_zero() {
        let json = r#"{
            "summary": {
                "successRate": null,
                "total": 10.001,
                "slowest": null,
                "fastest": null,
                "average": null,
                "requestsPerSec": 89268.43,
                "totalData": 0,
                "sizePerRequest": null,
                "sizePerSec": 0.0
            },
            "metrics": {
                "success_rate": 0.0,
                "requests_per_sec": 89268.43,
                "latency_ms": {
                    "min": null,
                    "mean": null,
                    "p50": null,
                    "p95": null,
                    "p99": null,
                    "max": null
                }
            },
            "latencyPercentiles": {
                "p50": null,
                "p95": null
            },
            "statusCodeDistribution": {},
            "errorDistribution": {}
        }"#;
        let report: OhaJsonReport = serde_json::from_str(json).expect("parse");
        assert_eq!(report.summary.success_rate, 0.0);
        assert_eq!(report.metrics.latency_ms.min, 0.0);
        assert_eq!(report.metrics.latency_ms.p99, 0.0);
        assert_eq!(report.latency_percentiles.get("p50"), Some(&0.0));
    }

    #[test]
    fn histogram_bins_basic() {
        // Ten 5ms and ten 150ms samples.
        let mut lat: Vec<u64> = (0..10).map(|_| 5).collect();
        lat.extend((0..10).map(|_| 150));
        let bins = histogram_bins(&lat);
        assert!(!bins.is_empty());
        let total: u64 = bins.iter().map(|(_, c)| c).sum();
        assert_eq!(total, 20);
    }
}
