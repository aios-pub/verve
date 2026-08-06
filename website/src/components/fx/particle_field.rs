//! WASM-driven constellation particle field rendered to a 2D canvas.
//!
//! Particles drift slowly, nearby ones are linked by thin lines (a "network"),
//! and the cursor gently pulls particles near it. Colors are read from the
//! theme CSS variables so it follows light/dark automatically.
//!
//! The whole component is gated behind [`super::motion::effects_enabled`] and
//! pauses while the tab is hidden — no raf runs on mobile or for reduced-motion
//! users.

use std::cell::RefCell;
use std::rc::Rc;

use leptos::html::Canvas;
use leptos::prelude::*;
use leptos_use::{
    use_document_visibility, use_element_size, use_mouse, use_raf_fn, UseRafFnCallbackArgs,
};
use wasm_bindgen::JsCast;
use web_sys::CanvasRenderingContext2d;

use super::motion::effects_enabled;

struct Particle {
    x: f64,
    y: f64,
    vx: f64,
    vy: f64,
    r: f64,
    pro: bool,
}

struct DrawState {
    css_w: f64,
    css_h: f64,
    bolt: (u8, u8, u8),
    pro: (u8, u8, u8),
}

impl Default for DrawState {
    fn default() -> Self {
        Self {
            css_w: -1.0,
            css_h: -1.0,
            bolt: (232, 249, 63),
            pro: (139, 92, 246),
        }
    }
}

#[component]
pub fn ParticleField() -> impl IntoView {
    let enabled = effects_enabled();
    let canvas_ref: NodeRef<Canvas> = NodeRef::new();
    let wrap_ref: NodeRef<leptos::html::Div> = NodeRef::new();

    let size = use_element_size(wrap_ref);
    let mouse = use_mouse();
    let visibility = use_document_visibility();

    let particles: Rc<RefCell<Vec<Particle>>> = Rc::new(RefCell::new(Vec::new()));
    let ctx: Rc<RefCell<Option<CanvasRenderingContext2d>>> = Rc::new(RefCell::new(None));
    let state = Rc::new(RefCell::new(DrawState::default()));

    let raf = {
        let ctx = Rc::clone(&ctx);
        let state = Rc::clone(&state);
        let particles = Rc::clone(&particles);

        use_raf_fn(move |_: UseRafFnCallbackArgs| {
            let Some(canvas) = canvas_ref.get() else { return };
            let w = size.width.get();
            let h = size.height.get();
            if w <= 0.0 || h <= 0.0 {
                return;
            }

            // Reconfigure on first paint or resize.
            {
                let st = state.borrow();
                if (st.css_w - w).abs() > 0.5 || (st.css_h - h).abs() > 0.5 {
                    drop(st);
                    reconfigure(&canvas, &ctx, &state, &particles, w, h);
                }
            }

            // Cursor relative to the canvas.
            let rect = canvas.get_bounding_client_rect();
            let mx = mouse.x.get() - rect.left();
            let my = mouse.y.get() - rect.top();
            let mouse_active = mx >= 0.0 && my >= 0.0 && mx <= w && my <= h;

            let (bolt, pro) = {
                let st = state.borrow();
                (st.bolt, st.pro)
            };

            // Integrate.
            {
                let mut ps = particles.borrow_mut();
                for p in ps.iter_mut() {
                    if mouse_active {
                        let dx = mx - p.x;
                        let dy = my - p.y;
                        let d2 = dx * dx + dy * dy;
                        if d2 < 140.0 * 140.0 && d2 > 1.0 {
                            let d = d2.sqrt();
                            let force = (1.0 - d / 140.0) * 0.06;
                            p.vx += (dx / d) * force;
                            p.vy += (dy / d) * force;
                        }
                    }
                    p.x += p.vx;
                    p.y += p.vy;
                    p.vx *= 0.985;
                    p.vy *= 0.985;
                    if p.x < -10.0 {
                        p.x = w + 10.0;
                    } else if p.x > w + 10.0 {
                        p.x = -10.0;
                    }
                    if p.y < -10.0 {
                        p.y = h + 10.0;
                    } else if p.y > h + 10.0 {
                        p.y = -10.0;
                    }
                }
            }

            // Draw.
            let c = match ctx.borrow().as_ref() {
                Some(c) => c.clone(),
                None => return,
            };
            c.clear_rect(0.0, 0.0, w, h);

            const LINK_DIST: f64 = 120.0;
            let ps = particles.borrow();

            for i in 0..ps.len() {
                let a = &ps[i];
                for b in ps.iter().skip(i + 1) {
                    let dx = a.x - b.x;
                    let dy = a.y - b.y;
                    let d = (dx * dx + dy * dy).sqrt();
                    if d < LINK_DIST {
                        let alpha = (1.0 - d / LINK_DIST) * 0.45;
                        let (r, g, bl) = bolt;
                        let _ = c.set_stroke_style_str(&format!(
                            "rgba({r},{g},{bl},{alpha:.3})"
                        ));
                        c.begin_path();
                        c.move_to(a.x, a.y);
                        c.line_to(b.x, b.y);
                        c.stroke();
                    }
                }
            }

            for p in ps.iter() {
                let (r, g, b) = if p.pro { pro } else { bolt };
                let _ = c.set_fill_style_str(&format!("rgb({r},{g},{b})"));
                c.begin_path();
                let _ = c.arc(p.x, p.y, p.r, 0.0, std::f64::consts::TAU);
                c.fill();
            }
        })
    };

    let raf_eff = raf.clone();
    Effect::new(move |_| {
        let on = enabled.get()
            && visibility.get() == web_sys::VisibilityState::Visible;
        if on {
            (raf_eff.resume)();
        } else {
            (raf_eff.pause)();
        }
    });

    let raf_clean = raf.clone();
    on_cleanup(move || {
        (raf_clean.pause)();
    });

    view! {
        <Show when=move || enabled.get()>
            <div node_ref=wrap_ref class="pointer-events-none absolute inset-0 z-[1]">
                <canvas node_ref=canvas_ref class="block h-full w-full" />
            </div>
        </Show>
    }
}

