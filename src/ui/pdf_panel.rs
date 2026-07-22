//! PDF Editor panel — open, scroll through all pages, edit (add text/image,
//! erase, page ops), and save PDFs. Pdfium native library is extracted at compile
//! time from assets/lib/.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, Result as AnyResult};
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::popover::Popover;
use gpui_component::{
    h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Selectable as _, Sizable as _,
    WindowExt as _,
};
use image::{Frame as ImageFrame, RgbaImage};
use pdfium_render::prelude::{
    PdfColor, PdfDocument, PdfPagePaperSize, PdfPageObjectsCommon, PdfPageRenderRotation,
    PdfPoints, PdfRect, PdfRenderConfig, Pdfium, PdfiumError,
};
use gpui::{Pixels as Gpx, ScrollHandle};

#[derive(Clone, Copy, PartialEq, Eq)]
enum EditMode {
    View,
    AddText,
    AddImage,
    Erase,
}

#[derive(Clone)]
struct PageRender {
    image: Arc<RenderImage>,
    /// (render_w_px, render_h_px, page_w_pts, page_h_pts)
    meta: (u32, u32, f32, f32),
}

pub struct PdfPanel {
    file_path: Option<PathBuf>,
    edited_bytes: Option<Vec<u8>>,
    pdfium_available: bool,
    page_count: Option<u32>,
    /// Index of the page currently scrolled-to (topmost visible). Driven by ScrollHandle.
    current_page: u32,
    /// Lazily-rendered page cache.
    page_cache: HashMap<u32, PageRender>,
    /// Pages currently being rendered in the background (in-flight set to avoid duplicate jobs).
    rendering: std::collections::HashSet<u32>,
    edit_mode: EditMode,
    erase_start: Option<(u32, f32, f32)>, // (page_idx, x_pts, y_pts)
    /// Shared with each page's canvas prepaint closure.
    /// Key = page_idx; value = bounds of that page in the scroll container.
    page_bounds: Rc<RefCell<HashMap<u32, Bounds<Gpx>>>>,
    /// GPUI scroll handle for virtualization.
    scroll_handle: ScrollHandle,
    _subs: Vec<Subscription>,
}

