//! PDF Viewer panel — opens PDF files, renders pages using bundled Pdfium.
//! Pdfium native library is extracted at compile time from assets/lib/.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _};
use pdfium_render::prelude::*;
use std::path::PathBuf;

pub struct PdfPanel {
    file_path: Option<PathBuf>,
    pdfium_available: bool,
    page_count: Option<u32>,
    current_page: u32,
    _subs: Vec<Subscription>,
}

impl PdfPanel {
    pub fn new(_window: &mut Window, _cx: &mut Context<Self>) -> Self {
        // Bind to Pdfium - try the path adjacent to the executable first
        // (where build.rs copied it), fall back to system search
        let pdfium_available = Self::bind_pdfium().is_ok();

        Self {
            file_path: None,
            pdfium_available,
            page_count: None,
            current_page: 0,
            _subs: Vec::new(),
        }
    }

    /// Bind to the Pdfium library, searching several locations:
    /// 1. The directory adjacent to the executable (this is the build.rs copied, signed location)
    /// 2. The directory containing the compiled-in OUT_DIR
    /// 3. Default system search paths
    fn bind_pdfium() -> Result<Pdfium, PdfiumError> {
        let lib_name = Pdfium::pdfium_platform_library_name();

        // First, try executable directory (signed by build.rs)
        if let Ok(exe_path) = std::env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                let local = exe_dir.join(&lib_name);
                if local.exists() {
                    let bindings = Pdfium::bind_to_library(local.to_string_lossy().as_ref())?;
                    return Ok(Pdfium::new(bindings));
                }
            }
        }

        // Then try OUT_DIR from build time
        let out_dir = env!("OUT_DIR");
        let out_lib_path = PathBuf::from(out_dir).join("pdfium").join("lib").join(&lib_name);
        if out_lib_path.exists() {
            let bindings = Pdfium::bind_to_library(out_lib_path.to_string_lossy().as_ref())?;
            return Ok(Pdfium::new(bindings));
        }

        // Fall back to system search
        let bindings = Pdfium::bind_to_system_library()?;
        Ok(Pdfium::new(bindings))
    }

    pub fn open_pdf_from_title(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_pdf(window, cx);
    }

    fn open_pdf(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let file = rfd::FileDialog::new()
            .add_filter("PDF Files", &["pdf"])
            .set_title("Open PDF File")
            .pick_file();

        if let Some(path) = file {
            if let Ok(pdfium) = Self::bind_pdfium() {
                if let Ok(doc) = pdfium.load_pdf_from_file(&path, None) {
                    self.page_count = Some(doc.pages().len() as u32);
                    self.file_path = Some(path);
                    self.current_page = 0;
                }
            }
            cx.notify();
        }
    }

    fn prev_page(&mut self, cx: &mut Context<Self>) {
        if self.current_page > 0 {
            self.current_page -= 1;
            cx.notify();
        }
    }

    fn next_page(&mut self, cx: &mut Context<Self>) {
        if let Some(total) = self.page_count {
            if self.current_page + 1 < total {
                self.current_page += 1;
                cx.notify();
            }
        }
    }
}

impl Render for PdfPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        v_flex()
            .size_full()
            .bg(theme.background)
            .gap_3()
            .p_4()
            // Toolbar
            .child(
                h_flex()
                    .gap_2()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .when(self.pdfium_available, |f| {
                                f.child(
                                    Button::new("pdf-open")
                                        .label(rust_i18n::t!("pdf.open_btn").to_string())
                                        .icon(IconName::File)
                                        .small()
                                        .primary()
                                        .on_click(cx.listener(|this, _, window, cx| this.open_pdf(window, cx))),
                                )
                            }),
                    )
                    .when(self.file_path.is_some() && self.page_count.is_some(), |flex| {
                        flex.child(
                            h_flex()
                                .gap_1()
                                .items_center()
                                .child(
                                    Button::new("pdf-prev")
                                        .icon(IconName::ChevronLeft)
                                        .small()
                                        .ghost()
                                        .disabled(self.current_page == 0)
                                        .on_click(cx.listener(|this, _, _, cx| this.prev_page(cx))),
                                )
                                .child(
                                    div()
                                        .text_sm()
                                        .child(format!("{} / {}", self.current_page + 1, self.page_count.unwrap())),
                                )
                                .child(
                                    Button::new("pdf-next")
                                        .icon(IconName::ChevronRight)
                                        .small()
                                        .ghost()
                                        .disabled(self.current_page + 1 >= self.page_count.unwrap_or(0))
                                        .on_click(cx.listener(|this, _, _, cx| this.next_page(cx))),
                                ),
                        )
                    })
                    .when_some(self.file_path.as_ref(), |flex, path| {
                        flex.child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child(path.file_name()
                                    .map(|n| n.to_string_lossy().to_string())
                                    .unwrap_or_default()),
                        )
                    }),
            )
            // Main content
            .child({
                if !self.pdfium_available {
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
                                .child("Pdfium not available. PDF viewing/editing is disabled."),
                        )
                } else if self.file_path.is_none() {
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
                } else {
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
                                .text_color(theme.foreground)
                                .child(format!("Page {} of {}", self.current_page + 1, self.page_count.unwrap_or(0))),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("PDF loaded - page rendering will be displayed here"),
                        )
                }
            })
    }
}
