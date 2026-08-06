//! Hero "boot line" — cycles through a handful of `module::status` style
//! phrases with a blinking caret. Lightweight: a plain interval swaps the label
//! and CSS handles the crossfade + caret. No per-frame work.

use leptos::prelude::*;
use leptos_use::use_interval_fn;

const LINES: &[&str] = &[
    "gpu_pipeline::online",
    "wasm_runtime::ready",
    "cold_boot < 1s",
    "render_backend: gpui",
    "memory::baseline 84MiB",
];

#[component]
pub fn BootLine() -> impl IntoView {
    let (idx, set_idx) = signal(0_usize);

    let pausable = use_interval_fn(
        move || {
            set_idx.update(|i| *i = (*i + 1) % LINES.len());
        },
        2800,
    );
    on_cleanup(move || (pausable.pause)());

    view! {
        <div class="mt-8 flex items-center justify-center gap-2 text-mono text-[12px] text-ink-500">
            <span class="status-dot"></span>
            <span class="relative inline-flex h-[1.4em] items-center overflow-hidden">
                {move || LINES[idx.get()]}
            </span>
            <span class="caret" aria-hidden="true"></span>
        </div>
    }
}
