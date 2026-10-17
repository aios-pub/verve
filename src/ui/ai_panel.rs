//! The floating AI assistant panel — a draggable, resizable overlay card.
//!
//! Rendered via `deferred` from the app root when open; the geometry and
//! open flag persist in `layout.json` (`ai_float` / `ai_float_open`).

use std::io::Cursor;
use std::path::Path;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState, Textarea, TextareaState};
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::{
    ActiveTheme, Disableable as _, IconName, Sizable as _, button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::ai::agent::{AgentSession, Attachment, Phase, PlanDecision, TranscriptItem};
use crate::ai::config::{self, AgentMode};
use crate::ai::style;
use crate::state::AppState;
use crate::state::persistence;

/// Global weak handle so the title bar / import cards can open the panel.
pub struct AiPanelGlobal(pub gpui::WeakEntity<AiPanel>);
impl gpui::Global for AiPanelGlobal {}

/// Open the floating panel from anywhere; optionally preselect a skill and
/// prefill the composer (applied at next render, where a Window is at hand).
pub fn open_ai_panel(cx: &mut App, skill: Option<&str>, prefill: Option<String>) {
    let weak = cx.global::<AiPanelGlobal>().0.clone();
    let Some(panel) = weak.upgrade() else { return };
    panel.update(cx, |p, cx| {
        p.open(cx);
        if let Some(s) = skill {
            p.session.update(cx, |sess, cx| sess.set_skill(s, cx));
        }
        if let Some(text) = prefill {
            p.pending_prefill = Some(text);
        }
    });
}

/// Which resize grip is active.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ResizeAxis {
    /// Left edge: width + x.
    Left,
    /// Top edge: height + y.
    Top,
    /// Right edge: width only.
    Right,
    /// Bottom edge: height only.
    Bottom,
    /// Bottom-right corner: both.
    Corner,
}

// --- attachment loading ----------------------------------------------------

const IMAGE_EXTS: [&str; 5] = ["png", "jpg", "jpeg", "webp", "gif"];
const TEXT_EXTS: [&str; 4] = ["txt", "md", "json", "csv"];
/// Documents converted to Markdown via the `anydoc` crate.
const DOC_EXTS: [&str; 18] = [
    "pdf", "doc", "docx", "docm", "odt", "rtf", "epub", "xls", "xlsx", "xlsm", "xlsb", "ods",
    "ppt", "pptx", "pptm", "ppsx", "ppsm", "odp",
];
const MAX_TEXT_BYTES: u64 = 200 * 1024;
const MAX_IMAGE_EDGE: u32 = 1536;
/// Converted-document cap (chars) fed to the model.
const MAX_DOC_CHARS: usize = 120_000;

/// Truncate by chars with a visible note (AGENTS.md-safe: never slices a
/// multi-byte char in half).
fn cap_chars(content: String, max: usize, note: &str) -> String {
    if content.chars().count() <= max {
        return content;
    }
    let mut out: String = content.chars().take(max).collect();
    out.push_str(note);
    out
}

/// Build an attachment from a file: images are decoded, downscaled and
/// base64-encoded as data URLs (model vision); plain text is read verbatim;
/// office/pdf documents are converted to Markdown with `anydoc`.
pub fn load_attachment(path: &Path) -> Result<Attachment, String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "附件".to_string());
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    if IMAGE_EXTS.contains(&ext.as_str()) {
        let img = image::ImageReader::open(path)
            .map_err(|e| format!("打开图片失败：{e}"))?
            .decode()
            .map_err(|e| format!("解码图片失败：{e}"))?;
        let img = if img.width() > MAX_IMAGE_EDGE || img.height() > MAX_IMAGE_EDGE {
            img.thumbnail(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE)
        } else {
            img
        };
        let mut buf = Vec::new();
        // Re-encode as PNG: lossless, mime unambiguous across vendors.
        img.write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .map_err(|e| format!("编码图片失败：{e}"))?;
        use base64::Engine as _;
        let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
        return Ok(Attachment::Image {
            name,
            data_url: format!("data:image/png;base64,{b64}"),
        });
    }
    if TEXT_EXTS.contains(&ext.as_str()) {
        let meta = path.metadata().map_err(|e| format!("读取文件信息失败：{e}"))?;
        if meta.len() > MAX_TEXT_BYTES {
            return Err(format!("文本文件超过 {}KB 上限", MAX_TEXT_BYTES / 1024));
        }
        let content = std::fs::read_to_string(path).map_err(|e| format!("读取失败：{e}"))?;
        return Ok(Attachment::TextFile { name, content });
    }
    if DOC_EXTS.contains(&ext.as_str()) {
        let md = anydoc::to_markdown(path).map_err(|e| format!("文档转换失败：{e}"))?;
        return Ok(Attachment::TextFile {
            name,
            content: cap_chars(md, MAX_DOC_CHARS, "\n…（文档过长已截断）"),
        });
    }
    Err(format!(
        "不支持的文件类型 .{ext}（支持：图片 png/jpg/webp/gif；文本 txt/md/json/csv；文档 pdf/doc/docx/xls/xlsx/ppt/pptx/odt/rtf/epub）"
    ))
}

/// Clipboard image → vision attachment (downscaled PNG data URL).
fn clipboard_image_attachment(img: &gpui::Image) -> Option<Attachment> {
    let decoded = image::load_from_memory(&img.bytes).ok()?;
    let decoded = if decoded.width() > MAX_IMAGE_EDGE || decoded.height() > MAX_IMAGE_EDGE {
        decoded.thumbnail(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE)
    } else {
        decoded
    };
    let mut buf = Vec::new();
    decoded
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .ok()?;
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&buf);
    let name = format!("剪贴板图片-{}", img.id);
    Some(Attachment::Image {
        name,
        data_url: format!("data:image/png;base64,{b64}"),
    })
}

/// Case-insensitive substring find returning a byte index valid in `hay`
/// (needle is ASCII so matches land on char boundaries).
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() || h.len() < n.len() {
        return None;
    }
    (0..=h.len() - n.len()).find(|&i| h[i..i + n.len()].eq_ignore_ascii_case(n))
}

/// Crude HTML → readable text for online docs: drops script/style, keeps
/// table cells pipe-separated (API docs live in tables), decodes common
/// entities. Deliberately dependency-free.
pub fn html_to_text(html: &str) -> String {
    let mut s = html.to_string();
    for tag in ["script", "style", "noscript"] {
        while let Some(start) = find_ci(&s, &format!("<{tag}")) {
            match find_ci(&s, &format!("</{tag}>")) {
                // Unterminated block: drop the rest.
                None => {
                    s.truncate(start);
                    break;
                }
                Some(end) if end >= start => {
                    let end = end + tag.len() + 3;
                    s.replace_range(start..end, "");
                }
                Some(_) => break,
            }
        }
    }
    let mut out = String::with_capacity(s.len());
    let mut tag_buf = String::new();
    let mut in_tag = false;
    for c in s.chars() {
        if c == '<' {
            in_tag = true;
            tag_buf.clear();
            tag_buf.push('<');
            continue;
        }
        if in_tag {
            tag_buf.push(c);
            if c == '>' {
                in_tag = false;
                let t = tag_buf.to_ascii_lowercase();
                if t.starts_with("</td") || t.starts_with("</th") {
                    out.push_str(" | ");
                } else if t.starts_with("<br") || t.starts_with("<tr") || t.starts_with("<p")
                    || t.starts_with("</p") || t.starts_with("</div") || t.starts_with("</li")
                    || t.starts_with("<li") || t.starts_with("</h") || t.starts_with("</table")
                {
                    out.push('\n');
                }
            }
            continue;
        }
        out.push(c);
    }
    let out = out
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&mdash;", "—")
        .replace("&hellip;", "…");
    let lines: Vec<String> = out
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect();
    cap_chars(lines.join("\n"), 60_000, "\n…（网页内容过长已截断）")
}

