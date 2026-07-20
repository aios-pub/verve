//! Driver for the built-in HTTP load generator.
//!
//! We previously shelled out to [`oha`](https://github.com/hatoo/oha) as a
//! library (writing JSON to a temp file and parsing it). oha's internal
//! worker/monitor modules are private so we couldn't stream per-request
//! results to the UI for live chart rendering. We now run a small native
//! load generator on top of `reqwest` (already a project dependency) which:
//!
//! - keeps `concurrency` workers looping issuing the same prepared request
//!   for `duration_secs`,
//! - streams per-request latency/status/error events to an aggregator,
//! - emits `StressSnapshot` every ~200ms for real-time UI rendering,
//! - returns a `StressResult` with full latency percentiles + a histogram.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use reqwest::ClientBuilder;
use tokio::sync::{mpsc, oneshot};

use crate::http;
use crate::state::models::{ApiRequest, Project};

use super::models::{
    histogram_bins, percentiles_from_sorted, LatencyPercentiles, StressResult, StressSnapshot,
};
use super::scenario::StressStopHandle;

/// No-op kept for API compatibility (oha is no longer downloaded; the runner
/// is built-in).
pub fn find_or_download() -> Result<(), String> {
    Ok(())
}

/// A single per-request event flowing from a worker to the aggregator.
struct RequestEvent {
    latency_ms: u64,
    status: Option<u16>,
    error: Option<String>,
}

/// Resolve the effective variable map for a request in the project (global <
/// env; folder/request variables are applied per-request by the UI).
pub fn effective_vars(project: &Project) -> BTreeMap<String, String> {
    use crate::state::models::effective_variables;
    effective_variables(
        &project.global_variables,
        project.active_env_variables(),
        &[],
        &[],
    )
}

/// Kick off a load test against a single resolved request.
///
/// Returns immediately: workers and the aggregator run on a freshly spawned
/// Tokio runtime bound to a background thread. Poll the returned handle for
/// live snapshots and call `.join()` (or let it drop) to get the final result.
pub fn run_oha(
    _oha_path: &(),
    req: &ApiRequest,
    vars: &BTreeMap<String, String>,
    concurrency: u32,
    duration_secs: u32,
    qps: Option<u32>,
) -> Result<StressStopHandle, String> {
    let prepared = http::client::prepare(
        req.method,
        &req.url,
        &req.params,
        &req.headers,
        &req.path,
        &req.cookies,
        &req.auth,
        &req.body,
        vars,
        60,
    )
    .map_err(|e| format!("prepare: {e}"))?;

    // Build a reqwest client with oha-like defaults: no TLS cert verification,
    // 10s connect timeout, no auto-retries, IPv4 preferred when possible.
    let client = ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_secs(10))
        .pool_max_idle_per_host(concurrency.max(32) as usize)
        .connection_verbose(false)
        .build()
        .map_err(|e| format!("build client: {e}"))?;

    // Build the request template.
    let method = match prepared.method {
        crate::state::models::RequestMethod::Get => reqwest::Method::GET,
        crate::state::models::RequestMethod::Post => reqwest::Method::POST,
        crate::state::models::RequestMethod::Put => reqwest::Method::PUT,
        crate::state::models::RequestMethod::Delete => reqwest::Method::DELETE,
        crate::state::models::RequestMethod::Patch => reqwest::Method::PATCH,
        crate::state::models::RequestMethod::Head => reqwest::Method::HEAD,
        crate::state::models::RequestMethod::Options => reqwest::Method::OPTIONS,
    };
    let mut builder = client.request(method, &prepared.url);
    for (k, v) in &prepared.headers {
        if k.eq_ignore_ascii_case("content-length") {
            continue; // reqwest sets this itself
        }
        builder = builder.header(k, v);
    }
    let body_bytes = prepared.body.clone();
    if !body_bytes.is_empty() {
        builder = builder.body(body_bytes);
    }
    let request = builder.build().map_err(|e| format!("build request: {e}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let (ev_tx, ev_rx) = mpsc::unbounded_channel::<RequestEvent>();
    let (snap_tx, snap_rx) = mpsc::unbounded_channel::<StressSnapshot>();
    let (result_tx, result_rx) = oneshot::channel::<StressResult>();

    let workers_stop = stop.clone();
    let conc = concurrency.max(1) as u64;
    let duration = Duration::from_secs(duration_secs.max(1) as u64);
    let qps_per_worker = qps.map(|q| q as f64 / conc as f64);

    std::thread::Builder::new()
        .name("oha-runner".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(concurrency.max(2).min(16) as usize)
                .build()
                .unwrap();

            rt.block_on(async move {
                // Spawn workers.
                let mut worker_handles = Vec::with_capacity(conc as usize);
                for _ in 0..conc {
                    let client = client.clone();
                    let req = request.try_clone().expect("request body must be clonable");
                    let ev_tx = ev_tx.clone();
                    let stop = workers_stop.clone();
                    worker_handles.push(tokio::spawn(async move {
                        run_worker(client, req, ev_tx, stop, duration, qps_per_worker).await
                    }));
                }
                // Drop our sender so the aggregator ends when workers exit.
                drop(ev_tx);

                // Run aggregator on this task.
                let result = run_aggregator(ev_rx, snap_tx).await;

                // Wait for all workers to finish (they should have exited by now).
                for h in worker_handles {
                    let _ = h.await;
                }
                let _ = result_tx.send(result);
            });
        })
        .map_err(|e| format!("spawn runner: {e}"))?;

    Ok(StressStopHandle {
        stop,
        result_rx: Some(result_rx),
        snapshot_rx: Some(snap_rx),
    })
}

