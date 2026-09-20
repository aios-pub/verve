# Verve website

The official website for **Verve**, built with [Leptos](https://leptos.dev) 0.8 (Rust, CSR) + [Tailwind CSS](https://tailwindcss.com) v4, bundled by [Trunk](https://trunkrs.dev). Deployed to GitHub Pages at **https://aios-pub.github.io/verve/**.

## Stack

- **Framework:** Leptos 0.8 (CSR — client-side rendering, no server)
- **i18n:** `leptos_i18n` 0.6 (zh-CN default, English) — locales in `i18n/`, codegen via `build.rs`
- **Styling:** Tailwind CSS v4 (dark theme aligned with the GPUI desktop app)
- **Build/bundle:** Trunk 0.21 → static bundle in `dist/`

## Base path

The site is served from a subdirectory (`/verve/`), configured in **two places**:

1. **Trunk** — `public_url = "/verve/"` in `Trunk.toml` rewrites every asset href in `index.html` to an absolute `/verve/...` path.
2. **Router** — `<Router base="/verve">` in `src/lib.rs` so the leptos router strips the base when matching routes.
3. **`asset()` / `route()`** in `src/utils.rs` prefix static asset links and full anchor hrefs with `/verve`.

To retarget (e.g. a custom domain at the root), change `BASE_PATH` in `src/consts.rs`, `public_url` in `Trunk.toml`, the `Router base` in `src/lib.rs`, and the favicon/stylesheet hrefs in `index.html`.

## Develop

```bash
npm install
trunk serve          # → http://127.0.0.1:8080 (dev serves at root)
```

Tailwind is rebuilt automatically by the `build` hook in `Trunk.toml` on every rebuild.

## Production build

```bash
trunk build --release        # → dist/
```

Deployment is automated via `.github/workflows/website.yml` (push to `main`/`dev` touching `website/**`, `assets/**`, or `docs/verve-server.md`). The workflow also drops a `.nojekyll` and a `404.html` (SPA fallback) into `dist/`.

## Entry point note

Trunk's JS shim calls `init()` but **not** `main`, so the WASM entry runs via `#[wasm_bindgen(start)]` on `pub fn start()` in `src/main.rs`. A bare `fn main(){}` satisfies the bin-crate requirement. See the file for details.

## Content reuse

- Screenshots are copied from `../assets/verve_demo/` into `public/screenshots/`.
- The `verve-server` doc is embedded at build time via `include_str!("../../../../docs/verve-server.md")`.
- Release/download links point to `https://github.com/aios-pub/verve`.

## License

AGPL-3.0 (the Community edition license).