/// Extract candidate online-doc URLs from a message (max 2, assets skipped).
pub fn extract_urls(text: &str) -> Vec<String> {
    const ASSET_EXTS: [&str; 12] = [
        "png", "jpg", "jpeg", "gif", "svg", "webp", "ico", "css", "js", "woff", "woff2", "ttf",
    ];
    let delims: &[char] = &[
        '，', '。', '、', '「', '」', '『', '』', '（', '）', '(', ')', '【', '】', '《', '》', '<', '>',
        ',', ';', '；', '！', '？', '"', '\'', '`', '\n', '\r', '\t',
    ];
    let mut urls: Vec<String> = Vec::new();
    for tok in text.split(|c: char| c.is_whitespace() || delims.contains(&c)) {
        let t = tok.trim_end_matches(['.', ',', ':', ';']);
        if !(t.starts_with("http://") || t.starts_with("https://")) {
            continue;
        }
        let is_asset = t
            .rsplit('.')
            .next()
            .is_some_and(|e| ASSET_EXTS.contains(&e.to_ascii_lowercase().as_str()));
        if is_asset {
            continue;
        }
        if !urls.iter().any(|u| u == t) {
            urls.push(t.to_string());
        }
        if urls.len() >= 2 {
            break;
        }
    }
    urls
}

// --- the panel -------------------------------------------------------------

pub struct AiPanel {
    pub session: Entity<AgentSession>,
    /// Composer input (multi-line, Enter sends / Shift+Enter newline).
    pub input: Entity<TextareaState>,
    pub provider_select: Entity<SelectState<Vec<String>>>,
    pub model_select: Entity<SelectState<Vec<String>>>,
    /// Session usage detail popover.
    pub usage_open: bool,
    pub open: bool,
    /// Minimized-to-edge state (pill instead of the full card).
    pub minimized: bool,
    pub x: Pixels,
    pub y: Pixels,
    pub w: Pixels,
    pub h: Pixels,
    /// Cursor offset within the panel while title-bar dragging.
    drag_anchor: Option<Point<Pixels>>,
    /// (axis, cursor, x, y, w, h) snapshot when a resize grip was pressed.
    resize_start: Option<(ResizeAxis, Point<Pixels>, Pixels, Pixels, Pixels, Pixels)>,
    last_persist: Option<std::time::Instant>,
    /// Set by the Enter-to-send subscription (no Window there); the composer
    /// is cleared at the top of render where a Window is available.
    pending_clear_input: bool,
    /// Composer prefill requested from a context without a Window.
    pending_prefill: Option<String>,
    /// Refresh the provider dropdown from settings at next render (Window
    /// is needed for `SelectState::set_items`).
    pending_provider_refresh: bool,
    /// Online-doc fetch in progress for the pending message.
    fetching_web: bool,
    /// Refresh the model dropdown at next render (needs a Window).
    pending_model_refresh: bool,
    /// Last-seen mtime of ai_providers.json — detects edits made in the
    /// provider dialog (separate window) so dropdowns refresh immediately.
    providers_mtime: Option<std::time::SystemTime>,
    /// Transient "已附加 …" confirmation shown after a paste, with its
    /// issue time (auto-hides after ~3s).
    paste_flash: Option<(String, std::time::Instant)>,
    /// Keeps the app-level keystroke observer alive (paste interception —
    /// see `new` for why neither action listeners nor key handlers work).
    paste_observer: gpui::Subscription,
    /// Per-plan-card adjustment-note inputs, keyed by transcript item index
    /// (created lazily in render where a Window is available).
    plan_notes: std::collections::HashMap<usize, Entity<InputState>>,
    /// Transcript auto-follow handle while the agent is producing output.
    transcript_scroll: gpui::ScrollHandle,
    /// Whether the transcript sticks to the bottom during output. Any
    /// explicit scroll-up gesture detaches it; sending a new message or
    /// pressing the "回到最新" chip reattaches.
    auto_follow: bool,
    _subs: Vec<Subscription>,
}