fn reconfigure(
    canvas: &web_sys::HtmlCanvasElement,
    ctx: &Rc<RefCell<Option<CanvasRenderingContext2d>>>,
    state: &Rc<RefCell<DrawState>>,
    particles: &Rc<RefCell<Vec<Particle>>>,
    w: f64,
    h: f64,
) {
    let dpr = web_sys::window()
        .map(|win| win.device_pixel_ratio())
        .unwrap_or(1.0)
        .clamp(1.0_f64, 2.0_f64);
    canvas.set_width((w * dpr).round() as u32);
    canvas.set_height((h * dpr).round() as u32);
    if let Some(c) = canvas
        .get_context("2d")
        .ok()
        .flatten()
        .and_then(|c| c.dyn_into::<CanvasRenderingContext2d>().ok())
    {
        c.set_transform(dpr, 0.0, 0.0, dpr, 0.0, 0.0).ok();
        *ctx.borrow_mut() = Some(c);
    }

    let mut st = state.borrow_mut();
    st.css_w = w;
    st.css_h = h;
    if let Some(c) = read_theme_color("--color-bolt-400") {
        st.bolt = c;
    }
    if let Some(c) = read_theme_color("--color-pro-500") {
        st.pro = c;
    }
    drop(st);

    let count = ((w * h) / 16_000.0).round() as usize;
    let count = count.clamp(26, 64);
    let mut ps = particles.borrow_mut();
    ps.clear();
    for _ in 0..count {
        ps.push(Particle {
            x: js_sys::Math::random() * w,
            y: js_sys::Math::random() * h,
            vx: (js_sys::Math::random() - 0.5) * 0.18,
            vy: (js_sys::Math::random() - 0.5) * 0.18,
            r: if js_sys::Math::random() > 0.85 { 1.8 } else { 1.1 },
            pro: js_sys::Math::random() < 0.16,
        });
    }
}

/// Read a CSS custom property from `<html>` and parse it as an RGB triple.
fn read_theme_color(prop: &str) -> Option<(u8, u8, u8)> {
    let win = web_sys::window()?;
    let doc = win.document()?;
    let root = doc.document_element()?;
    let val = win
        .get_computed_style(&root)
        .ok()??
        .get_property_value(prop)
        .ok()?
        .trim()
        .to_string();
    parse_color(&val)
}

fn parse_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim();
    if let Some(hex) = s.strip_prefix('#') {
        let h = hex.trim();
        if h.len() == 6 {
            let r = u8::from_str_radix(&h[0..2], 16).ok()?;
            let g = u8::from_str_radix(&h[2..4], 16).ok()?;
            let b = u8::from_str_radix(&h[4..6], 16).ok()?;
            return Some((r, g, b));
        }
        if h.len() == 3 {
            let r = u8::from_str_radix(&h[0..1], 16).ok()? * 17;
            let g = u8::from_str_radix(&h[1..2], 16).ok()? * 17;
            let b = u8::from_str_radix(&h[2..3], 16).ok()? * 17;
            return Some((r, g, b));
        }
    }
    if s.starts_with("rgb") {
        let inner = s
            .split_once('(')
            .map(|(_, rest)| rest)
            .and_then(|rest| rest.strip_suffix(')'))
            .unwrap_or(s);
        let mut parts = inner.split(',').filter_map(|p| {
            p.trim()
                .split('/')
                .next()
                .and_then(|n| n.trim().parse::<f64>().ok())
                .map(|n| n.round() as u8)
        });
        return Some((parts.next()?, parts.next()?, parts.next()?));
    }
    None
}