/// One worker loop: issue the request as fast as possible (or pacing if a QPS
/// cap is given) until duration elapses or stop is signaled.
async fn run_worker(
    client: reqwest::Client,
    request: reqwest::Request,
    ev_tx: mpsc::UnboundedSender<RequestEvent>,
    stop: Arc<AtomicBool>,
    duration: Duration,
    qps_per_worker: Option<f64>,
) {
    let start = Instant::now();
    let min_gap = qps_per_worker.filter(|q| *q > 0.0).map(|q| Duration::from_secs_f64(1.0 / q));

    loop {
        if stop.load(Ordering::SeqCst) || start.elapsed() >= duration {
            break;
        }

        let req_start = Instant::now();
        let event = match client.execute(request.try_clone().unwrap()).await {
            Ok(resp) => {
                let status = Some(resp.status().as_u16());
                // Drain body so timing reflects full transfer.
                let _ = resp.bytes().await;
                RequestEvent {
                    latency_ms: req_start.elapsed().as_millis() as u64,
                    status,
                    error: None,
                }
            }
            Err(e) => RequestEvent {
                latency_ms: req_start.elapsed().as_millis() as u64,
                status: None,
                error: Some(short_err(&e)),
            },
        };
        let _ = ev_tx.send(event);

        if let Some(gap) = min_gap {
            let elapsed = req_start.elapsed();
            if elapsed < gap {
                tokio::time::sleep(gap - elapsed).await;
            }
        }
    }
}