impl AiPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let session = cx.new(|cx| AgentSession::new(state.clone(), window, cx));
        let input = cx.new(|cx| {
            TextareaState::new(window, cx)
                .auto_grow(3, 12)
                // Plain Enter submits (PressEnter fires without inserting a
                // newline); Shift+Enter inserts the newline.
                .submit_on_enter(true)
                .placeholder("描述需求、粘贴 cURL / 文档 / 接口文本…（Enter 发送，Shift+Enter 换行）")
        });
        let settings = config::load_ai_settings();
        let provider_options: Vec<String> = settings
            .providers
            .iter()
            .filter(|p| p.enabled)
            .map(|p| format!("{} · {}", p.name, p.model))
            .collect();
        let active_ix = settings
            .providers
            .iter()
            .filter(|p| p.enabled)
            .position(|p| Some(p.id.as_str()) == settings.active_provider_id.as_deref())
            .unwrap_or(0);
        let provider_select = cx.new(|cx| {
            SelectState::new(
                provider_options,
                Some(gpui_component::IndexPath::new(active_ix)),
                window,
                cx,
            )
        });
        let model_select = cx.new(|cx| {
            SelectState::new(
                Vec::<String>::new(),
                Some(gpui_component::IndexPath::new(0)),
                window,
                cx,
            )
        });

        // Paste interception via the app-level keystroke OBSERVER: when the
        // input's own cmd-v binding matches, gpui dispatches the Paste
        // action and returns before ANY key listener (capture or bubble) or
        // ancestor action listener can run — observers, however, are invoked
        // after the action, so the text is already inserted when we add the
        // file/image attachments.
        let paste_observer = cx.observe_keystrokes(|this, event, window, cx| {
            let ks = &event.keystroke;
            let is_paste = ks.key.eq_ignore_ascii_case("v")
                && (ks.modifiers.platform || ks.modifiers.control)
                && !ks.modifiers.shift
                && !ks.modifiers.alt;
            if !is_paste || !this.open || this.minimized {
                return;
            }
            // Only when OUR composer input has focus (observers are global).
            use gpui::Focusable as _;
            let input_focus = this.input.read(cx).focus_handle(cx);
            if !input_focus.is_focused(window) {
                return;
            }
            this.handle_clipboard_paste(window, cx);
        });
        let subs = vec![
            cx.subscribe(&input, Self::on_input_event),
            cx.subscribe(&provider_select, Self::on_provider_change),
            cx.subscribe(&model_select, Self::on_model_change),
        ];

        let (g, open) = persistence::load_ai_float();
        Self {
            session,
            input,
            provider_select,
            model_select,
            usage_open: false,
            open,
            minimized: false,
            x: px(g[0]),
            y: px(g[1]),
            w: px(g[2]),
            h: px(g[3]),
            drag_anchor: None,
            resize_start: None,
            last_persist: None,
            pending_clear_input: false,
            pending_prefill: None,
            pending_provider_refresh: false,
            fetching_web: false,
            pending_model_refresh: false,
            providers_mtime: None,
            paste_flash: None,
            paste_observer,
            plan_notes: std::collections::HashMap::new(),
            transcript_scroll: gpui::ScrollHandle::new(),
            auto_follow: true,
            _subs: subs,
        }
    }

    pub fn open(&mut self, cx: &mut Context<Self>) {
        self.open = true;
        // Providers may have been added/renamed in the settings window since
        // this panel was built — refresh the dropdown.
        self.pending_provider_refresh = true;
        self.persist(true, cx);
        cx.notify();
    }

    /// Rebuild the provider dropdown options + selection from settings.
    fn refresh_provider_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = config::load_ai_settings();
        let enabled: Vec<&config::AiProviderConfig> =
            settings.providers.iter().filter(|p| p.enabled).collect();
        let options: Vec<String> = enabled.iter().map(|p| p.name.clone()).collect();
        let active_ix = enabled
            .iter()
            .position(|p| Some(p.id.as_str()) == settings.active_provider_id.as_deref())
            .unwrap_or(0);
        self.provider_select.update(cx, |sel, cx| {
            sel.set_items(options, window, cx);
            sel.set_selected_index(Some(gpui_component::IndexPath::new(active_ix)), window, cx);
        });
    }

    /// Rebuild the model dropdown from the active provider's model list.
    fn refresh_model_select(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = config::load_ai_settings();
        let Some(p) = settings.active_provider() else {
            return;
        };
        let models: Vec<String> = if p.models.is_empty() {
            vec![p.model.clone()]
        } else {
            p.models.clone()
        };
        let active_ix = models.iter().position(|m| *m == p.model);
        self.model_select.update(cx, |sel, cx| {
            sel.set_items(models, window, cx);
            sel.set_selected_index(
                active_ix.map(gpui_component::IndexPath::new),
                window,
                cx,
            );
        });
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.open = false;
        self.session.update(cx, |s, cx| {
            if s.running {
                s.stop(cx);
            }
        });
        self.persist(true, cx);
        cx.notify();
    }

    pub fn toggle(&mut self, cx: &mut Context<Self>) {
        if self.open {
            self.close(cx);
        } else {
            self.open(cx);
        }
    }

    fn persist(&mut self, force: bool, cx: &mut Context<Self>) {
        let now = std::time::Instant::now();
        let due = force
            || self
                .last_persist
                .map(|t| now.duration_since(t).as_millis() > 250)
                .unwrap_or(true);
        if due {
            persistence::save_ai_float(
                [self.x.as_f32(), self.y.as_f32(), self.w.as_f32(), self.h.as_f32()],
                self.open,
            );
            self.last_persist = Some(now);
        }
        let _ = cx;
    }

    fn on_input_event(
        &mut self,
        _src: Entity<TextareaState>,
        ev: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if let InputEvent::PressEnter { shift: false, .. } = ev {
            // Input clearing is handled in dispatch_send (also fixes the
            // send button never clearing the composer).
            self.send(cx);
        }
    }

    fn on_provider_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let Some(ix) = src.read(cx).selected_index(cx) else {
            return;
        };
        let settings = config::load_ai_settings();
        let enabled: Vec<&config::AiProviderConfig> =
            settings.providers.iter().filter(|p| p.enabled).collect();
        let Some(p) = enabled.get(ix.row) else { return };
        let id = p.id.clone();
        let mut updated = settings;
        updated.active_provider_id = Some(id);
        let _ = config::save_ai_settings(&updated);
        // Model list follows the provider.
        self.pending_model_refresh = true;
        cx.notify();
    }

    /// Switch the active provider's model — the agent loads the provider
    /// fresh on every turn, so this takes effect on the next request
    /// (mid-conversation friendly).
    fn on_model_change(
        &mut self,
        src: Entity<SelectState<Vec<String>>>,
        _ev: &SelectEvent<Vec<String>>,
        cx: &mut Context<Self>,
    ) {
        let Some(model) = src.read(cx).selected_value().cloned() else {
            return;
        };
        let Some(pid) = config::load_ai_settings()
            .active_provider()
            .map(|p| p.id.clone())
        else {
            return;
        };
        config::set_active_model(&pid, &model);
        log::info!("AI 模型切换为 {model}（下一轮生效）");
        cx.notify();
    }


    fn send(&mut self, cx: &mut Context<Self>) {
        if self.fetching_web {
            return;
        }
        let text = self.input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        // Sending implies wanting to watch the answer again.
        self.auto_follow = true;
        let urls = extract_urls(&text);
        if urls.is_empty() {
            self.dispatch_send(text, cx);
            return;
        }
        // Online-document scenario: fetch referenced pages first and attach
        // their text so the model can generate API docs from them.
        self.fetching_web = true;
        cx.notify();
        let client = cx.http_client();
        let entity = cx.entity();
        let _fetch_task = cx.spawn(async move |_this, cx| {
            let mut fetched: Vec<Attachment> = Vec::new();
            let mut failures: Vec<String> = Vec::new();
            for url in &urls {
                match fetch_web_doc(&client, url).await {
                    Ok(content) => fetched.push(Attachment::TextFile {
                        name: format!("🌐 {url}"),
                        content,
                    }),
                    Err(e) => failures.push(format!("{url}：{e}")),
                }
            }
            let _ = entity.update(cx, |this: &mut AiPanel, cx| {
                this.fetching_web = false;
                for a in fetched {
                    this.session.update(cx, |s, cx| s.add_attachment(a, cx));
                }
                for f in failures {
                    this.session.update(cx, |s, cx| {
                        s.items.push(TranscriptItem::Note(format!(
                            "在线文档读取失败 {f}，已按原文发送"
                        )));
                        cx.notify();
                    });
                }
                this.dispatch_send(text, cx);
            });
        });
    }

    /// Actually hand the message to the agent (input clears on next render —
    /// `set_value` needs a Window).
    fn dispatch_send(&mut self, text: String, cx: &mut Context<Self>) {
        self.pending_clear_input = true;
        cx.notify();
        self.session.update(cx, |s, cx| s.send(text, cx));
    }

    /// Paste handler on the composer container: runs AFTER the input's own
    /// text paste (it does not stop propagation), and turns clipboard
    /// files / screenshots into attachments.
    fn handle_clipboard_paste(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(item) = cx.read_from_clipboard() else {
            return;
        };
        let mut added: Vec<String> = Vec::new();
        let mut junk_path: Option<String> = None;
        for entry in &item.entries {
            match entry {
                gpui::ClipboardEntry::ExternalPaths(paths) => {
                    // macOS puts the path string on the pasteboard too; the
                    // input's own paste has already inserted it as text by
                    // observer time — clear it below.
                    if junk_path.is_none()
                        && let Some(gpui::ClipboardEntry::String(s)) = item.entries.last()
                    {
                        junk_path = Some(s.text.clone());
                    }
                    for path in paths.paths().iter().take(3) {
                        match load_attachment(path) {
                            Ok(a) => {
                                added.push(a.name().to_string());
                                this_attach(self, a, cx);
                            }
                            Err(e) => {
                                this_note(
                                    self,
                                    format!("粘贴的文件无法加载（{}）：{e}", path.display()),
                                    cx,
                                );
                            }
                        }
                    }
                }
                gpui::ClipboardEntry::Image(img) => match clipboard_image_attachment(img) {
                    Some(a) => {
                        added.push(a.name().to_string());
                        this_attach(self, a, cx);
                    }
                    None => this_note(self, "剪贴板图片解码失败".into(), cx),
                },
                gpui::ClipboardEntry::String(_) => {}
            }
        }
        if !added.is_empty() {
            // The file-path text the input inserted alongside our chip.
            if let Some(junk) = junk_path
                && self.input.read(cx).value().trim() == junk.trim()
            {
                self.input.update(cx, |st, cx| st.set_value("", window, cx));
            }
            // Visible confirmation that the paste landed.
            let msg = format!("✅ 已附加：{}", added.join("、"));
            self.paste_flash = Some((msg, std::time::Instant::now()));
            let entity = cx.entity();
            cx.spawn(async move |_this, cx| {
                smol::Timer::after(std::time::Duration::from_secs(3)).await;
                let _ = entity.update(cx, |this: &mut AiPanel, cx| {
                    this.paste_flash = None;
                    cx.notify();
                });
            })
            .detach();
            cx.notify();
        }
    }

    /// Open the AI section of the settings window (own OS window, like the
    /// environments manager — doesn't need the app root).
    fn open_ai_settings(&mut self, cx: &mut Context<Self>) {
        let state = self.session.read(cx).state.clone();
        cx.defer(move |cx| {
            let bounds = crate::ui::app::centered_window_bounds(px(920.), px(640.), cx);
            let mut options = gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                window_min_size: Some(gpui::size(px(700.), px(480.))),
                titlebar: None,
                is_minimizable: true,
                ..Default::default()
            };
            crate::ui::app::apply_window_chrome_fixes(&mut options, true);
            let _ = cx.open_window(options, |window, cx| {
                let view = cx.new(|cx| {
                    crate::ui::settings_window::SettingsWindow::new(
                        state.clone(),
                        crate::ui::settings_window::SettingsKind::Ai,
                        window,
                        cx,
                    )
                });
                cx.new(|cx| {
                    gpui_component::Root::new(view, window, cx).bg(cx.theme().background)
                })
            });
        });
    }

    fn pick_attachment(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("选择图片 / 文档（Word·PDF·Excel·PPT）/ 文本文件".into()),
        });
        let entity = cx.entity();
        cx.spawn(async move |_this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await
                && let Some(path) = paths.first().cloned()
            {
                let shown = path.display().to_string();
                // Conversion (anydoc / image decode) can take a while for
                // large documents — keep it off the UI thread.
                let att = smol::unblock(move || load_attachment(&path)).await;
                let _ = entity.update(cx, |this: &mut AiPanel, cx| {
                    match att {
                        Ok(a) => {
                            this.session.update(cx, |s, cx| s.add_attachment(a, cx));
                        }
                        Err(e) => {
                            this.session.update(cx, |s, cx| {
                                s.items.push(TranscriptItem::Error(format!(
                                    "附件加载失败（{shown}）：{e}"
                                )));
                                cx.notify();
                            });
                        }
                    }
                    cx.notify();
                });
            }
        })
        .detach();
    }


    /// Lazily create (and reuse) the adjustment-note input for a plan card.
    fn ensure_plan_note(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<InputState> {
        if let Some(n) = self.plan_notes.get(&ix) {
            return n.clone();
        }
        let note = cx.new(|cx| {
            InputState::new(window, cx)
                .placeholder("想怎么调整？写下意见后点「让AI调整」（留空则让 AI 先询问你）")
        });
        self.plan_notes.insert(ix, note.clone());
        note
    }

    /// The minimized state: an icon badge snapped to the window's right edge.
    /// Click restores the full floating card at its previous geometry.
    fn render_edge_pill(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let running = self.session.read(cx).running;
        let vh = window.viewport_size().height;
        let top = self.y.clamp(px(40.), (vh - px(72.)).max(px(40.)));
        let fg = if running {
            theme.accent_foreground
        } else {
            theme.foreground
        };
        gpui::deferred(
            div()
                .id("ai-float-pill")
                .absolute()
                .right_0()
                .top(top)
                .w(px(34.))
                .h(px(38.))
                .flex()
                .items_center()
                .justify_center()
                .rounded_tl_lg()
                .rounded_bl_lg()
                .border_1()
                .border_r_0()
                .border_color(if running { theme.accent } else { theme.border })
                .bg(if running {
                    theme.accent.opacity(0.85)
                } else {
                    theme.background
                })
                .shadow_lg()
                .cursor_pointer()
                .hover(|d| d.border_color(theme.accent))
                .child(
                    div()
                        .size(px(22.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_color(fg)
                        .child(IconName::Bot),
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.minimized = false;
                    cx.notify();
                })),
        )
        .into_any_element()
    }
    fn render_title(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let running = self.session.read(cx).running;
        let mode = self.session.read(cx).mode;

        let bar = h_flex()
            .id("ai-float-title")
            .h(px(34.))
            .px_2()
            .gap_2()
            .items_center()
            .border_b_1()
            .border_color(theme.border)
            .bg(theme.muted)
            .flex_shrink_0()
            .cursor_grab()
            .child(div().text_sm().font_weight(FontWeight::SEMIBOLD).child("✨ AI 助手"))
            // Mode segmented control.
            .child(
                h_flex()
                    .gap_1()
                    .child(
                        Button::new("ai-mode-plan")
                            .xsmall()
                            .when(mode == AgentMode::Plan, |b| b.primary())
                            .when(mode != AgentMode::Plan, |b| b.ghost())
                            .label("计划")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.session.update(cx, |s, cx| s.set_mode(AgentMode::Plan, cx));
                            })),
                    )
                    .child(
                        Button::new("ai-mode-full")
                            .xsmall()
                            .when(mode == AgentMode::FullAccess, |b| b.primary())
                            .when(mode != AgentMode::FullAccess, |b| b.ghost())
                            .label("完全访问")
                            .tooltip("完全访问模式：写操作立即执行（git 自动提交可回滚）")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.session
                                    .update(cx, |s, cx| s.set_mode(AgentMode::FullAccess, cx));
                            })),
                    ),
            )
            .child(div().flex_1())
            // Provider / model dropdowns live in the title bar (skills are
            // switched autonomously by the model via the use_skill tool);
            // stop_propagation keeps clicks from starting a panel drag.
            .child(
                h_flex()
                    .id("ai-title-selects")
                    .gap_1()
                    .min_w_0()
                    .overflow_hidden()
                    .on_mouse_down(
                        MouseButton::Left,
                        |_, _, cx| cx.stop_propagation(),
                    )
                    .child(
                        div()
                            .w(px(92.))
                            .min_w_0()
                            .child(Select::new(&self.provider_select).small()),
                    )
                    .child(
                        div()
                            .w(px(150.))
                            .min_w_0()
                            .child(Select::new(&self.model_select).small()),
                    )
            )
            .child(
                Button::new("ai-minimize")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Minus)
                    .tooltip("最小化到边缘")
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.minimized = true;
                        cx.notify();
                    })),
            )
            .child(
                Button::new("ai-clear")
                    .ghost()
                    .xsmall()
                    .icon(IconName::Close)
                    .tooltip("清空会话")
                    .disabled(running)
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.session.update(cx, |s, cx| s.clear(cx));
                    })),
            )
            .child(
                Button::new("ai-close")
                    .ghost()
                    .xsmall()
                    .icon(IconName::CircleX)
                    .tooltip("关闭")
                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
            )
            // Drag to move: record the grab offset; the window-level
            // tracker registered in render() follows the cursor.
            .on_mouse_down(MouseButton::Left, cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                this.drag_anchor = Some(point(
                    ev.position.x - this.x,
                    ev.position.y - this.y,
                ));
                cx.notify();
            }));
        let _ = window;
        bar
    }

    fn render_transcript(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let provider_missing = config::load_ai_settings().active_provider().is_none();
        let status_zh = self.session.read(cx).status_zh;
        // Extract owned view models under the read borrow, then render with a
        // free `cx` (plan-card buttons need cx.listener).
        let views: Vec<ItemView> = {
            let s = self.session.read(cx);
            s.items
                .iter()
                .map(|item| ItemView::from_item(item))
                .collect()
        };
        let status_idx = self.session.read(cx).status_idx;
        // Auto-follow: pin to the bottom while output streams — but detach
        // the moment the user scrolls up so history stays readable.
        let running = self.session.read(cx).running;
        if running && self.auto_follow {
            self.transcript_scroll.scroll_to_bottom();
        }
        let show_jump = !self.auto_follow;
        let scroll = self.transcript_scroll.clone();
        // Relative wrapper so the floating chip overlays (not scrolls with)
        // the transcript. Must be a flex column: a plain `div()` is
        // display:block, where the scroll child's `flex_1` is ignored and it
        // sizes to its full content height — painting the transcript behind
        // the status bar and composer below.
        v_flex()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
        div()
            .id("ai-transcript")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .track_scroll(&self.transcript_scroll)
            .on_scroll_wheel(cx.listener(
                |this: &mut Self, ev: &ScrollWheelEvent, _window, cx| {
                    let dy = match &ev.delta {
                        gpui::ScrollDelta::Pixels(p) => p.y.as_f32(),
                        gpui::ScrollDelta::Lines(l) => l.y * 20.0,
                    };
                    // Scrolling toward earlier content detaches follow.
                    if dy > 0.0 && this.auto_follow {
                        this.auto_follow = false;
                        cx.notify();
                    }
                },
            ))
            .child(
                // Inner wrapper keeps min-height = viewport so content anchors
                // to the bottom when short, while remaining fully scrollable
                // when tall (plain `justify_end` on the scroll container clips
                // the top overflow out of reach).
                v_flex()
                    .w_full()
                    .min_h_full()
                    .justify_end()
                    .p_2()
                    .gap_2()
                    // Quick-action card when no provider is configured.
                    .when(provider_missing, |col| {
                        col.child(
                            v_flex()
                                .id("ai-no-provider")
                        .p_3()
                        .gap_2()
                        .items_center()
                        .rounded_md()
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.muted.opacity(0.5))
                        .child(
                            div()
                                .text_sm()
                                .text_color(theme.muted_foreground)
                                .child("尚未配置 AI 供应商 — 添加后即可对话、提取接口与整理目录。"),
                        )
                        .child(
                            Button::new("ai-open-settings")
                                .primary()
                                .small()
                                .icon(IconName::Settings)
                                .label("打开 AI 智能体设置")
                                .on_click(cx.listener(|this, _, _, cx| this.open_ai_settings(cx))),
                        ),
                )
            })
            // First-run hint when there is nothing else to show.
            .when(views.is_empty() && !provider_missing, |col| {
                col.child(
                    div()
                        .text_sm()
                        .text_color(theme.muted_foreground)
                        .child("描述需求、粘贴 cURL / 文档 / 截图开始；Enter 发送。"),
                )
            })
                    .children(views.into_iter().enumerate().map(|(ix, view)| {
                        // Plan cards get their own adjustment-note input.
                        let plan_note = match &view {
                            ItemView::Plan { pending, .. } if *pending => {
                                Some(self.ensure_plan_note(ix, window, cx))
                            }
                            _ => None,
                        };
                        render_item(ix, &view, status_zh, status_idx, &theme, plan_note, window, cx)
                    })),
            ),
            )
            // Floating "back to latest" chip overlays the transcript once
            // the user scrolled away from the bottom.
            .when(show_jump, |wrapper| {
                wrapper.child(
                    div()
                        .absolute()
                        .bottom_2()
                        .right_2()
                        .id("ai-jump-latest")
                        .px_2()
                        .py_1()
                        .rounded_full()
                        .border_1()
                        .border_color(theme.accent)
                        .bg(theme.background.opacity(0.9))
                        .shadow_sm()
                        .cursor_pointer()
                        .text_xs()
                        .text_color(theme.accent_foreground)
                        .child(if running || self.fetching_web {
                            "↓ 回到最新（输出中）"
                        } else {
                            "↓ 回到最新"
                        })
                        .on_click(cx.listener({
                            let scroll = scroll.clone();
                            move |this, _, _, cx| {
                                this.auto_follow = true;
                                scroll.scroll_to_bottom();
                                cx.notify();
                            }
                        })),
                )
            })
    }

    fn render_status(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let s = self.session.read(cx);
        let running = s.running;
        let phase = s.phase;
        let iteration = s.iteration;
        let mode = s.mode;
        let retry = s.retry;
        let word = if running {
            style::status_word(s.status_zh, s.status_idx)
        } else {
            ""
        };
        let prompt_tok = s.usage_total.prompt_tokens;
        let completion_tok = s.usage_total.completion_tokens;
        // Session cost uses the current provider pricing.
        let (cost, currency) = {
            let provider = config::load_ai_settings().active_provider().cloned();
            match provider.as_ref().and_then(|p| p.cost(prompt_tok, completion_tok)) {
                Some(c) => (
                    Some(c),
                    provider
                        .as_ref()
                        .and_then(|p| p.currency.clone())
                        .unwrap_or_default(),
                ),
                None => (None, String::new()),
            }
        };

        h_flex()
            .px_2()
            .py_1()
            .gap_2()
            .items_center()
            .border_t_1()
            .border_color(theme.border)
            .flex_shrink_0()
            .when(running, |bar| {
                bar.child(
                    div()
                        .text_xs()
                        .text_color(theme.accent_foreground)
                        .child(format!("循环 {iteration} · {}", mode_label(mode))),
                )
                // 429 rate-limit retry progress.
                .when_some(retry, |bar, r| {
                    bar.child(
                        div()
                            .id("ai-retry-chip")
                            .px_1p5()
                            .py_0p5()
                            .rounded(px(4.))
                            .bg(theme.danger.opacity(0.15))
                            .text_xs()
                            .text_color(theme.danger)
                            .child(format!(
                                "⏳ 重试中 · 第 {}/{} 次（{}s 后）",
                                r.attempt, r.max, r.wait_secs
                            )),
                    )
                })
                .when(!word.is_empty(), |bar| {
                    bar.child(div().text_xs().text_color(theme.muted_foreground).child(word))
                })
                .child(
                    Button::new("ai-stop")
                        .danger()
                        .xsmall()
                        .label("停止")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.session.update(cx, |s, cx| s.stop(cx));
                        })),
                )
            })
            .child(div().flex_1())
            .child(
                div()
                    .id("ai-usage-line")
                    .text_xs()
                    .text_color(theme.muted_foreground)
                    .cursor_pointer()
                    .child(match cost {
                        Some(c) => format!("本次会话 {prompt_tok}+{completion_tok} tok · {currency}{c:.4}"),
                        None => format!("本次会话 {prompt_tok}+{completion_tok} tok"),
                    })
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.usage_open = !this.usage_open;
                        cx.notify();
                    })),
            )
            .when(self.usage_open, |bar| {
                bar.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child(format!("（{} 轮对话）", self.session.read(cx).turn_lines.len())),
                )
            })
    }

    fn render_composer(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.pending_clear_input {
            self.pending_clear_input = false;
            self.input.update(cx, |s, cx| s.set_value("", window, cx));
        }
        if let Some(prefill) = self.pending_prefill.take() {
            let current = self.input.read(cx).value().to_string();
            let merged = format!("{prefill}{current}");
            self.input.update(cx, |s, cx| s.set_value(&merged, window, cx));
        }
        let theme = cx.theme().clone();
        let running = self.session.read(cx).running;
        let attachments: Vec<(usize, String)> = self
            .session
            .read(cx)
            .attachments
            .iter()
            .enumerate()
            .map(|(i, a)| (i, a.name().to_string()))
            .collect();

        v_flex()
            .id("ai-composer")
            .p_2()
            .gap_1()
            .border_t_1()
            .border_color(theme.border)
            .flex_shrink_0()
            // Paste interception lives in `new` (app-level keystroke
            // observer): when the input's cmd-v binding matches, gpui
            // returns before any capture/bubble key handler or ancestor
            // action listener can run — only observers still fire.
            .when(self.fetching_web, |col| {
                col.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted_foreground)
                        .child("🌐 正在读取消息中的在线文档…"),
                )
            })
            .when_some(
                self.paste_flash.as_ref().map(|(m, _)| m.clone()),
                |col, msg| {
                    col.child(
                        div()
                            .id("ai-paste-flash")
                            .px_2()
                            .py_0p5()
                            .rounded_md()
                            .bg(theme.success.opacity(0.12))
                            .text_xs()
                            .text_color(theme.success)
                            .child(msg),
                    )
                },
            )
            .when(!attachments.is_empty(), |col| {
                col.child(
                    h_flex().gap_1().flex_wrap().children(
                        attachments
                            .into_iter()
                            .map(|(ix, name)| self.render_attachment_chip(ix, name, &theme, cx)),
                    ),
                )
            })
            .child(
                h_flex()
                    .gap_1()
                    .items_end()
                    .child(
                        Button::new("ai-attach")
                            .ghost()
                            .small()
                            .icon(IconName::File)
                            .tooltip("附加图片 / Word / PDF / Excel / PPT / 文本（也可直接粘贴文件或截图）")
                            .on_click(cx.listener(|this, _, _, cx| this.pick_attachment(cx))),
                    )
                    .child(div().flex_1().min_w_0().child(Textarea::new(&self.input).small()))
                    .child(if running {
                        Button::new("ai-send-disabled")
                            .small()
                            .ghost()
                            .icon(IconName::LoaderCircle)
                            .disabled(true)
                            .into_any_element()
                    } else {
                        Button::new("ai-send")
                            .primary()
                            .small()
                            .icon(IconName::ArrowUp)
                            .tooltip("发送（Enter）")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.send(cx);
                            }))
                            .into_any_element()
                    }),
            )
    }
}