impl PdfPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        let pdfium_available = Self::bind_pdfium().is_ok();
        Self {
            file_path: None,
            edited_bytes: None,
            pdfium_available,
            page_count: None,
            current_page: 0,
            page_cache: HashMap::new(),
            rendering: std::collections::HashSet::new(),
            edit_mode: EditMode::View,
            erase_start: None,
            page_bounds: Rc::new(RefCell::new(HashMap::new())),
            scroll_handle: ScrollHandle::new(),
            _subs: Vec::new(),
        }
    }

    fn bind_pdfium() -> Result<Pdfium, PdfiumError> {
        let lib_name = Pdfium::pdfium_platform_library_name();
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let local = exe_dir.join(&lib_name);
                if local.exists() {
                    let bindings = Pdfium::bind_to_library(local.to_string_lossy().as_ref())?;
                    return Ok(Pdfium::new(bindings));
                }
            }
        }
        let out_dir = env!("OUT_DIR");
        let out_lib_path = PathBuf::from(out_dir).join("pdfium").join("lib").join(&lib_name);
        if out_lib_path.exists() {
            let bindings = Pdfium::bind_to_library(out_lib_path.to_string_lossy().as_ref())?;
            return Ok(Pdfium::new(bindings));
        }
        let bindings = Pdfium::bind_to_system_library()?;
        Ok(Pdfium::new(bindings))
    }

    pub fn open_pdf_from_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_pdf(window, cx);
    }

    fn open_pdf(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .add_filter("PDF Files", &["pdf"])
                .set_title("Open PDF File")
                .pick_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let _ = ent.update(cx, |this, cx| match std::fs::read(&path) {
                            Ok(bytes) => {
                                this.file_path = Some(path);
                                this.edited_bytes = Some(bytes);
                                this.page_count = None;
                                this.current_page = 0;
                                this.page_cache.clear();
                                this.rendering.clear();
                                this.erase_start = None;
                                this.edit_mode = EditMode::View;
                                this.page_bounds.borrow_mut().clear();
                                this.scroll_handle.scroll_to_item(0);
                                this.refresh_page_count(cx);
                            }
                            Err(e) => log::error!("failed to read pdf: {e:?}"),
                        });
                        break;
                    }
                    Ok(None) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        })
        .detach();
    }

    fn refresh_page_count(&mut self, cx: &mut Context<Self>) {
        let Some(bytes) = self.edited_bytes.clone() else {
            return;
        };
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let count = cx
                .background_executor()
                .spawn(async move {
                    let pdfium = Self::bind_pdfium().ok()?;
                    let doc = pdfium.load_pdf_from_byte_slice(&bytes, None).ok()?;
                    Some(doc.pages().len() as u32)
                })
                .await;
            let _ = ent.update(cx, |this, cx| {
                if let Some(n) = count {
                    this.page_count = Some(n);
                    if this.current_page >= n {
                        this.current_page = n.saturating_sub(1);
                    }
                    this.ensure_visible_pages_rendered(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Kick off renders for pages near the current visible range.
    fn ensure_visible_pages_rendered(&mut self, cx: &mut Context<Self>) {
        let Some(total) = self.page_count else {
            return;
        };
        if total == 0 {
            return;
        }
        // Determine visible range from scroll handle. When the handle hasn't tracked
        // children yet (first render), fall back to 0..3.
        let max_offset = self.scroll_handle.max_offset();
        let visible_range = if max_offset.y > px(0.) {
            // Scroll has meaningful bounds; use top/bottom item.
            let top = self.scroll_handle.top_item() as u32;
            let bottom = self.scroll_handle.bottom_item() as u32;
            let hi = if bottom > top { bottom + 1 } else { top + 2 };
            (top.saturating_sub(1), hi.min(total.saturating_sub(1)))
        } else {
            (0, 2.min(total.saturating_sub(1)))
        };
        let (lo, hi) = visible_range;
        self.current_page = lo.min(total.saturating_sub(1));

        for idx in lo..=hi {
            if self.page_cache.contains_key(&idx) || self.rendering.contains(&idx) {
                continue;
            }
            self.render_page(idx, cx);
        }
        // Prune far-away pages from cache.
        let keep_lo = lo.saturating_sub(3);
        let keep_hi = (hi + 3).min(total.saturating_sub(1));
        let pages_to_drop: Vec<u32> = self
            .page_cache
            .keys()
            .copied()
            .filter(|i| *i < keep_lo || *i > keep_hi)
            .collect();
        for i in pages_to_drop {
            self.page_cache.remove(&i);
        }
    }

    fn render_page(&mut self, page_idx: u32, cx: &mut Context<Self>) {
        let Some(bytes) = self.edited_bytes.clone() else {
            return;
        };
        if self.rendering.contains(&page_idx) {
            return;
        }
        self.rendering.insert(page_idx);
        const MAX_DIM_PX: i32 = 1200;
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    Self::render_page_from_bytes(&bytes, page_idx, MAX_DIM_PX)
                })
                .await;
            let _ = ent.update(cx, |this, cx| {
                this.rendering.remove(&page_idx);
                match result {
                    Ok(Some((img, meta))) => {
                        this.page_cache.insert(
                            page_idx,
                            PageRender {
                                image: Arc::new(img),
                                meta,
                            },
                        );
                    }
                    Ok(None) => log::warn!("pdf page {page_idx} out of range"),
                    Err(e) => log::error!("pdf render page {page_idx} failed: {e:?}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_page_from_bytes(
        bytes: &[u8],
        page_idx: u32,
        max_dim_px: i32,
    ) -> AnyResult<Option<(RenderImage, (u32, u32, f32, f32))>> {
        let pdfium = Self::bind_pdfium().map_err(|e| anyhow::anyhow!("pdfium: {e}"))?;
        let doc = pdfium
            .load_pdf_from_byte_slice(bytes, None)
            .map_err(|e| anyhow::anyhow!("load pdf: {e}"))?;
        let pages = doc.pages();
        if page_idx as u16 >= pages.len() {
            return Ok(None);
        }
        let page = pages
            .get(page_idx as u16)
            .map_err(|e| anyhow::anyhow!("get page: {e}"))?;
        let w_pts = page.width().value;
        let h_pts = page.height().value;
        let aspect = w_pts / h_pts.max(1.0);
        let (target_w, target_h) = if aspect >= 1.0 {
            let w = max_dim_px;
            let h = (w as f32 / aspect) as i32;
            (w, h)
        } else {
            let h = max_dim_px;
            let w = (h as f32 * aspect) as i32;
            (w, h)
        };
        let config = PdfRenderConfig::new().set_target_size(target_w, target_h);
        let bitmap = page
            .render_with_config(&config)
            .map_err(|e| anyhow::anyhow!("render: {e}"))?;
        let width = bitmap.width() as u32;
        let height = bitmap.height() as u32;
        let mut bgra = bitmap.as_raw_bytes();
        let expected = width as usize * height as usize * 4;
        if bgra.len() < expected {
            anyhow::bail!("bitmap too small: {} < {}", bgra.len(), expected);
        }
        bgra.truncate(expected);
        let rgba_img = RgbaImage::from_raw(width, height, bgra)
            .ok_or_else(|| anyhow::anyhow!("RgbaImage from_raw failed"))?;
        let frame = ImageFrame::new(rgba_img);
        let ri = RenderImage::new(vec![frame]);
        Ok(Some((ri, (width, height, w_pts, h_pts))))
    }

    fn with_doc_mut<F>(&mut self, cx: &mut Context<Self>, describe: &str, f: F)
    where
        F: FnOnce(&Pdfium, &mut PdfDocument) -> AnyResult<()> + Send + 'static,
    {
        let Some(bytes_in) = self.edited_bytes.clone() else {
            return;
        };
        let describe = describe.to_string();
        cx.notify();
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            let result: AnyResult<Vec<u8>> = cx
                .background_executor()
                .spawn(async move {
                    let pdfium = Self::bind_pdfium().map_err(|e| anyhow::anyhow!("pdfium: {e}"))?;
                    let mut doc = pdfium
                        .load_pdf_from_byte_slice(&bytes_in, None)
                        .map_err(|e| anyhow::anyhow!("load: {e}"))?;
                    f(&pdfium, &mut doc)?;
                    let out = doc
                        .save_to_bytes()
                        .map_err(|e| anyhow::anyhow!("save: {e}"))?;
                    Ok(out)
                })
                .await;
            let _ = ent.update(cx, |this, cx| {
                match result {
                    Ok(new_bytes) => {
                        this.edited_bytes = Some(new_bytes);
                        log::info!("pdf edit ok: {describe}");
                        // Drop cache: everything changed.
                        this.page_cache.clear();
                        this.rendering.clear();
                        this.refresh_page_count(cx);
                    }
                    Err(e) => log::error!("pdf edit '{describe}' failed: {e:?}"),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn add_page_after_current(&mut self, cx: &mut Context<Self>) {
        let ins_at = (self.current_page + 1) as u16;
        self.with_doc_mut(cx, "add page", move |_pdfium, doc| {
            doc.pages_mut()
                .create_page_at_index(PdfPagePaperSize::a4(), ins_at)
                .map(|_| ())
                .map_err(|e| anyhow::anyhow!("create_page: {e}"))?;
            Ok(())
        });
    }

    fn delete_current_page(&mut self, cx: &mut Context<Self>) {
        let idx = self.current_page as u16;
        self.with_doc_mut(cx, "delete page", move |_pdfium, doc| {
            let page = doc
                .pages()
                .get(idx)
                .map_err(|e| anyhow::anyhow!("get page: {e}"))?;
            page.delete().map_err(|e| anyhow::anyhow!("delete: {e}"))?;
            Ok(())
        });
    }

    fn rotate_current(&mut self, cx: &mut Context<Self>, delta: i32) {
        let idx = self.current_page as u16;
        self.with_doc_mut(cx, "rotate page", move |_pdfium, doc| {
            let mut page = doc
                .pages_mut()
                .get(idx)
                .map_err(|e| anyhow::anyhow!("get page: {e}"))?;
            let current = page.rotation().map_err(|e| anyhow::anyhow!("rotation: {e}"))?;
            let current_val = match current {
                PdfPageRenderRotation::None => 0,
                PdfPageRenderRotation::Degrees90 => 1,
                PdfPageRenderRotation::Degrees180 => 2,
                PdfPageRenderRotation::Degrees270 => 3,
            };
            let new_val = ((current_val + delta) % 4 + 4) % 4;
            let new_rot = match new_val {
                0 => PdfPageRenderRotation::None,
                1 => PdfPageRenderRotation::Degrees90,
                2 => PdfPageRenderRotation::Degrees180,
                _ => PdfPageRenderRotation::Degrees270,
            };
            page.set_rotation(new_rot);
            Ok(())
        });
    }

    fn merge_from_path(&mut self, cx: &mut Context<Self>, other_path: PathBuf) {
        self.with_doc_mut(cx, "merge pdf", move |pdfium, doc| {
            let other_bytes = std::fs::read(&other_path).context("read merge source")?;
            let other_doc = pdfium
                .load_pdf_from_byte_slice(&other_bytes, None)
                .map_err(|e| anyhow::anyhow!("load merge source: {e}"))?;
            doc.pages_mut()
                .append(&other_doc)
                .map_err(|e| anyhow::anyhow!("append: {e}"))?;
            Ok(())
        });
    }

    fn add_text_at_impl(
        &mut self,
        cx: &mut Context<Self>,
        page_idx: u16,
        x_pts: f32,
        y_pts: f32,
        text: String,
    ) {
        let text = if text.trim().is_empty() {
            " ".to_string()
        } else {
            text
        };
        self.with_doc_mut(cx, "add text", move |_pdfium, doc| {
            let font = doc.fonts_mut().helvetica();
            let mut page = doc
                .pages_mut()
                .get(page_idx)
                .map_err(|e| anyhow::anyhow!("get page: {e}"))?;
            page.objects_mut()
                .create_text_object(
                    PdfPoints::new(x_pts),
                    PdfPoints::new(y_pts),
                    &text,
                    font,
                    PdfPoints::new(12.0),
                )
                .map_err(|e| anyhow::anyhow!("create text: {e}"))?;
            Ok(())
        });
    }

    fn add_image_at(
        &mut self,
        cx: &mut Context<Self>,
        page_idx: u16,
        x_pts: f32,
        y_pts: f32,
        img_path: PathBuf,
    ) {
        self.with_doc_mut(cx, "add image", move |_pdfium, doc| {
            let dyn_img = image::open(&img_path).context("open image")?;
            let mut page = doc
                .pages_mut()
                .get(page_idx)
                .map_err(|e| anyhow::anyhow!("get page: {e}"))?;
            page.objects_mut()
                .create_image_object(
                    PdfPoints::new(x_pts),
                    PdfPoints::new(y_pts),
                    &dyn_img,
                    Some(PdfPoints::new(200.0)),
                    None,
                )
                .map_err(|e| anyhow::anyhow!("create image: {e}"))?;
            Ok(())
        });
    }

    fn erase_rect(
        &mut self,
        cx: &mut Context<Self>,
        page_idx: u16,
        x1: f32,
        y1: f32,
        x2: f32,
        y2: f32,
    ) {
        let left = x1.min(x2);
        let right = x1.max(x2);
        let bottom = y1.min(y2);
        let top = y1.max(y2);
        self.with_doc_mut(cx, "erase rect", move |_pdfium, doc| {
            let mut page = doc
                .pages_mut()
                .get(page_idx)
                .map_err(|e| anyhow::anyhow!("get page: {e}"))?;
            let rect = PdfRect::new(
                PdfPoints::new(bottom),
                PdfPoints::new(left),
                PdfPoints::new(top),
                PdfPoints::new(right),
            );
            page.objects_mut()
                .create_path_object_rect(rect, None, None, Some(PdfColor::WHITE))
                .map_err(|e| anyhow::anyhow!("create rect: {e}"))?;
            Ok(())
        });
    }

    fn save_as(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(bytes) = self.edited_bytes.clone() else {
            return;
        };
        let suggested = self
            .file_path
            .as_ref()
            .and_then(|p| p.file_stem().map(|s| s.to_string_lossy().to_string()))
            .unwrap_or_else(|| "edited".to_string())
            + ".pdf";
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .set_file_name(&suggested)
                .add_filter("PDF Files", &["pdf"])
                .save_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let bytes_c = bytes.clone();
                        let res = cx
                            .background_executor()
                            .spawn(async move { std::fs::write(&path, &bytes_c).map_err(|e| e) })
                            .await;
                        let _ = ent.update(cx, |_this, _cx| match res {
                            Ok(()) => log::info!("pdf saved"),
                            Err(e) => log::error!("save failed: {e:?}"),
                        });
                        break;
                    }
                    Ok(None) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        })
        .detach();
    }

    fn merge_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .add_filter("PDF Files", &["pdf"])
                .set_title("Merge PDF (append)")
                .pick_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let _ = ent.update(cx, |this, cx| this.merge_from_path(cx, path));
                        break;
                    }
                    Ok(None) => break,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(50))
                            .await;
                    }
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                }
            }
        })
        .detach();
    }

    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page == 0 {
            return;
        }
        let target = self.current_page - 1;
        self.scroll_handle.scroll_to_top_of_item(target as usize);
        self.current_page = target;
        // Directly render the target page and neighbors.
        for i in target.saturating_sub(1)..=(target + 1).min(self.page_count.unwrap_or(0).saturating_sub(1)) {
            if !self.page_cache.contains_key(&i) && !self.rendering.contains(&i) {
                self.render_page(i, cx);
            }
        }
        cx.notify();
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        let Some(total) = self.page_count else {
            return;
        };
        if self.current_page + 1 >= total {
            return;
        }
        let target = self.current_page + 1;
        self.scroll_handle.scroll_to_top_of_item(target as usize);
        self.current_page = target;
        for i in target.saturating_sub(1)..=(target + 1).min(total.saturating_sub(1)) {
            if !self.page_cache.contains_key(&i) && !self.rendering.contains(&i) {
                self.render_page(i, cx);
            }
        }
        cx.notify();
    }

    fn set_mode(&mut self, mode: EditMode, cx: &mut Context<Self>) {
        self.edit_mode = mode;
        self.erase_start = None;
        cx.notify();
    }

    fn window_pos_to_pdf_points(
        &self,
        page_idx: u32,
        win_pos: Point<Gpx>,
    ) -> Option<(f32, f32)> {
        let bounds = self.page_bounds.borrow().get(&page_idx).cloned()?;
        let pr = self.page_cache.get(&page_idx)?;
        let (img_w, img_h, page_w_pts, page_h_pts) = pr.meta;
        let (img_w, img_h) = (img_w as f32, img_h as f32);
        let cont_w = bounds.size.width.as_f32();
        let cont_h = bounds.size.height.as_f32();
        if cont_w <= 0.0 || cont_h <= 0.0 || img_w <= 0.0 || img_h <= 0.0 {
            return None;
        }
        let scale = (cont_w / img_w).min(cont_h / img_h);
        let disp_w = img_w * scale;
        let disp_h = img_h * scale;
        let off_x = (cont_w - disp_w) * 0.5;
        let off_y = (cont_h - disp_h) * 0.5;
        let rel_x = (win_pos.x.as_f32() - bounds.origin.x.as_f32()) - off_x;
        let rel_y = (win_pos.y.as_f32() - bounds.origin.y.as_f32()) - off_y;
        if rel_x < 0.0 || rel_y < 0.0 || rel_x > disp_w || rel_y > disp_h {
            return None;
        }
        let img_x = rel_x / scale;
        let img_y = rel_y / scale;
        let pdf_x = (img_x / img_w) * page_w_pts;
        let pdf_y = page_h_pts - (img_y / img_h) * page_h_pts;
        Some((pdf_x, pdf_y))
    }

    fn handle_page_click(
        &mut self,
        page_idx: u32,
        pos: Point<Gpx>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.edited_bytes.is_none() || !self.page_cache.contains_key(&page_idx) {
            return;
        }
        let Some((x_pts, y_pts)) = self.window_pos_to_pdf_points(page_idx, pos) else {
            return;
        };
        let page_idx_u16 = page_idx as u16;
        match self.edit_mode {
            EditMode::View => {}
            EditMode::AddText => {
                let (x_cap, y_cap) = (x_pts, y_pts);
                let input = cx.new(|cx| {
                    let mut s = InputState::new(window, cx).placeholder("Text to insert");
                    s.set_value("", window, cx);
                    s
                });
                let ent = cx.entity();
                window.open_dialog(cx, move |dialog, _window, _cx| {
                    let input_c = input.clone();
                    let input_f = input.clone();
                    let ent_f = ent.clone();
                    dialog
                        .title("Add text")
                        .content(move |content, _window, _cx| {
                            content.child(
                                v_flex()
                                    .p_4()
                                    .w(px(340.))
                                    .gap_2()
                                    .child(Input::new(&input_c).small()),
                            )
                        })
                        .footer(
                            Button::new("pdf-add-text-confirm")
                                .primary()
                                .small()
                                .label("Insert")
                                .on_click(move |_ev, window, cx| {
                                    let text = input_f.read(cx).value().to_string();
                                    window.close_dialog(cx);
                                    let _ = ent_f.update(cx, |this, cx| {
                                        this.add_text_at_impl(
                                            cx, page_idx_u16, x_cap, y_cap, text,
                                        );
                                        this.set_mode(EditMode::View, cx);
                                    });
                                }),
                        )
                });
            }
            EditMode::AddImage => {
                let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
                std::thread::spawn(move || {
                    let res = rfd::FileDialog::new()
                        .add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "webp"])
                        .pick_file();
                    let _ = tx.send(res.map(|p| p.to_path_buf()));
                });
                let ent = cx.entity().downgrade();
                let (x_c, y_c) = (x_pts, y_pts);
                cx.spawn(async move |_this, cx| {
                    loop {
                        match rx.try_recv() {
                            Ok(Some(p)) => {
                                let _ = ent.update(cx, |this, cx| {
                                    this.add_image_at(cx, page_idx_u16, x_c, y_c, p);
                                    this.set_mode(EditMode::View, cx);
                                });
                                break;
                            }
                            Ok(None) => break,
                            Err(std::sync::mpsc::TryRecvError::Empty) => {
                                cx.background_executor()
                                    .timer(std::time::Duration::from_millis(50))
                                    .await;
                            }
                            Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                        }
                    }
                })
                .detach();
            }
            EditMode::Erase => {
                if self.erase_start.as_ref().map(|e| e.0) != Some(page_idx) {
                    self.erase_start = Some((page_idx, x_pts, y_pts));
                    cx.notify();
                } else {
                    let (_, x1, y1) = self.erase_start.take().unwrap();
                    self.erase_rect(cx, page_idx_u16, x1, y1, x_pts, y_pts);
                    cx.notify();
                }
            }
        }
    }

}

