//! Scenario (multi-step TestCase) stress runner.
//!
//! oha only supports single URLs, so for multi-step autotest cases we spawn N
//! workers each running the autotest runner in a loop, recording per-iteration
//! latency and pass/fail, and aggregating percentiles via a sorted latency
//! buffer at the end. This is less precise than oha's histogram but sufficient
//! for the expected concurrency range (≤ a few hundred RPS).

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use http_client::HttpClient;
use tokio::sync::{mpsc, oneshot};

use crate::autotest::runner::run_case;
use crate::autotest::TestCase;
use crate::state::models::Project;

use super::models::{LatencyPercentiles, StressResult, StressSnapshot};

/// Handle returned by stress runners; call [`stop()`](StressStopHandle::stop)
/// to request graceful shutdown, or drop it to abort.
pub struct StressStopHandle {
    pub(crate) stop: Arc<AtomicBool>,
    pub(crate) result_rx: Option<oneshot::Receiver<StressResult>>,
    pub(crate) snapshot_rx: Option<mpsc::UnboundedReceiver<StressSnapshot>>,
}

impl StressStopHandle {
    /// Request the run stop; returns immediately.
    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
    /// Await the aggregated result.
    pub async fn result(mut self) -> StressResult {
        match self.result_rx.take() {
            Some(rx) => rx.await.unwrap_or_default(),
            None => StressResult::default(),
        }
    }
    /// Poll the next progress snapshot (none if the channel is empty/closed).
    pub fn poll_snapshot(&mut self) -> Option<StressSnapshot> {
        self.snapshot_rx.as_mut().and_then(|r| r.try_recv().ok())
    }
}

/// Run an N-worker scenario stress for `duration_secs`. Returns immediately with
/// a handle; the run proceeds on spawned Tokio tasks. `http_client` is shared
/// across workers (reqwest/hyper handles connection pooling internally).
pub fn run_scenario(
    project: Project,
    case: TestCase,
    http_client: Arc<dyn HttpClient>,
    concurrency: u32,
    duration_secs: u32,
    think_ms: u64,
) -> StressStopHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let (snap_tx, snap_rx) = mpsc::unbounded_channel::<StressSnapshot>();
    let (result_tx, result_rx) = oneshot::channel::<StressResult>();
    let completed = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let latencies: Arc<tokio::sync::Mutex<Vec<u64>>> = Arc::new(tokio::sync::Mutex::new(Vec::new()));

    let start = Instant::now();
    let mut workers = Vec::with_capacity(concurrency as usize);
    for _ in 0..concurrency {
        let stop = stop.clone();
        let completed = completed.clone();
        let failed = failed.clone();
        let latencies = latencies.clone();
        let case = case.clone();
        let project = project.clone();
        let client = http_client.clone();
        let think = think_ms;
        workers.push(tokio::spawn(async move {
            while !stop.load(Ordering::SeqCst) {
                if start.elapsed().as_secs() >= duration_secs as u64 {
                    break;
                }
                let (tx, _rx) = mpsc::unbounded_channel();
                let t0 = Instant::now();
                match run_case(&case, &project, &*client, tx, None).await {
                    Ok(res) => {
                        let total_ms = res.total_ms;
                        latencies.lock().await.push(total_ms);
                        if res.passed {
                            completed.fetch_add(1, Ordering::SeqCst);
                        } else {
                            failed.fetch_add(1, Ordering::SeqCst);
                        }
                    }
                    Err(_) => {
                        failed.fetch_add(1, Ordering::SeqCst);
                    }
                }
                if think > 0 {
                    tokio::time::sleep(Duration::from_millis(think)).await;
                }
            }
        }));
    }

    // Snapshot pumper.
    let stop_snap = stop.clone();
    let completed_snap = completed.clone();
    let failed_snap = failed.clone();
    let snap_task = tokio::spawn(async move {
        while !stop_snap.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let done = completed_snap.load(Ordering::SeqCst);
            let _ = snap_tx.send(StressSnapshot {
                elapsed_secs: start.elapsed().as_secs_f64(),
                duration_secs: duration_secs as f64,
                completed: done,
                success: done,
                failed: failed_snap.load(Ordering::SeqCst),
                rps: 0.0,
                window_rps: 0.0,
                timescale_secs: 1.0,
                latency_bins: Vec::new(),
                p50_ms: 0.0,
                p95_ms: 0.0,
                p99_ms: 0.0,
                window_min_ms: 0.0,
                window_max_ms: 0.0,
                window_mean_ms: 0.0,
                rps_bins: Vec::new(),
                total_bytes: 0,
                status: format!("完成 {} 次", done),
            });
            if start.elapsed().as_secs() >= duration_secs as u64 {
                break;
            }
        }
    });

    // Aggregator task.
    tokio::spawn(async move {
        for w in workers {
            let _ = w.await;
        }
        let _ = snap_task.await;
        let total = completed.load(Ordering::SeqCst) + failed.load(Ordering::SeqCst);
        let elapsed = start.elapsed().as_secs_f64().max(f64::MIN_POSITIVE);
        let mut lat = latencies.lock().await.clone();
        lat.sort();
        let result = StressResult {
            total_secs: elapsed,
            total,
            success: completed.load(Ordering::SeqCst),
            failed: failed.load(Ordering::SeqCst),
            rps: total as f64 / elapsed,
            latency_ms: percentiles_from_sorted(&lat),
            status_distribution: BTreeMap::new(),
            errors: BTreeMap::new(),
            response_time_histogram: Vec::new(),
            rps_timeline: Vec::new(),
            rps_bins: Vec::new(),
            rps_bins_timescale: 1.0,
            total_bytes: 0,
        };
        let _ = result_tx.send(result);
    });

    StressStopHandle {
        stop,
        result_rx: Some(result_rx),
        snapshot_rx: Some(snap_rx),
    }
}

fn percentiles_from_sorted(sorted: &[u64]) -> LatencyPercentiles {
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
    fn percentiles_basic() {
        // 1..=100 sorted
        let data: Vec<u64> = (1..=100).collect();
        let p = percentiles_from_sorted(&data);
        assert_eq!(p.min, 1.0);
        assert_eq!(p.max, 100.0);
        // 0-indexed nearest-rank on 100 elements (idx=round(99*pct)).
        assert!(p.p50 >= 49.0 && p.p50 <= 51.0, "p50={}", p.p50);
        assert!(p.p95 >= 94.0 && p.p95 <= 96.0, "p95={}", p.p95);
        assert!(p.p99 >= 98.0 && p.p99 <= 100.0, "p99={}", p.p99);
        assert!((p.mean - 50.5).abs() < 0.1);
    }

    #[test]
    fn percentiles_empty() {
        let p = percentiles_from_sorted(&[]);
        assert_eq!(p.p50 as u64, 0);
    }
}