fn this_attach(panel: &mut AiPanel, a: Attachment, cx: &mut gpui::Context<AiPanel>) {
    panel.session.update(cx, |s, cx| s.add_attachment(a, cx));
    cx.notify();
}

fn this_note(panel: &mut AiPanel, msg: String, cx: &mut gpui::Context<AiPanel>) {
    panel
        .session
        .update(cx, |s, cx| s.items.push(TranscriptItem::Error(msg)));
    cx.notify();
}

/// Fetch an online document and return model-ready text (HTML → readable
/// text, tables kept pipe-separated). 15s timeout, ~800KB cap.
async fn fetch_web_doc(
    client: &std::sync::Arc<dyn http_client::HttpClient>,
    url: &str,
) -> Result<String, String> {
    use futures::AsyncReadExt as _;
    use futures::future::Either;
    let uri: http::Uri = url.parse().map_err(|e| format!("URL 无效：{e}"))?;
    let req = http::Request::builder()
        .method(http::Method::GET)
        .uri(uri)
        .header("User-Agent", "Verve/0.6 (API workbench assistant)")
        .header("Accept", "text/html,application/json,text/plain,*/*")
        .body(http_client::AsyncBody::from(Vec::new()))
        .map_err(|e| format!("构造请求失败：{e}"))?;
    let timer = smol::Timer::after(std::time::Duration::from_secs(15));
    let resp = match futures::future::select(Box::pin(client.send(req)), timer).await {
        Either::Left((resp, _)) => resp,
        Either::Right(_) => Err(anyhow::anyhow!("读取超时（15s）")),
    }
    .map_err(|e| format!("请求失败：{e}"))?;
    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }
    let content_type = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_lowercase();
    let mut body = resp.into_body();
    let mut buf: Vec<u8> = Vec::with_capacity(16_384);
    let mut chunk = [0u8; 16_384];
    loop {
        let n = body.read(&mut chunk).await.unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.len() > 800 * 1024 {
            break;
        }
    }
    let text = String::from_utf8_lossy(&buf).to_string();
    if content_type.contains("html")
        || (content_type.is_empty() && text.trim_start().starts_with('<'))
    {
        Ok(html_to_text(&text))
    } else if content_type.contains("json")
        || content_type.starts_with("text/")
        || content_type.is_empty()
    {
        Ok(text)
    } else {
        Err(format!("不支持的内容类型 {content_type}"))
    }
}