/// Aggregate per-request events into a running state, emit snapshots every
/// 200ms, and produce the final `StressResult` once all workers finish.
async fn run_aggregator(
    mut ev_rx: mpsc::UnboundedReceiver<RequestEvent>,
    snap_tx: mpsc::UnboundedSender<StressSnapshot>,
) -> StressResult {
    let start = Instant::now();
    let mut latencies: Vec<u64> = Vec::with_capacity(4096);
    let mut recent_latencies: Vec<(Instant, u64)> = Vec::with_capacity(2048);
    let mut total: u64 = 0;
    let mut success: u64 = 0;
    let mut failed: u64 = 0;
    let mut status_dist: BTreeMap<u16, u64> = BTreeMap::new();
    let mut errors: BTreeMap<String, u64> = BTreeMap::new();
    let mut rps_timeline: Vec<(f64, f64)> = Vec::new();

    // We poll events and emit snapshots on a fixed tick.
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut last_snapshot_count: u64 = 0;
    let mut last_snapshot_instant = start;
    let mut done = false;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                if done {
                    break;
                }
                // Emit a snapshot.
                let elapsed = start.elapsed();
                // Drop samples older than 1s for the recent RPS window.
                let cutoff = Instant::now() - Duration::from_secs(1);
                recent_latencies.retain(|(t, _)| *t >= cutoff);
                let recent_count = recent_latencies.len() as u64;
                let rps = recent_count as f64;

                // For chart rendering, build a histogram from all latencies so far.
                let bins = histogram_bins(&latencies);
                let sorted = {
                    let mut s = latencies.clone();
                    s.sort_unstable();
                    s
                };
                let pct = percentiles_from_sorted(&sorted);

                let snapshot = StressSnapshot {
                    elapsed_secs: elapsed.as_secs_f64(),
                    completed: total,
                    success,
                    failed,
                    rps,
                    latency_bins: bins,
                    p50_ms: pct.p50,
                    p95_ms: pct.p95,
                    p99_ms: pct.p99,
                    status: format!("完成 {} 次", total),
                };
                let _ = snap_tx.send(snapshot);

                // Track RPS timeline using the delta between ticks.
                let dt = last_snapshot_instant.elapsed().as_secs_f64().max(0.001);
                let delta = total - last_snapshot_count;
                let instantaneous_rps = delta as f64 / dt;
                rps_timeline.push((elapsed.as_secs_f64(), instantaneous_rps));
                last_snapshot_count = total;
                last_snapshot_instant = Instant::now();

                // Stop condition: if elapsed is well past duration and there are
                // no in-flight events left to drain (checked below when None),
                // but the channel is our only completion signal.
            }
            ev = ev_rx.recv() => {
                match ev {
                    Some(e) => {
                        total += 1;
                        latencies.push(e.latency_ms);
                        recent_latencies.push((Instant::now(), e.latency_ms));
                        match e.status {
                            Some(code) => {
                                if (200..400).contains(&code) {
                                    success += 1;
                                } else {
                                    failed += 1;
                                }
                                *status_dist.entry(code).or_insert(0) += 1;
                            }
                            None => {
                                failed += 1;
                                if let Some(msg) = e.error {
                                    *errors.entry(msg).or_insert(0) += 1;
                                }
                            }
                        }
                    }
                    None => {
                        // All workers have exited. Drain one last snapshot then finalize.
                        done = true;
                    }
                }
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64().max(f64::MIN_POSITIVE);
    latencies.sort_unstable();
    let pct = percentiles_from_sorted(&latencies);
    let bins = histogram_bins(&latencies);

    StressResult {
        total_secs: elapsed,
        total,
        success,
        failed,
        rps: total as f64 / elapsed,
        latency_ms: LatencyPercentiles {
            p50: pct.p50,
            p90: pct.p90,
            p95: pct.p95,
            p99: pct.p99,
            p999: pct.p999,
            min: pct.min,
            max: pct.max,
            mean: pct.mean,
        },
        status_distribution: status_dist,
        errors,
        response_time_histogram: bins,
        rps_timeline,
    }
}

fn short_err(e: &reqwest::Error) -> String {
    // Produce a short, stable error label keyed off the error category so the
    // distribution doesn't explode with one-off messages.
    if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() {
        "connect error".into()
    } else if e.is_body() {
        "body error".into()
    } else if e.is_decode() {
        "decode error".into()
    } else if e.is_request() {
        "request error".into()
    } else {
        // Avoid per-URL messages (DNS failures often include the host).
        "other error".into()
    }
}
