//! CSR entry point. Trunk builds the default `website` bin target to WASM;
//! `#[wasm_bindgen(start)]` runs this automatically on instantiation.
//! A bare `fn main()` satisfies the bin crate requirement but is never called
//! by the WASM runtime (Trunk's JS only calls `init`, then the `start`
//! section fires).

use leptos::mount::mount_to_body;
use leptos::prelude::*;
use wasm_bindgen::prelude::*;
use website::App;

fn main() {}

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    mount_to_body(|| {
        view! { <App /> }
    });
}