fn mode_label(mode: AgentMode) -> &'static str {
    match mode {
        AgentMode::Plan => "计划模式",
        AgentMode::FullAccess => "完全访问",
    }
}

/// Owned render model extracted from [`TranscriptItem`] (the source can't be
/// cloned out of the session because plan cards hold a oneshot sender).
enum ItemView {
    User {
        text: String,
        attachment_names: Vec<String>,
    },
    /// (step, detail, 0=pending 1=in_progress 2=completed)
    WorkPlan {
        steps: Vec<(String, String, u8)>,
    },
    Assistant {
        text: String,
        streaming: bool,
        usage: Option<crate::ai::agent::UsageLine>,
    },
    Tool {
        name: String,
        summary: String,
        ok: bool,
    },
    Plan {
        lines: Vec<(bool, String)>,
        pending: bool,
    },
    Error(String),
    Note(String),
}

/// A tiny copy affordance that copies `text` to the clipboard.
fn copy_chip(id: String, text: String, theme: &gpui_component::Theme) -> AnyElement {
    let muted = theme.muted_foreground;
    let fg = theme.foreground;
    div()
        .id(id)
        .size(px(18.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(3.))
        .cursor_pointer()
        .text_color(muted)
        .hover(move |d| d.text_color(fg))
        .child(IconName::Copy)
        .on_click(move |_, _, cx: &mut App| {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(text.clone()));
        })
        .into_any_element()
}