impl Render for PdfPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let has_doc = self.edited_bytes.is_some();
        let mode = self.edit_mode;
        let total = self.page_count.unwrap_or(0);
        let cur = self.current_page + 1;
        let is_busy = !self.rendering.is_empty();

        let toolbar = h_flex()
            .gap_2()
            .items_center()
            .flex_wrap()
            .child(
                Button::new("pdf-open")
                    .label(rust_i18n::t!("pdf.open_btn").to_string())
                    .icon(IconName::FolderOpen)
                    .small()
                    .ghost()
                    .on_click(cx.listener(|this, _, window, cx| this.open_pdf(window, cx))),
            )
            .child(
                Button::new("pdf-saveas")
                    .label("Save As…")
                    .icon(IconName::ExternalLink)
                    .small()
                    .ghost()
                    .disabled(!has_doc || is_busy)
                    .on_click(cx.listener(|this, _, window, cx| this.save_as(window, cx))),
            )
            .child(div().w(px(8.)))
            .child(
                Button::new("pdf-add-page")
                    .icon(IconName::Plus)
                    .small()
                    .ghost()
                    .tooltip("Insert blank A4 page after current")
                    .disabled(!has_doc || is_busy)
                    .on_click(cx.listener(|this, _, _, cx| this.add_page_after_current(cx))),
            )
            .child({
                let ent = cx.entity();
                Button::new("pdf-del-page")
                    .icon(IconName::Delete)
                    .small()
                    .ghost()
                    .tooltip("Delete current page")
                    .disabled(!has_doc || is_busy)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        let ent = ent.clone();
                        window.open_dialog(cx, move |dialog, _window, _cx| {
                            let ent_in = ent.clone();
                            dialog
                                .title("Delete page")
                                .content(move |c, _w, _cx| {
                                    c.child(
                                        v_flex()
                                            .p_4()
                                            .w(px(320.))
                                            .child("Delete the current page? This cannot be undone."),
                                    )
                                })
                                .footer(
                                    h_flex()
                                        .gap_2()
                                        .justify_end()
                                        .child(
                                            Button::new("pdf-del-cancel")
                                                .ghost()
                                                .small()
                                                .label("Cancel")
                                                .on_click(|_e, window, cx| window.close_dialog(cx)),
                                        )
                                        .child(
                                            Button::new("pdf-del-confirm")
                                                .danger()
                                                .small()
                                                .label("Delete")
                                                .on_click(move |_e, window, cx| {
                                                    window.close_dialog(cx);
                                                    let _ = ent_in.update(cx, |this, cx| {
                                                        this.delete_current_page(cx)
                                                    });
                                                }),
                                        ),
                                )
                        });
                    }))
            })
            .child(
                Popover::new("pdf-rotate-pop")
                    .anchor(Anchor::BottomLeft)
                    .trigger(
                        Button::new("pdf-rotate")
                            .small()
                            .ghost()
                            .label("Rotate")
                            .icon(IconName::ChevronDown)
                            .disabled(!has_doc || is_busy),
                    )
                    .p(px(4.))
                    .child({
                        let ent = cx.entity().downgrade();
                        v_flex().w(px(170.)).gap(px(2.)).children([
                            Self::rotate_item("90° Clockwise", ent.clone(), 1),
                            Self::rotate_item("90° Counter-CW", ent.clone(), 3),
                            Self::rotate_item("180°", ent, 2),
                        ])
                    }),
            )
            .child(
                Button::new("pdf-merge")
                    .icon(IconName::Copy)
                    .small()
                    .ghost()
                    .label("Merge…")
                    .tooltip("Append pages from another PDF")
                    .disabled(!has_doc || is_busy)
                    .on_click(cx.listener(|this, _, window, cx| this.merge_dialog(window, cx))),
            )
            .child(div().w(px(8.)))
            .child(
                Button::new("pdf-prev")
                    .icon(IconName::ChevronLeft)
                    .small()
                    .ghost()
                    .disabled(self.current_page == 0 || !has_doc || is_busy)
                    .on_click(cx.listener(|this, _, _, cx| this.prev_page(cx))),
            )
            .child(div().text_sm().child(format!("{cur} / {total}")))
            .child(
                Button::new("pdf-next")
                    .icon(IconName::ChevronRight)
                    .small()
                    .ghost()
                    .disabled(total == 0 || self.current_page + 1 >= total || !has_doc || is_busy)
                    .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
            )
            .child(div().w(px(8.)))
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("mode-view")
                            .small()
                            .ghost()
                            .label("View")
                            .selected(mode == EditMode::View)
                            .on_click(cx.listener(|this, _, _, cx| this.set_mode(EditMode::View, cx))),
                    )
                    .child(
                        Button::new("mode-text")
                            .small()
                            .ghost()
                            .label("Text")
                            .selected(mode == EditMode::AddText)
                            .disabled(!has_doc)
                            .on_click(cx.listener(|this, _, _, cx| this.set_mode(EditMode::AddText, cx))),
                    )
                    .child(
                        Button::new("mode-image")
                            .small()
                            .ghost()
                            .label("Image")
                            .selected(mode == EditMode::AddImage)
                            .disabled(!has_doc)
                            .on_click(cx.listener(|this, _, _, cx| this.set_mode(EditMode::AddImage, cx))),
                    )
                    .child(
                        Button::new("mode-erase")
                            .small()
                            .ghost()
                            .label("Erase")
                            .selected(mode == EditMode::Erase)
                            .disabled(!has_doc)
                            .on_click(cx.listener(|this, _, _, cx| this.set_mode(EditMode::Erase, cx))),
                    ),
            );

        let main = if !self.pdfium_available {
            v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .gap_2()
                .p_4()
                .child(
                    div()
                        .text_color(theme.danger)
                        .child("Pdfium not available. PDF editing is disabled."),
                )
                .into_any_element()
        } else if !has_doc {
            v_flex()
                .flex_1()
                .min_h_0()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(theme.border)
                .rounded(px(4.))
                .bg(theme.muted.opacity(0.2))
                .gap_2()
                .child(IconName::File)
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child(rust_i18n::t!("pdf.no_file").to_string()),
                )
                .into_any_element()
        } else {
            // Build the per-page scroll list.
            let mut pages_col = v_flex()
                .id("pdf-pages-scroll")
                .track_scroll(&self.scroll_handle)
                .overflow_y_scroll()
                .flex_1()
                .min_h_0()
                .w_full()
                .gap_3()
                .p_4()
                // Re-evaluate virtualization when scroll happens.
                .on_scroll_wheel(cx.listener(|this: &mut Self, _ev: &ScrollWheelEvent, _window, cx| {
                    // Render pages near the current scroll position. Since scroll wheel
                    // events fire during scrolling, just re-render the pages around top_item.
                    this.ensure_visible_pages_rendered(cx);
                    cx.notify();
                }));

            // Page cards for each known page index.
            if total > 0 {
                let mut cards: Vec<AnyElement> = Vec::with_capacity(total as usize);
                for idx in 0..total {
                    cards.push(self.render_page_card(idx, cx));
                }
                pages_col = pages_col.children(cards);
            }

            v_flex()
                .flex_1()
                .min_h_0()
                .size_full()
                .child(pages_col)
                .child(
                    div()
                        .py_1()
                        .px_2()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child(match (mode, self.erase_start.as_ref()) {
                            (EditMode::View, _) => String::new(),
                            (EditMode::AddText, _) => "Click a page to place text".to_string(),
                            (EditMode::AddImage, _) => "Click a page to place an image".to_string(),
                            (EditMode::Erase, Some(s)) => {
                                format!(
                                    "Erasing: click again on page {} to set the rectangle end",
                                    s.0 + 1
                                )
                            }
                            (EditMode::Erase, None) => {
                                "Click once to set rectangle start, then again to finish".to_string()
                            }
                        }),
                )
                .into_any_element()
        };

        v_flex()
            .size_full()
            .bg(theme.background)
            .gap_3()
            .p_4()
            .child(toolbar)
            .child(main)
    }
}

