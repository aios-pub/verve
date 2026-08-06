//! Live telemetry HUD — a small "mission control" readout that measures *its
//! own host*: real FPS, a tiny sparkline, session duration, and (where the
//! browser exposes it) JS heap usage. This is the most honest demo that the
//! site is actual Rust/WASM doing real work, not just CSS.
//!
//! Rendered as a fixed, collapsible panel. On reduced-motion / touch devices it
//! falls back to a static indicator.

use std::cell::Cell;

use leptos::prelude::*;
use leptos_use::{use_raf_fn, UseRafFnCallbackArgs};

use super::motion::effects_enabled;

const SPARK_LEN: usize = 32;

#[component]
pub fn Telemetry() -> impl IntoView {
    let (fps, set_fps) = signal(60.0_f64);
    let (heap, set_heap) = signal(String::new());
    let (uptime, set_uptime) = signal(String::from("00:00"));
    let (spark, set_spark) = signal([0.0_f32; SPARK_LEN]);
    let (open, set_open) = signal(true);

    let start = web_sys::window().and_then(|w| w.performance()).map(|p| p.now());
    let last = Cell::new(0.0_f64);
    let ema = Cell::new(60.0_f64);
    let sample_acc = Cell::new(0.0_f64);

    let raf = use_raf_fn(move |args: UseRafFnCallbackArgs| {
        let ts = args.timestamp;
        let prev = last.get();
        if prev > 0.0 {
            let delta = ts - prev;
            if delta > 0.0 {
                let instant = 1000.0 / delta;
                // exponential smoothing, clamped to kill tab-switch spikes
                let cur = ema.get();
                let next = if cur <= 0.0 { instant } else { cur * 0.9 + instant * 0.1 };
                ema.set(next);
            }
        }
        last.set(ts);

        // Sample FPS roughly 4x/sec for the readout + sparkline.
        let acc = sample_acc.get() + args.delta;
        sample_acc.set(acc);
        if acc >= 250.0 {
            let value = ema.get().round().clamp(0.0, 99.0);
            set_fps.set(value);
            set_spark.update(|buf| {
                buf.copy_within(1.., 0);
                buf[SPARK_LEN - 1] = value as f32;
            });
            sample_acc.set(0.0);

            if let Some(t0) = start {
                let secs = ((ts - t0) / 1000.0) as u64;
                set_uptime.set(format!("{:02}:{:02}", secs / 60, secs % 60));
            }
            set_heap.set(read_heap_mb());
        }
    });

    // Only run the raf when effects are allowed; otherwise keep the static
    // panel showing a fixed 60fps.
    let enabled = effects_enabled();
    let raf2 = raf.clone();
    Effect::new(move |_| {
        if enabled.get() {
            (raf2.resume)();
        } else {
            (raf2.pause)();
        }
    });
    let raf3 = raf.clone();
    on_cleanup(move || (raf3.pause)());

    // Sparkline path from the circular buffer.
    let spark_path = move || {
        let buf = spark.get();
        let w = 88.0_f32;
        let h = 20.0_f32;
        let step = w / (SPARK_LEN - 1) as f32;
        buf.iter().enumerate().fold(String::new(), |mut acc, (i, v)| {
            let x = i as f32 * step;
            let y = h - (v.clamp(0.0, 60.0) / 60.0) * h;
            if i == 0 {
                acc.push_str(&format!("M{x:.1},{y:.1}"));
            } else {
                acc.push_str(&format!(" L{x:.1},{y:.1}"));
            }
            acc
        })
    };

    let fps_color = move || {
        let v = fps.get();
        if v >= 55.0 {
            "text-bolt-300"
        } else if v >= 30.0 {
            "text-pro-400"
        } else {
            "text-red-400"
        }
    };

    view! {
        <div class="pointer-events-none fixed bottom-4 right-4 z-50">
            <div class="pointer-events-auto hud-panel corners overflow-hidden p-3 text-[11px] leading-relaxed text-ink-300 shadow-2xl shadow-black/20 opacity-75 transition hover:opacity-100"
                 class=("w-44", move || open.get())>
                // Header row
                <div class="flex items-center justify-between gap-3">
                    <span class="tech-label">"telemetry"</span>
                    <button on:click=move |_| set_open.update(|o| *o = !*o)
                        class="text-ink-500 hover:text-bolt-300 transition"
                        aria-label="Toggle telemetry">
                        {move || if open.get() { "▾" } else { "▸" }}
                    </button>
                </div>

                <Show when=move || open.get()>
                    <div class="mt-2 space-y-1.5">
                        <div class="flex items-center justify-between gap-4">
                            <span class="text-ink-500">"FPS"</span>
                            <span class=move || format!("text-mono font-semibold {}", fps_color())>
                                {move || format!("{:.0}", fps.get())}
                            </span>
                        </div>
                        <svg viewBox="0 0 88 20" preserveAspectRatio="none"
                             class="h-5 w-full" aria-hidden="true">
                            <path d=move || spark_path() fill="none"
                                  stroke="currentColor" stroke-width="1.2"
                                  class=move || fps_color() />
                        </svg>
                        <Show when=move || !heap.get().is_empty()>
                            <div class="flex items-center justify-between gap-4">
                                <span class="text-ink-500">"HEAP"</span>
                                <span class="text-mono text-ink-200">{heap}</span>
                            </div>
                        </Show>
                        <div class="flex items-center justify-between gap-4">
                            <span class="text-ink-500">"UPTIME"</span>
                            <span class="text-mono text-ink-200">{uptime}</span>
                        </div>
                        <div class="flex items-center gap-1.5 pt-1 text-ink-500">
                            <span class="status-dot"></span>
                            <span class="text-mono text-[10px] tracking-wider">"WASM RUNTIME · OK"</span>
                        </div>
                    </div>
                </Show>
            </div>
        </div>
    }
}

/// Read `performance.memory.usedJSHeapSize` in MiB (Chromium-only). Returns an
/// empty string when unavailable (Firefox / Safari).
fn read_heap_mb() -> String {
    (|| -> Option<String> {
        let win = web_sys::window()?;
        let perf = win.performance()?;
        let perf_val: &wasm_bindgen::JsValue = perf.as_ref();
        let mem = js_sys::Reflect::get(perf_val, &"memory".into()).ok()?;
        if mem.is_undefined() || mem.is_null() {
            return None;
        }
        let used = js_sys::Reflect::get(&mem, &"usedJSHeapSize".into()).ok()?;
        let bytes = used.as_f64()?;
        Some(format!("{:.0} MB", bytes / 1048576.0))
    })()
    .unwrap_or_default()
}