impl ItemView {
    fn from_item(item: &TranscriptItem) -> Self {
        match item {
            TranscriptItem::User {
                text,
                attachment_names,
            } => ItemView::User {
                text: text.clone(),
                attachment_names: attachment_names.clone(),
            },
            TranscriptItem::Assistant { text, streaming, usage } => ItemView::Assistant {
                text: text.clone(),
                streaming: *streaming,
                usage: usage.clone(),
            },
            TranscriptItem::WorkPlan { steps } => ItemView::WorkPlan {
                steps: steps
                    .iter()
                    .map(|st| {
                        let code = match st.status {
                            crate::ai::agent::PlanStatus::Pending => 0u8,
                            crate::ai::agent::PlanStatus::InProgress => 1,
                            crate::ai::agent::PlanStatus::Completed => 2,
                        };
                        (st.step.clone(), st.detail.clone(), code)
                    })
                    .collect(),
            },
            TranscriptItem::Tool { name, summary, ok } => ItemView::Tool {
                name: name.clone(),
                summary: summary.clone(),
                ok: *ok,
            },
            TranscriptItem::Plan {
                ops,
                validation,
                applied,
                decided,
                ..
            } => ItemView::Plan {
                lines: (0..ops.len())
                    .map(|oix| {
                        let r = applied
                            .as_ref()
                            .and_then(|a| a.get(oix))
                            .or_else(|| validation.get(oix));
                        match r {
                            Some(r) => {
                                let err = r.error.clone().unwrap_or_default();
                                let text = if err.is_empty() {
                                    r.summary.clone()
                                } else {
                                    format!("{} — {}", r.summary, err)
                                };
                                (r.ok, text)
                            }
                            None => (true, String::new()),
                        }
                    })
                    .collect(),
                pending: decided.is_none(),
            },
            TranscriptItem::Error(msg) => ItemView::Error(msg.clone()),
            TranscriptItem::Note(msg) => ItemView::Note(msg.clone()),
        }
    }
}

impl AiPanel {
    fn render_attachment_chip(
        &mut self,
        ix: usize,
        name: String,
        theme: &gpui_component::Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        h_flex()
            .id(format!("ai-att-{ix}"))
            .px_1p5()
            .py_0p5()
            .rounded(px(4.))
            .border_1()
            .border_color(theme.border)
            .gap_1()
            .text_xs()
            .child(div().max_w(px(160.)).truncate().child(name))
            .child(
                div()
                    .id(format!("ai-att-x-{ix}"))
                    .cursor_pointer()
                    .text_color(theme.muted_foreground)
                    .child("✕")
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.session.update(cx, |s, cx| s.remove_attachment(ix, cx));
                    })),
            )
            .into_any_element()
    }
}