impl PdfPanel {
    fn render_page_card(&mut self, idx: u32, cx: &mut Context<Self>) -> AnyElement {
        let bounds_cell = self.page_bounds.clone();
        let ent = cx.entity().downgrade();
        let idx_for_click = idx;
        let has_render = self.page_cache.contains_key(&idx);
        let pr = self.page_cache.get(&idx).cloned();

        // Use canvas: prepaint records bounds for click coord translation; paint
        // draws the page image using paint_image.
        let canvas = canvas(
            move |bounds, _window, cx| {
                bounds_cell.borrow_mut().insert(idx, bounds);
                let _ = ent.update(cx, |this, cx| {
                    this.notify_page_visible(cx, idx);
                });
            },
            move |bounds, (), window, _cx| {
                let Some(pr) = pr.clone() else {
                    return;
                };
                let disp_w = bounds.size.width.as_f32();
                let disp_h = bounds.size.height.as_f32();
                let sz = pr.image.size(0);
                let img_aspect = sz.width.0 as f32 / sz.height.0 as f32;
                let cont_aspect = disp_w / disp_h.max(1.0);
                let (w, h) = if cont_aspect >= img_aspect {
                    let h = disp_h;
                    let w = h * img_aspect;
                    (w, h)
                } else {
                    let w = disp_w;
                    let h = w / img_aspect;
                    (w, h)
                };
                let x = bounds.origin.x.as_f32() + (disp_w - w) * 0.5;
                let y = bounds.origin.y.as_f32() + (disp_h - h) * 0.5;
                let out = Bounds {
                    origin: point(px(x), px(y)),
                    size: size(px(w), px(h)),
                };
                let _ = window.paint_image(out, Corners::default(), pr.image.clone(), 0, false);
            },
        );

        let click_handler = cx.listener(move |this: &mut Self, ev: &MouseDownEvent, window, cx| {
            this.handle_page_click(idx_for_click, ev.position, window, cx);
        });

        // Eagerly render the first few pages so opening shows content quickly.
        if idx < 3 && !has_render && !self.rendering.contains(&idx) && self.edited_bytes.is_some() {
            self.render_page(idx, cx);
        }

        // Make the card itself a direct child of the scroll container with an id
        // so ScrollHandle can track its bounds. The card includes the page label
        // and the fixed-size white "paper" with the canvas inside.
        v_flex()
            .id(("pdf-page", idx))
            .w_full()
            .gap_1()
            .py_1()
            .child(
                div()
                    .text_sm()
                    .text_color(cx.theme().muted_foreground)
                    .w_full()
                    .text_center()
                    .child(format!("Page {}", idx + 1)),
            )
            .child(
                div()
                    .w(px(800.))
                    .h(px(1130.))
                    .mx_auto()
                    .border_1()
                    .border_color(cx.theme().border)
                    .rounded(px(4.))
                    .bg(white())
                    .shadow_md()
                    .child(canvas.size_full())
                    .when(has_render, |d| {
                        d.on_mouse_down(MouseButton::Left, click_handler)
                    }),
            )
            .into_any_element()
    }

    fn notify_page_visible(&mut self, cx: &mut Context<Self>, idx: u32) {
        // Called from each page's canvas prepaint. Just schedule a render of this
        // page if needed. Virtualization (pruning) happens when scroll wheel fires.
        if !self.page_cache.contains_key(&idx) && !self.rendering.contains(&idx) {
            self.render_page(idx, cx);
        }
    }

    fn rotate_item(
        label: &'static str,
        ent: WeakEntity<PdfPanel>,
        delta: i32,
    ) -> impl IntoElement {
        div()
            .w_full()
            .h(px(26.))
            .px(px(8.))
            .flex()
            .items_center()
            .text_size(px(12.))
            .rounded(px(4.))
            .text_color(gpui::hsla(0.0, 0.0, 0.4, 1.0))
            .child(label.to_string())
            .on_mouse_down(
                MouseButton::Left,
                move |_e, window, cx: &mut App| {
                    let _ = ent.update(cx, |this, cx| this.rotate_current(cx, delta));
                    window.refresh();
                },
            )
    }
}
