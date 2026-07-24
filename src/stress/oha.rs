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

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use reqwest::ClientBuilder;
use reqwest::redirect::Policy;
use tokio::sync::{mpsc, oneshot};

use crate::http;
use crate::state::models::{ApiRequest, Project};

use super::models::{
    LatencyPercentiles, StressResult, StressSnapshot, histogram_bins, percentiles_from_sorted,
};
use super::scenario::StressStopHandle;

/// Tuning knobs for the load generator, mapped from oha-style CLI flags.
pub struct OhaOptions {
    pub concurrency: u32,
    pub duration_secs: u32,
    pub qps: Option<u32>,
    pub timeout_secs: u32,
    pub disable_keepalive: bool,
    pub follow_redirects: bool,
    pub http2: bool,
}

/// No-op kept for API compatibility (oha is no longer downloaded; the runner
/// is built-in).
pub fn find_or_download() -> Result<(), String> {
    Ok(())
}

/// A single per-request event flowing from a worker to the aggregator.
struct RequestEvent {
    /// When the request finished (Instant::now() at completion time).
    end: Instant,
    latency_ms: u64,
    status: Option<u16>,
    error: Option<String>,
    bytes: u64,
}

/// One completed request kept in the aggregator's bounded queue for
/// sliding-window stats.
#[derive(Clone, Copy)]
struct Finished {
    end: Instant,
    latency_ms: u64,
    success: bool,
    bytes: u64,
}