#[allow(clippy::too_many_arguments)]
fn render_item(
    ix: usize,
    item: &ItemView,
    status_zh: bool,
    status_idx: usize,
    theme: &gpui_component::Theme,
    plan_note: Option<Entity<InputState>>,
    window: &mut Window,
    cx: &mut Context<AiPanel>,
) -> AnyElement {
    match item {
        ItemView::User {
            text,
            attachment_names,
        } => {
            // Bubble sits in a full-width row (justify_end) so the text gets
            // a definite available width and actually wraps — inside an
            // items_end column it is measured fit-content and lays out as
            // one unwrapped line that overflows the panel.
            let bubble = div()
                .max_w(px(340.))
                .px_2()
                .py_1p5()
                .rounded_lg()
                .bg(theme.accent.opacity(0.15))
                .text_sm()
                .whitespace_normal()
                .child(text.clone());
            v_flex()
                .gap_1()
                .child(h_flex().w_full().justify_end().child(bubble))
                .when(!attachment_names.is_empty(), |col| {
                    col.child(
                        h_flex().w_full().justify_end().child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("📎 {}", attachment_names.join(", "))),
                        ),
                    )
                })
                .child(
                    h_flex()
                        .w_full()
                        .justify_end()
                        .child(copy_chip(format!("ai-copy-user-{ix}"), text.clone(), theme)),
                )
                .into_any_element()
        }
        ItemView::Assistant { text, streaming, usage } => {
            let status_word = if *streaming && text.is_empty() {
                style::status_word(status_zh, status_idx)
            } else {
                ""
            };
            // Plain stretched column: children receive a definite width and
            // wrap; `items_start` would measure them fit-content (MinContent)
            // and lay text out as unwrapped single lines.
            v_flex()
                .w_full()
                .gap_1()
                .when(!status_word.is_empty(), |col| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(format!("{status_word}…")),
                    )
                })
                .when(!text.is_empty(), |col| {
                    // Markdown-rendered streaming output (re-parsed each
                    // render; chat-bubble sizes make that cheap).
                    col.child(
                        div()
                            .max_w_full()
                            .min_w_0()
                            .child(crate::ui::ai_markdown::render_markdown(text, theme))
                            .when(*streaming, |d| {
                                d.child(div().text_sm().text_color(theme.accent).child("▍"))
                            }),
                    )
                })
                .when_some(usage.clone(), |col, u| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(u.display()),
                    )
                })
                .into_any_element()
        }
        ItemView::WorkPlan { steps } => v_flex()
            .id(format!("ai-workplan-{ix}"))
            .px_2()
            .py_1p5()
            .rounded_md()
            .border_1()
            .border_color(theme.accent.opacity(0.4))
            .bg(theme.accent.opacity(0.04))
            .gap_1()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("🗂 工作计划（自动随压缩保留）"),
            )
            .children(steps.iter().map(|(step, detail, code)| {
                let (icon, color) = match code {
                    2 => ("✓", theme.success),
                    1 => ("▶", theme.accent),
                    _ => ("◻", theme.muted_foreground),
                };
                v_flex()
                    .gap(px(1.))
                    .child(
                        h_flex()
                            .gap_1()
                            .items_start()
                            .child(div().text_xs().text_color(color).child(icon))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .text_color(if *code == 2 {
                                        theme.muted_foreground
                                    } else {
                                        theme.foreground
                                    })
                                    .child(step.clone()),
                            ),
                    )
                    .when(!detail.is_empty(), |col| {
                        col.child(
                            div()
                                .pl(px(14.))
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(detail.clone()),
                        )
                    })
            }))
            .into_any_element(),
        ItemView::Tool { name, summary, ok } => h_flex()
            .id(format!("ai-tool-{ix}"))
            .max_w_full()
            .px_2()
            .py_1()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.4))
            .gap_2()
            .text_xs()
            .child(
                div()
                    .flex_shrink_0()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(format!("⚙ {name}")),
            )
            // Single-line ellipsis: wrapping long JSON summaries under a
            // fixed max_h sliced the second line of glyphs in half.
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_color(if *ok { theme.foreground } else { theme.danger })
                    .child(summary.clone()),
            )
            .into_any_element(),
        ItemView::Plan { lines, pending } => {
            let pending = *pending;
            v_flex()
                .id(format!("ai-plan-{ix}"))
                .px_2()
                .py_2()
                .rounded_md()
                .border_1()
                .border_color(theme.accent.opacity(0.6))
                .bg(theme.accent.opacity(0.05))
                .gap_1()
                .child(
                    div()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child("📋 变更计划"),
                )
                .children(lines.iter().map(|(ok, summary)| {
                    h_flex()
                        .gap_1()
                        .text_xs()
                        .child(
                            div()
                                .text_color(if *ok { theme.success } else { theme.danger })
                                .child(if *ok { "✓" } else { "✗" }),
                        )
                        .child(div().flex_1().min_w_0().truncate().child(summary.clone()))
                }))
                .when(pending, |card| {
                    card.child(
                        v_flex()
                            .gap_2()
                            .mt_1()
                            .when_some(plan_note, |card, note| {
                                card.child(div().child(Input::new(&note).small()))
                            })
                            .child(
                                h_flex()
                                    .gap_2()
                                    .child(
                                        Button::new(("ai-plan-apply", ix))
                                            .primary()
                                            .xsmall()
                                            .label("应用")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.session.update(cx, |s, cx| {
                                                    s.decide_plan(ix, PlanDecision::Apply, cx)
                                                });
                                            })),
                                    )
                                    .child(
                                        Button::new(("ai-plan-revise", ix))
                                            .xsmall()
                                            .label("让AI调整")
                                            .on_click(cx.listener(move |this, _, window, cx| {
                                                let note = this
                                                    .plan_notes
                                                    .get(&ix)
                                                    .map(|n| n.read(cx).value().to_string())
                                                    .unwrap_or_default();
                                                if let Some(n) = this.plan_notes.get(&ix) {
                                                    n.update(cx, |st, cx| st.set_value("", window, cx));
                                                }
                                                this.session.update(cx, |s, cx| {
                                                    s.decide_plan(
                                                        ix,
                                                        PlanDecision::Revise { note },
                                                        cx,
                                                    )
                                                });
                                            })),
                                    )
                                    .child(
                                        Button::new(("ai-plan-reject", ix))
                                            .danger()
                                            .xsmall()
                                            .label("拒绝")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.session.update(cx, |s, cx| {
                                                    s.decide_plan(ix, PlanDecision::Reject, cx)
                                                });
                                            })),
                                    ),
                            ),
                    )
                })
                .into_any_element()
        }
        ItemView::Error(msg) => div()
            .px_2()
            .py_1()
            .rounded_md()
            .bg(theme.danger.opacity(0.1))
            .text_xs()
            .text_color(theme.danger)
            .child(format!("⚠ {msg}"))
            .into_any_element(),
        ItemView::Note(msg) => div()
            .text_xs()
            .text_color(theme.muted_foreground)
            .child(format!("· {msg}"))
            .into_any_element(),
    }
}

