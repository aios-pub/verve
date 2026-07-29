//! Light/dark theme toggle with full manual control (avoids the
//! `use_color_mode` ↔ FOUC-script interaction). Reads/writes the same
//! `leptos-use-color-scheme` key the inline FOUC script uses, so they stay
//! in sync.
//!
//! Priority: stored pref > system pref > **light** (light is the default).

use leptos::prelude::*;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast;

const STORAGE_KEY: &str = "leptos-use-color-scheme";

fn system_prefers_dark() -> bool {
    web_sys::window()
        .and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten())
        .map(|m| m.matches())
        .unwrap_or(false)
}

fn stored_theme() -> Option<bool> {
    web_sys::window()?
        .local_storage()
        .ok()?
        .and_then(|ls| ls.get_item(STORAGE_KEY).ok().flatten())
        .map(|v| match v.as_str() {
            "dark" => Some(true),
            "light" => Some(false),
            _ => None,
        })
        .flatten()
}

fn initial_is_dark() -> bool {
    // matches the FOUC script in index.html
    match stored_theme() {
        Some(d) => d,
        None => system_prefers_dark(),
    }
}

/// Apply a theme to <html> and persist it.
fn apply_theme(dark: bool) {
    let html = web_sys::window()
        .and_then(|w| w.document())
        .map(|d| d.document_element());
    if let Some(Some(el)) = html {
        let class_list = el.class_list();
        if dark {
            let _ = class_list.add_1("dark");
        } else {
            let _ = class_list.remove_1("dark");
        }
    }
    if let Some(ls) = web_sys::window().and_then(|w| w.local_storage().ok().flatten()) {
        let _ = ls.set_item(STORAGE_KEY, if dark { "dark" } else { "light" });
    }
}

#[component]
pub fn ThemeToggle() -> impl IntoView {
    // Signal seeded from the FOUC-resolved initial state.
    let (is_dark, set_is_dark) = signal(initial_is_dark());

    // Listen for system theme changes so an "auto" user sees updates.
    if let Some(w) = web_sys::window() {
        let cb = Closure::<dyn Fn()>::new(move || {
            // Only react if the user hasn't explicitly chosen a theme yet.
            if stored_theme().is_none() {
                let dark = system_prefers_dark();
                apply_theme(dark);
                set_is_dark.set(dark);
            }
        });
        if let Ok(Some(mq)) = w.match_media("(prefers-color-scheme: dark)") {
            let _ = mq.add_event_listener_with_callback("change", cb.as_ref().unchecked_ref());
        }
        // Leak the closure so it lives for the app lifetime.
        cb.forget();
    }

    let toggle = move |_| {
        let next = !is_dark.get();
        apply_theme(next);
        set_is_dark.set(next);
    };

    let title = Signal::derive(move || if is_dark.get() { "切换到亮色" } else { "切换到暗色" });

    view! {
        <button
            on:click=toggle
            title=move || title.get()
            aria-label=move || title.get()
            class="flex h-8 w-8 items-center justify-center rounded-lg border border-ink-700 text-ink-300 transition hover:border-bolt-500/50 hover:text-bolt-300"
        >
            // Sun (shown in dark mode)
            <Show when=move || is_dark.get()>
                <svg class="h-[18px] w-[18px]" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <circle cx="12" cy="12" r="4" />
                    <path d="M12 2v2M12 20v2M4.93 4.93l1.41 1.41M17.66 17.66l1.41 1.41M2 12h2M20 12h2M6.34 17.66l-1.41 1.41M19.07 4.93l-1.41 1.41" />
                </svg>
            </Show>
            // Moon (shown in light mode)
            <Show when=move || !is_dark.get()>
                <svg class="h-[18px] w-[18px]" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
                    <path d="M12 3a6 6 0 0 0 9 9 9 9 0 1 1-9-9Z" />
                </svg>
            </Show>
        </button>
    }
}
