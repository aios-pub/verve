//! Counts a number up from 0 to `value` once it scrolls into view. Uses
//! leptos-use's `use_raf_fn` (which manages the rAF lifecycle / cleanup) and
//! pauses itself once the target is reached.

use std::cell::Cell;
use std::rc::Rc;

use leptos::html::Span;
use leptos::prelude::*;
use leptos_use::{use_intersection_observer, use_raf_fn, UseRafFnCallbackArgs};

#[component]
pub fn CountUp(
    /// Target value.
    value: f64,
    /// Decimal places to render.
    #[prop(default = 0_u8)]
    decimals: u8,
    /// String appended after the number (e.g. "MB", "fps", "s").
    #[prop(default = "")]
    suffix: &'static str,
    /// String prepended before the number (e.g. "<", "~").
    #[prop(default = "")]
    prefix: &'static str,
    /// Duration of the count animation in milliseconds.
    #[prop(default = 1200.0_f64)]
    duration: f64,
) -> impl IntoView {
    let el: NodeRef<Span> = NodeRef::new();
    let (display, set_display) = signal(0.0_f64);

    let start_time: Rc<Cell<f64>> = Rc::new(Cell::new(0.0));
    let started = Rc::new(Cell::new(false));
    let done = Rc::new(Cell::new(false));

    let raf = use_raf_fn(move |args: UseRafFnCallbackArgs| {
        if done.get() {
            return;
        }
        let t0 = start_time.get();
        if t0 <= 0.0 {
            start_time.set(args.timestamp);
        }
        let t = ((args.timestamp - start_time.get()) / duration).clamp(0.0, 1.0);
        // ease-out cubic
        let eased = 1.0 - (1.0 - t).powi(3);
        set_display.set(eased * value);
        if t >= 1.0 {
            set_display.set(value);
            done.set(true);
        }
    });
    (raf.pause)();

    use_intersection_observer(el, {
        let raf = raf.clone();
        move |entries, _| {
            if started.get() {
                return;
            }
            if entries.iter().any(|e| e.is_intersecting()) {
                started.set(true);
                (raf.resume)();
            }
        }
    });

    let raf_c = raf.clone();
    on_cleanup(move || (raf_c.pause)());

    let fmt = move || {
        let v = display.get();
        format!("{prefix}{:.*}{suffix}", decimals as usize, v)
    };

    view! {
        <span node_ref=el class="tabular-nums">{fmt}</span>
    }
}