impl Render for AiPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.open {
            return div().into_any_element();
        }
        if self.pending_provider_refresh {
            self.pending_provider_refresh = false;
            self.refresh_provider_select(window, cx);
            self.pending_model_refresh = true;
        }
        if self.pending_model_refresh {
            self.pending_model_refresh = false;
            self.refresh_model_select(window, cx);
        }
        // Provider/model lists can change in the settings dialog (own OS
        // window) — watch the settings file and refresh on change.
        let mtime = config::settings_mtime();
        if mtime != self.providers_mtime {
            self.providers_mtime = mtime;
            self.refresh_provider_select(window, cx);
            self.refresh_model_select(window, cx);
        }
        let theme = cx.theme().clone();
        if self.minimized {
            return self.render_edge_pill(window, cx);
        }
        let dragging = self.drag_anchor.is_some();
        let resizing = self.resize_start.is_some();

        gpui::deferred(
            div()
                .id("ai-float")
                .absolute()
                .left(self.x)
                .top(self.y)
                .w(self.w)
                .h(self.h)
                .flex()
                .flex_col()
                .rounded_lg()
                .border_1()
                .border_color(if dragging || resizing { theme.accent } else { theme.border })
                .shadow_lg()
                .bg(theme.background)
                .overflow_hidden()
                .child(self.render_title(window, cx))
                .child(self.render_transcript(window, cx))
                .child(self.render_status(cx))
                .child(self.render_composer(window, cx))
                // Resize grips: right edge, bottom edge, bottom-right corner.
                // Press starts window-level tracking (registered below).
                .child(
                    div()
                        .id("ai-float-resize-right")
                        .absolute()
                        .top(px(8.))
                        .bottom(px(18.))
                        .right_0()
                        .w(px(5.))
                        .cursor_ew_resize()
                        .hover(|d| d.bg(theme.accent.opacity(0.5)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                                this.resize_start = Some((
                                    ResizeAxis::Right,
                                    ev.position,
                                    this.x,
                                    this.y,
                                    this.w,
                                    this.h,
                                ));
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("ai-float-resize-bottom")
                        .absolute()
                        .left(px(8.))
                        .right(px(18.))
                        .bottom_0()
                        .h(px(5.))
                        .cursor_ns_resize()
                        .hover(|d| d.bg(theme.accent.opacity(0.5)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                                this.resize_start = Some((
                                    ResizeAxis::Bottom,
                                    ev.position,
                                    this.x,
                                    this.y,
                                    this.w,
                                    this.h,
                                ));
                                cx.notify();
                            }),
                        ),
                )
                .child(
                    div()
                        .id("ai-float-resize")
                        .absolute()
                        .bottom_0()
                        .right_0()
                        .size(px(18.))
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_nwse_resize()
                        .text_color(theme.muted_foreground)
                        .opacity(0.5)
                        .hover(|d| d.opacity(1.0))
                        .child(IconName::ResizeCorner)
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                                this.resize_start = Some((
                                    ResizeAxis::Corner,
                                    ev.position,
                                    this.x,
                                    this.y,
                                    this.w,
                                    this.h,
                                ));
                                cx.notify();
                            }),
                        ),
                )
                // Left edge grip: width + x.
                .child(
                    div()
                        .id("ai-float-resize-left")
                        .absolute()
                        .top(px(8.))
                        .bottom(px(18.))
                        .left_0()
                        .w(px(5.))
                        .cursor_ew_resize()
                        .hover(|d| d.bg(theme.accent.opacity(0.5)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                                this.resize_start = Some((
                                    ResizeAxis::Left,
                                    ev.position,
                                    this.x,
                                    this.y,
                                    this.w,
                                    this.h,
                                ));
                                cx.notify();
                            }),
                        ),
                )
                // Top edge grip: height + y.
                .child(
                    div()
                        .id("ai-float-resize-top")
                        .absolute()
                        .left(px(8.))
                        .right(px(18.))
                        .top_0()
                        .h(px(5.))
                        .cursor_ns_resize()
                        .hover(|d| d.bg(theme.accent.opacity(0.5)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, ev: &MouseDownEvent, _w, cx| {
                                this.resize_start = Some((
                                    ResizeAxis::Top,
                                    ev.position,
                                    this.x,
                                    this.y,
                                    this.w,
                                    this.h,
                                ));
                                cx.notify();
                            }),
                        ),
                )
                // While a drag/resize is active, a zero-size canvas registers
                // window-level trackers so movement continues even when the
                // cursor leaves the panel (same technique as the wysiwyg
                // scrollbar drag).
                .when(dragging || resizing, |d| {
                    let weak_move = cx.entity().downgrade();
                    let weak_up = weak_move.clone();
                    d.child(gpui::canvas(
                        |_, _, _| (),
                        move |_, _, window, _| {
                            window.on_mouse_event(
                                move |event: &MouseMoveEvent, phase, _window, cx| {
                                    if !phase.bubble() || !event.dragging() {
                                        return;
                                    }
                                    let _ = weak_move.update(cx, |p, cx| {
                                        if let Some(anchor) = p.drag_anchor {
                                            p.x = (event.position.x - anchor.x).max(px(0.));
                                            p.y = (event.position.y - anchor.y).max(px(0.));
                                            p.persist(false, cx);
                                            cx.notify();
                                        } else if let Some((axis, start, x0, y0, w0, h0)) = p.resize_start {
                                            match axis {
                                                ResizeAxis::Right | ResizeAxis::Corner => {
                                                    p.w = (w0 + event.position.x - start.x).max(px(320.));
                                                }
                                                ResizeAxis::Bottom => {}
                                                ResizeAxis::Left => {
                                                    // Growing from the left: keep the right edge fixed.
                                                    let right = x0 + w0;
                                                    let mut nx = event.position.x.max(px(0.));
                                                    nx = nx.min(right - px(320.)).max(px(0.));
                                                    p.x = nx;
                                                    p.w = right - nx;
                                                }
                                                ResizeAxis::Top => {
                                                    let bottom = y0 + h0;
                                                    let mut ny = event.position.y.max(px(0.));
                                                    ny = ny.min(bottom - px(360.)).max(px(0.));
                                                    p.y = ny;
                                                    p.h = bottom - ny;
                                                }
                                            }
                                            if matches!(axis, ResizeAxis::Bottom | ResizeAxis::Corner) {
                                                p.h = (h0 + event.position.y - start.y).max(px(360.));
                                            }
                                            p.persist(false, cx);
                                            cx.notify();
                                        }
                                    });
                                },
                            );
                            window.on_mouse_event(
                                move |_event: &MouseUpEvent, phase, _window, cx| {
                                    if !phase.bubble() {
                                        return;
                                    }
                                    let _ = weak_up.update(cx, |p, cx| {
                                        let was_active =
                                            p.drag_anchor.take().is_some() | p.resize_start.take().is_some();
                                        if was_active {
                                            p.persist(true, cx);
                                            cx.notify();
                                        }
                                    });
                                },
                            );
                        },
                    ))
                }),
        )
        .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::{cap_chars, extract_urls, html_to_text};

    #[test]
    fn extract_urls_filters_assets_and_caps() {
        let text = "看这个文档 https://confluence.example.com/x/yA1?f=1，还有 http://api.foo.com/v2\n\
                    忽略图片 https://a.com/1.png 和 https://b.com/app.js 以及纯文本 foo.bar";
        let urls = extract_urls(text);
        assert_eq!(
            urls,
            vec![
                "https://confluence.example.com/x/yA1?f=1".to_string(),
                "http://api.foo.com/v2".to_string(),
            ]
        );
    }

    #[test]
    fn extract_urls_dedupes_and_strips_trailing_punct() {
        let urls = extract_urls("https://example.com/docs。再次 https://example.com/docs");
        assert_eq!(urls, vec!["https://example.com/docs".to_string()]);
    }

    #[test]
    fn html_to_text_keeps_tables_and_drops_scripts() {
        let html = r#"<html><head><style>.a{color:red}</style><script>alert(1)</script></head>
<body><h1>用户接口文档</h1>
<table><tr><td>GET</td><td>/users</td></tr><tr><td>POST</td><td>/users/{id}</td></tr></table>
<p>&amp; &lt;note&gt;</p></body></html>"#;
        let text = html_to_text(html);
        assert!(!text.contains("alert"), "script dropped: {text}");
        assert!(!text.contains("color:red"), "style dropped: {text}");
        assert!(text.contains("用户接口文档"));
        assert!(text.contains("GET | /users"));
        assert!(text.contains("POST | /users/{id}"));
        assert!(text.contains("& <note>"));
    }

    #[test]
    fn cap_chars_is_char_safe() {
        let s = "汉".repeat(10);
        let out = cap_chars(s, 3, "…");
        assert_eq!(out.chars().count(), 4);
        assert!(out.ends_with('…'));
        let short = cap_chars("abc".to_string(), 5, "…");
        assert_eq!(short, "abc");
    }
}