/// Resolve the effective variable map for a request in the project (global <
/// env; folder/request variables are applied per-request by the UI).
pub fn effective_vars(project: &Project) -> BTreeMap<String, String> {
    use crate::state::models::effective_variables;
    let mut system = BTreeMap::new();
    let remote_cfg = crate::state::persistence::load_remote_config();
    system.insert(
        "mock_server".to_string(),
        crate::share::remote::mock_server_url(&remote_cfg),
    );
    effective_variables(
        &system,
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
    opts: OhaOptions,
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

    let timeout_secs = opts.timeout_secs.max(1);
    let concurrency = opts.concurrency;
    let duration_secs = opts.duration_secs;
    let qps = opts.qps;

    // Build a reqwest client with oha-like defaults: no TLS cert verification,
    // 10s connect timeout, a configurable per-request timeout, no auto-retries.
    let mut builder = ClientBuilder::new()
        .danger_accept_invalid_certs(true)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(timeout_secs as u64))
        .redirect(if opts.follow_redirects {
            Policy::limited(10)
        } else {
            Policy::none()
        })
        .connection_verbose(false);
    if opts.disable_keepalive {
        // Setting idle-per-host pool to 0 prevents connection reuse.
        builder = builder.pool_max_idle_per_host(0);
    } else {
        builder = builder.pool_max_idle_per_host(concurrency.max(32) as usize);
    }
    if opts.http2 {
        builder = builder.http2_prior_knowledge();
    }
    let client = builder.build().map_err(|e| format!("build client: {e}"))?;

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
    let mut req_builder = client.request(method, &prepared.url);
    for (k, v) in &prepared.headers {
        if k.eq_ignore_ascii_case("content-length") {
            continue; // reqwest sets this itself
        }
        req_builder = req_builder.header(k, v);
    }
    let body_bytes = prepared.body.clone();
    if !body_bytes.is_empty() {
        req_builder = req_builder.body(body_bytes);
    }
    let request = req_builder
        .build()
        .map_err(|e| format!("build request: {e}"))?;

    let stop = Arc::new(AtomicBool::new(false));
    let (ev_tx, ev_rx) = mpsc::unbounded_channel::<RequestEvent>();
    let (snap_tx, snap_rx) = mpsc::unbounded_channel::<StressSnapshot>();
    let (result_tx, result_rx) = oneshot::channel::<StressResult>();

    let workers_stop = stop.clone();
    let conc = concurrency.max(1) as u64;
    let duration = Duration::from_secs(duration_secs.max(1) as u64);
    let qps_per_worker = qps.map(|q| q as f64 / conc as f64);
    let worker_timeout_secs = timeout_secs;

    std::thread::Builder::new()
        .name("oha-runner".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(concurrency.clamp(2, 16) as usize)
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
                    let to_secs = worker_timeout_secs;
                    worker_handles.push(tokio::spawn(async move {
                        run_worker(client, req, ev_tx, stop, duration, qps_per_worker, to_secs)
                            .await
                    }));
                }
                // Drop our sender so the aggregator ends when workers exit.
                drop(ev_tx);

                // Run aggregator on this task.
                let result = run_aggregator(ev_rx, snap_tx, duration).await;

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
    timeout_secs: u32,
) {
    let start = Instant::now();
    let min_gap = qps_per_worker
        .filter(|q| *q > 0.0)
        .map(|q| Duration::from_secs_f64(1.0 / q));

    loop {
        if stop.load(Ordering::SeqCst) || start.elapsed() >= duration {
            break;
        }

        // Hard per-iteration deadline: never let a single request+body drag on
        // for more than 2s past the configured run duration, so the worker
        // always terminates promptly even when the server is slow/hanging.
        let remaining = duration
            .checked_sub(start.elapsed())
            .unwrap_or(Duration::ZERO)
            .max(Duration::from_millis(50));
        let req_deadline =
            Duration::from_secs(timeout_secs as u64).min(remaining + Duration::from_secs(2));

        let req_start = Instant::now();
        let event = match tokio::time::timeout(req_deadline, execute_one(&client, &request)).await {
            Ok(inner) => inner,
            Err(_) => RequestEvent {
                end: Instant::now(),
                latency_ms: req_start.elapsed().as_millis() as u64,
                status: None,
                error: Some("deadline exceeded".into()),
                bytes: 0,
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

/// Issue a single HTTP request and drain the body, returning a RequestEvent.
async fn execute_one(client: &reqwest::Client, request: &reqwest::Request) -> RequestEvent {
    let req_start = Instant::now();
    match client.execute(request.try_clone().unwrap()).await {
        Ok(resp) => {
            let status = Some(resp.status().as_u16());
            // Drain body so timing reflects full transfer; capture bytes for stats.
            let bytes = resp.bytes().await.unwrap_or_default();
            RequestEvent {
                end: Instant::now(),
                latency_ms: req_start.elapsed().as_millis() as u64,
                status,
                error: None,
                bytes: bytes.len() as u64,
            }
        }
        Err(e) => RequestEvent {
            end: Instant::now(),
            latency_ms: req_start.elapsed().as_millis() as u64,
            status: None,
            error: Some(short_err(&e)),
            bytes: 0,
        },
    }
}

/// Choose the sliding-window length (seconds) based on elapsed time, matching
/// oha's auto-timescale: short window for short runs, expanding as the run
/// progresses so the chart stays readable.
fn auto_timescale(elapsed_secs: f64) -> f64 {
    if elapsed_secs < 1.0 {
        0.2 // 200ms
    } else if elapsed_secs < 10.0 {
        1.0
    } else if elapsed_secs < 60.0 {
        5.0
    } else {
        15.0
    }
}

/// Build 32 rolling RPS bins across the sliding window, matching oha's
/// "Requests / past X" bar chart. Returns (bin_center_secs_ago, count).
fn rps_bins(finished: &VecDeque<Finished>, now: Instant, timescale_secs: f64) -> Vec<(f64, u64)> {
    const N: usize = 32;
    let mut counts = vec![0u64; N];
    let bin_w = timescale_secs / N as f64;
    for f in finished.iter() {
        let ago = (now - f.end).as_secs_f64();
        if ago < 0.0 || ago >= timescale_secs {
            continue;
        }
        // bin 0 = newest, N-1 = oldest edge
        let idx = ((ago / bin_w) as usize).min(N - 1);
        counts[N - 1 - idx] += 1;
    }
    counts
        .into_iter()
        .enumerate()
        .map(|(i, c)| ((i as f64 + 0.5) * bin_w, c))
        .collect()
}

/// Aggregate per-request events into a running state, emit snapshots every
/// 200ms, and produce the final `StressResult` once all workers finish.
async fn run_aggregator(
    mut ev_rx: mpsc::UnboundedReceiver<RequestEvent>,
    snap_tx: mpsc::UnboundedSender<StressSnapshot>,
    configured_duration: Duration,
) -> StressResult {
    let start = Instant::now();
    let deadline = start + configured_duration;

    // All latencies for final percentiles.
    let mut all_latencies: Vec<u64> = Vec::with_capacity(4096);
    // Bounded queue of recent completions for sliding-window stats.
    // Retain up to ~60s worth (plenty for the longest window we'll ever render).
    let mut finished: VecDeque<Finished> = VecDeque::with_capacity(8192);

    let mut total: u64 = 0;
    let mut success: u64 = 0;
    let mut failed: u64 = 0;
    let mut total_in_window: u64 = 0; // requests completed by the deadline instant
    let mut total_bytes: u64 = 0;
    let mut status_dist: BTreeMap<u16, u64> = BTreeMap::new();
    let mut errors: BTreeMap<String, u64> = BTreeMap::new();
    let mut rps_timeline: Vec<(f64, f64)> = Vec::new();

    // We poll events and emit snapshots on a fixed tick.
    let mut tick = tokio::time::interval(Duration::from_millis(200));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    let mut last_snapshot_count: u64 = 0;
    let mut last_snapshot_instant = start;
    let mut done = false;
    let mut final_rps_bins: Vec<(f64, u64)> = Vec::new();
    let mut final_timescale: f64 = 1.0;

    loop {
        tokio::select! {
            _ = tick.tick() => {
                if done {
                    break;
                }
                let now = Instant::now();
                let elapsed = now - start;
                let elapsed_secs = elapsed.as_secs_f64();
                let timescale_secs = auto_timescale(elapsed_secs);

                // Drop finished entries older than 2x the timescale (cheap bound).
                let prune_cutoff = now - Duration::from_secs_f64(timescale_secs.max(1.0) * 2.0);
                while finished.front().map(|f| f.end < prune_cutoff).unwrap_or(false) {
                    finished.pop_front();
                }

                // Sliding-window latencies for live histogram / window stats.
                let win_start = now - Duration::from_secs_f64(timescale_secs);
                let window_lats: Vec<u64> = finished
                    .iter()
                    .filter(|f| f.end >= win_start)
                    .map(|f| f.latency_ms)
                    .collect();
                let win_count = window_lats.len();
                let window_rps = win_count as f64 / timescale_secs;

                let (w_min, w_max, w_mean) = if window_lats.is_empty() {
                    (0.0, 0.0, 0.0)
                } else {
                    let mut s = window_lats.clone();
                    s.sort_unstable();
                    let mn = s[0] as f64;
                    let mx = s[s.len() - 1] as f64;
                    let mean = s.iter().sum::<u64>() as f64 / s.len() as f64;
                    (mn, mx, mean)
                };

                let bins = histogram_bins(&window_lats);
                let win_pct = {
                    let mut s = window_lats.clone();
                    s.sort_unstable();
                    percentiles_from_sorted(&s)
                };

                let rps_bins = rps_bins(&finished, now, timescale_secs);

                let snapshot = StressSnapshot {
                    elapsed_secs,
                    duration_secs: configured_duration.as_secs_f64(),
                    completed: total,
                    success,
                    failed,
                    rps: total_in_window as f64 / configured_duration.as_secs_f64().max(f64::MIN_POSITIVE),
                    window_rps,
                    timescale_secs,
                    latency_bins: bins,
                    p50_ms: win_pct.p50,
                    p95_ms: win_pct.p95,
                    p99_ms: win_pct.p99,
                    window_min_ms: w_min,
                    window_max_ms: w_max,
                    window_mean_ms: w_mean,
                    rps_bins: rps_bins.clone(),
                    total_bytes,
                    status: format!("完成 {} 次", total),
                };
                let _ = snap_tx.send(snapshot);
                final_rps_bins = rps_bins;
                final_timescale = timescale_secs;

                // Track RPS timeline for the final line chart. Sample at ~1s
                // intervals (oha-style) so the chart stays readable and we
                // don't swamp the renderer with hundreds of points. Values are
                // rounded to 2 decimals for clean tooltip display.
                if elapsed <= configured_duration + Duration::from_millis(50) {
                    let last_sec = rps_timeline.last().map(|(t, _)| *t).unwrap_or(-1.0);
                    if elapsed_secs - last_sec >= 1.0 {
                        let dt = last_snapshot_instant.elapsed().as_secs_f64().max(0.001);
                        let delta = total - last_snapshot_count;
                        let instantaneous_rps = (delta as f64 / dt * 100.0).round() / 100.0;
                        rps_timeline.push((elapsed_secs, instantaneous_rps));
                        last_snapshot_count = total;
                        last_snapshot_instant = now;
                    }
                }
            }
            ev = ev_rx.recv() => {
                match ev {
                    Some(e) => {
                        total += 1;
                        let is_success = match e.status {
                            Some(code) => (200..400).contains(&code),
                            None => false,
                        };
                        if e.end <= deadline {
                            total_in_window += 1;
                        }
                        all_latencies.push(e.latency_ms);
                        total_bytes += e.bytes;
                        finished.push_back(Finished {
                            end: e.end,
                            latency_ms: e.latency_ms,
                            success: is_success,
                            bytes: e.bytes,
                        });
                        match e.status {
                            Some(code) => {
                                if is_success {
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
                        done = true;
                    }
                }
            }
        }
    }

    let elapsed = start.elapsed().as_secs_f64().max(f64::MIN_POSITIVE);
    // Ensure the RPS timeline ends at the final elapsed value (captures the
    // last partial second that wouldn't have triggered the 1s threshold).
    if let Some(last) = rps_timeline.last() {
        if (elapsed - last.0).abs() > 0.05 {
            let dt = last_snapshot_instant.elapsed().as_secs_f64().max(0.001);
            let delta = total - last_snapshot_count;
            let inst = (delta as f64 / dt * 100.0).round() / 100.0;
            rps_timeline.push((elapsed, inst));
        }
    } else {
        // No ticks fired (extremely short run): emit a single point.
        rps_timeline.push((elapsed, total as f64 / elapsed));
    }
    all_latencies.sort_unstable();
    let pct = percentiles_from_sorted(&all_latencies);
    let bins = histogram_bins(&all_latencies);
    let window_secs = configured_duration.as_secs_f64().max(f64::MIN_POSITIVE);

    StressResult {
        total_secs: elapsed,
        total,
        success,
        failed,
        rps: total_in_window as f64 / window_secs,
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
        rps_bins: final_rps_bins,
        rps_bins_timescale: final_timescale,
        total_bytes,
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
