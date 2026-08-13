//! Editor window rendering: centered scrollable block column,
//! unsaved-changes overlay dialog, custom scrollbar, and deferred
//! operations (focus, scroll, save, window title).

use std::time::{Duration, Instant};

use gpui::*;
use gpui_component::ActiveTheme as _;
use gpui_component::Disableable as _;
use gpui_component::Sizable as _;
use gpui_component::button::ButtonVariants as _;

use super::{Editor, InfoDialogKind, RenderWindow};
use crate::wysiwyg::app_menu::dispatch_menu_action_for_editor;
use crate::wysiwyg::components::CalloutVariant;
use crate::wysiwyg::components::{
    AddLanguageConfig, AddThemeConfig, Block, DismissTransientUi, FindNext, FindPrevious,
    NoRecentFiles, ReplaceAll,
};
use crate::wysiwyg::i18n::{I18nManager, I18nStrings};
use crate::wysiwyg::theme::{Theme, ThemeDimensions, ThemeManager};
use crate::wysiwyg::window_chrome::{custom_titlebar_height, render_custom_titlebar};

pub(crate) const ABOUT_GITHUB_URL: &str = "https://github.com/manyougz/velotype";

/// Rows within this many pixels of the viewport stay mounted, so a fast flick
/// paints them before they scroll in instead of showing a blank edge. The
/// window must be wide enough to cover a fast scrollbar-drag jump: when the
/// viewport teleports into a region whose rows have no cached stride yet, the
/// band has to reach past those unmeasured rows so they are already mounted
/// (and laid out) by the time they enter the viewport. 800px (~25 lines) was
/// too tight for a scrollbar fling and left a black gap that only filled in
/// after the next frame's re-measure — visibly read as "black screen + jitter."
/// 1600 still missed during aggressive drags into tall code/table blocks, so
/// raise it to 2400 (~75 short rows) to absorb the worst flings.
const RENDER_OVERDRAW_PX: f32 = 2400.0;

pub(crate) fn open_about_github_url(cx: &mut App) {
    cx.open_url(ABOUT_GITHUB_URL);
}

fn editor_text_font() -> Font {
    // FontFallbacks is internally `Arc<Vec<String>>` — building it once
    // per process and Arc-cloning per render is the right shape, since
    // editor_text_font() is called from Editor::render on every frame.
    static FALLBACKS: std::sync::OnceLock<FontFallbacks> = std::sync::OnceLock::new();
    let fallbacks = FALLBACKS
        .get_or_init(|| {
            FontFallbacks::from_fonts(tibetan_font_fallbacks_for_target_os(std::env::consts::OS))
        })
        .clone();
    let mut font = font(".SystemUIFont");
    font.fallbacks = Some(fallbacks);
    font
}

fn tibetan_font_fallbacks_for_target_os(target_os: &str) -> Vec<String> {
    let families = match target_os {
        "windows" => &[
            "Microsoft Himalaya",
            "Noto Serif Tibetan",
            "Noto Sans Tibetan",
            "BabelStone Tibetan",
        ][..],
        "macos" => &["Kailasa", "Noto Serif Tibetan", "Noto Sans Tibetan"][..],
        _ => &[
            "Noto Serif Tibetan",
            "Noto Sans Tibetan",
            "Microsoft Himalaya",
            "Kailasa",
            "BabelStone Tibetan",
        ][..],
    };
    families
        .iter()
        .map(|family| (*family).to_string())
        .collect()
}

/// Adjacent-row metadata used to collapse spacing inside visual groups.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct RenderedRowSpacingInfo {
    quote_group_anchor: Option<uuid::Uuid>,
    visible_quote_group_anchor: Option<uuid::Uuid>,
    callout_anchor: Option<uuid::Uuid>,
    callout_variant: Option<CalloutVariant>,
    is_callout_header: bool,
    footnote_anchor: Option<uuid::Uuid>,
    is_footnote_header: bool,
}

impl RenderedRowSpacingInfo {
    fn from_block(block: &Block) -> Self {
        Self {
            quote_group_anchor: block.quote_group_anchor,
            visible_quote_group_anchor: block.visible_quote_group_anchor,
            callout_anchor: block.callout_anchor,
            callout_variant: block.callout_variant,
            is_callout_header: block.kind().is_callout(),
            footnote_anchor: block.footnote_anchor,
            is_footnote_header: block.kind().is_footnote_definition(),
        }
    }
}

fn rendered_row_top_gap(
    previous: Option<RenderedRowSpacingInfo>,
    current: RenderedRowSpacingInfo,
    default_gap: f32,
) -> f32 {
    let Some(previous) = previous else {
        return 0.0;
    };

    if previous.quote_group_anchor.is_some()
        && previous.quote_group_anchor == current.quote_group_anchor
    {
        0.0
    } else {
        default_gap
    }
}

fn callout_colors(variant: CalloutVariant, theme: &Theme) -> (Hsla, Hsla) {
    let c = &theme.colors;
    match variant {
        CalloutVariant::Note => (c.callout_note_border, c.callout_note_bg),
        CalloutVariant::Tip => (c.callout_tip_border, c.callout_tip_bg),
        CalloutVariant::Important => (c.callout_important_border, c.callout_important_bg),
        CalloutVariant::Warning => (c.callout_warning_border, c.callout_warning_bg),
        CalloutVariant::Caution => (c.callout_caution_border, c.callout_caution_bg),
    }
}

fn callout_row_top_gap(
    previous: Option<RenderedRowSpacingInfo>,
    current: RenderedRowSpacingInfo,
    dimensions: &ThemeDimensions,
) -> f32 {
    let Some(previous) = previous else {
        return 0.0;
    };

    if previous.visible_quote_group_anchor.is_some()
        && previous.visible_quote_group_anchor == current.visible_quote_group_anchor
    {
        return 0.0;
    }

    if previous.is_callout_header {
        dimensions.callout_header_margin_bottom
    } else {
        dimensions.callout_body_gap
    }
}

fn footnote_row_top_gap(previous: Option<RenderedRowSpacingInfo>, default_gap: f32) -> f32 {
    let Some(previous) = previous else {
        return 0.0;
    };

    if previous.is_footnote_header {
        default_gap * 0.75
    } else {
        default_gap
    }
}

fn is_wide_menu_char(ch: char) -> bool {
    matches!(
        ch as u32,
        0x1100..=0x11ff
            | 0x2e80..=0xa4cf
            | 0xac00..=0xd7a3
            | 0xf900..=0xfaff
            | 0xfe10..=0xfe6f
            | 0xff00..=0xff60
            | 0xffe0..=0xffe6
    )
}

fn estimated_menu_label_width(label: &str, text_size: f32) -> f32 {
    label
        .chars()
        .map(|ch| {
            if ch.is_ascii_whitespace() {
                text_size * 0.35
            } else if ch.is_ascii_punctuation() {
                text_size * 0.45
            } else if ch.is_ascii() {
                text_size * 0.62
            } else if is_wide_menu_char(ch) {
                text_size
            } else {
                text_size * 0.85
            }
        })
        .sum()
}

fn menu_bar_button_width(label: &str, dimensions: &ThemeDimensions) -> f32 {
    let content_width = estimated_menu_label_width(label, dimensions.menu_text_size)
        + dimensions.menu_bar_button_padding_x * 2.0;
    dimensions.menu_bar_button_width.max(content_width.ceil())
}

fn supports_in_window_menu_for_target_os(target_os: &str) -> bool {
    target_os != "macos"
}

fn supports_in_window_menu() -> bool {
    supports_in_window_menu_for_target_os(std::env::consts::OS)
}

fn in_window_menu_bar_height_for_target_os(
    target_os: &str,
    has_menus: bool,
    dimensions: &ThemeDimensions,
) -> f32 {
    if has_menus && supports_in_window_menu_for_target_os(target_os) {
        dimensions.menu_bar_height
    } else {
        0.0
    }
}

fn menu_panel_left<S: AsRef<str>>(
    open_index: usize,
    menu_labels: &[S],
    dimensions: &ThemeDimensions,
) -> f32 {
    let prior_width: f32 = menu_labels
        .iter()
        .take(open_index)
        .map(|label| menu_bar_button_width(label.as_ref(), dimensions))
        .sum();
    dimensions.menu_bar_padding_x + prior_width + dimensions.menu_bar_gap * open_index as f32
}

fn menu_panel_width_for_labels<S: AsRef<str>>(labels: &[S], dimensions: &ThemeDimensions) -> f32 {
    let widest_label = labels
        .iter()
        .map(|label| estimated_menu_label_width(label.as_ref(), dimensions.menu_text_size))
        .fold(0.0, f32::max);
    let content_width = widest_label + dimensions.menu_item_padding_x * 2.0;
    dimensions.menu_panel_width.max(content_width.ceil())
}

fn owned_menu_item_labels(items: &[OwnedMenuItem]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| match item {
            OwnedMenuItem::Action { name, .. } => Some(name.to_string()),
            OwnedMenuItem::Submenu(menu) => Some(menu.name.to_string()),
            OwnedMenuItem::SystemMenu(menu) => Some(menu.name.to_string()),
            OwnedMenuItem::Separator => None,
        })
        .collect()
}

fn menu_item_visual_height(item: &OwnedMenuItem, dimensions: &ThemeDimensions) -> f32 {
    match item {
        OwnedMenuItem::Separator => {
            dimensions.menu_separator_height + dimensions.menu_separator_margin_y * 2.0
        }
        OwnedMenuItem::Action { .. } | OwnedMenuItem::Submenu(_) | OwnedMenuItem::SystemMenu(_) => {
            dimensions.menu_item_height
        }
    }
}

const SCROLLABLE_IMPORT_MENU_VISIBLE_ITEMS: usize = 12;

fn menu_items_visual_height_with_gaps(
    items: &[OwnedMenuItem],
    dimensions: &ThemeDimensions,
) -> f32 {
    if items.is_empty() {
        return 0.0;
    }

    let items_height: f32 = items
        .iter()
        .map(|item| menu_item_visual_height(item, dimensions))
        .sum();
    items_height + dimensions.menu_panel_gap * items.len().saturating_sub(1) as f32
}

fn import_menu_split_index(items: &[OwnedMenuItem]) -> Option<usize> {
    let [
        prefix @ ..,
        OwnedMenuItem::Separator,
        OwnedMenuItem::Action { action, .. },
    ] = items
    else {
        return None;
    };

    if action.as_ref().as_any().is::<AddThemeConfig>()
        || action.as_ref().as_any().is::<AddLanguageConfig>()
    {
        Some(prefix.len())
    } else {
        None
    }
}

fn scrollable_import_menu_scroll_height(
    scroll_items: &[OwnedMenuItem],
    footer_items: &[OwnedMenuItem],
    viewport_height: f32,
    top_offset: f32,
    dimensions: &ThemeDimensions,
) -> f32 {
    let visible_count = scroll_items.len().min(SCROLLABLE_IMPORT_MENU_VISIBLE_ITEMS);
    if visible_count == 0 {
        return 0.0;
    }

    let default_height =
        menu_items_visual_height_with_gaps(&scroll_items[..visible_count], dimensions);
    let footer_height = menu_items_visual_height_with_gaps(footer_items, dimensions);
    let footer_gap = if footer_items.is_empty() {
        0.0
    } else {
        dimensions.menu_panel_gap
    };
    let available_height = viewport_height
        - top_offset
        - dimensions.menu_panel_top
        - dimensions.menu_panel_padding * 2.0
        - footer_height
        - footer_gap
        - 8.0;
    let min_height = dimensions.menu_item_height.min(default_height).max(1.0);

    default_height.min(available_height.max(min_height))
}

fn submenu_panel_top(
    items: &[OwnedMenuItem],
    item_index: usize,
    dimensions: &ThemeDimensions,
) -> f32 {
    let prior_items_height: f32 = items
        .iter()
        .take(item_index)
        .map(|item| menu_item_visual_height(item, dimensions))
        .sum();
    let prior_gaps = dimensions.menu_panel_gap * item_index as f32;
    dimensions.menu_panel_top + dimensions.menu_panel_padding + prior_items_height + prior_gaps
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct MenuSubmenuBridgeGeometry {
    left: f32,
    top: f32,
    width: f32,
    height: f32,
}

fn submenu_bridge_geometry<S: AsRef<str>, T: AsRef<str>>(
    open_index: usize,
    menu_labels: &[S],
    items: &[OwnedMenuItem],
    item_index: usize,
    submenu_labels: &[T],
    dimensions: &ThemeDimensions,
) -> Option<MenuSubmenuBridgeGeometry> {
    let item = items.get(item_index)?;
    let main_panel_left = menu_panel_left(open_index, menu_labels, dimensions);
    let main_panel_width = menu_panel_width_for_labels(&owned_menu_item_labels(items), dimensions);
    let submenu_width = menu_panel_width_for_labels(submenu_labels, dimensions);
    let vertical_tolerance = dimensions.menu_panel_padding + dimensions.menu_panel_gap;
    let item_top = submenu_panel_top(items, item_index, dimensions);
    let top = (item_top - vertical_tolerance).max(dimensions.menu_panel_top);
    Some(MenuSubmenuBridgeGeometry {
        left: main_panel_left + main_panel_width,
        top,
        width: dimensions.menu_panel_gap + submenu_width,
        height: menu_item_visual_height(item, dimensions) + vertical_tolerance * 2.0,
    })
}

fn footnote_group_shell(
    children: Vec<AnyElement>,
    theme: &Theme,
    dimensions: &ThemeDimensions,
) -> AnyElement {
    div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(px(0.0))
        .px(px(dimensions.footnote_padding_x))
        .py(px(dimensions.footnote_padding_y))
        .rounded(px(dimensions.footnote_radius))
        .border(px(1.0))
        .border_color(theme.colors.footnote_border)
        .bg(theme.colors.footnote_bg)
        .children(children)
        .into_any_element()
}

impl Editor {
    fn on_titlebar_close(
        &mut self,
        event: &ClickEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if event.standard_click() {
            self.request_close_current_window(window, cx);
        }
    }

    pub(crate) fn install_close_guard(&mut self, cx: &mut Context<Self>, window: &mut Window) {
        if self.close_guard_installed {
            return;
        }

        self.force_install_close_guard(cx, window);
    }

    pub(crate) fn force_install_close_guard(
        &mut self,
        cx: &mut Context<Self>,
        window: &mut Window,
    ) {
        let editor = cx.entity().downgrade();
        window.on_window_should_close(cx, move |window, cx| {
            editor
                .update(cx, |this, cx| this.on_window_should_close(window, cx))
                .unwrap_or(true)
        });
        self.close_guard_installed = true;
    }

    fn apply_pending_focus(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(entity_id) = self.pending_focus.take() else {
            return;
        };
        let Some(block) = self.focusable_entity_by_id(entity_id) else {
            return;
        };
        let focus_handle = block.read(cx).focus_handle.clone();
        focus_handle.focus(window, cx);
        // Retry on the next frame if the target has no layout yet. On the first
        // render after a `set_content` note switch the freshly rebuilt block
        // has not been painted, so its `last_bounds` is `None`; calling
        // `focus()` now sets a handle whose element isn't mounted, and the
        // focus wouldn't survive to receive keystrokes. Re-arm `pending_focus`
        // (mirroring `pending_scroll_recheck_after_layout`) so the focus is
        // re-applied once the block is laid out.
        if block.read(cx).last_bounds.is_none() {
            self.pending_focus = Some(entity_id);
            cx.notify();
        }
    }

    fn ensure_focused_caret_visible(&mut self, window: &Window, cx: &App) -> bool {
        // Resolve the block whose caret/selection should stay in view. Normally
        // that is the actually-focused edit target, but while the find bar is
        // open the matched block is intentionally *not* keyboard-focused (the
        // query input keeps focus so Cmd-G/F3 keep working). In that case fall
        // back to `active_entity_id`, which the find navigation sets to the
        // matched block; the block's `selected_range` still resolves to bounds
        // via `active_range_or_cursor_bounds`, so we can scroll to the match.
        let target = self.focused_edit_target(window, cx).or_else(|| {
            if self.find_state.is_some() {
                self.active_entity_id
                    .and_then(|id| self.focusable_entity_by_id(id))
            } else {
                None
            }
        });
        let Some(focused_block) = target else {
            return false;
        };
        let active_bounds =
            focused_block.read_with(cx, |block, _cx| block.active_range_or_cursor_bounds());

        let viewport = self.scroll_handle.bounds();
        let padding = px(20.0);
        let top_limit = viewport.top() + padding;
        let bottom_limit = viewport.bottom() - padding;
        let scroll_offset = self.scroll_handle.offset();
        let mut offset = scroll_offset;
        let mut changed = false;

        if let Some(active_bounds) = active_bounds {
            // Caret/selection has layout bounds: scroll those into view.
            if active_bounds.top() < top_limit {
                offset.y += top_limit - active_bounds.top();
                changed = true;
            } else if active_bounds.bottom() > bottom_limit {
                offset.y -= active_bounds.bottom() - bottom_limit;
                changed = true;
            }
        } else {
            // No layout bounds: this happens for source-raw blocks (HtmlBlock,
            // …) in their rendered (unfocused) form, which never capture a text
            // layout. A find match that lands in such a block would otherwise
            // never scroll into view — the user steps to "容器" inside an HTML
            // block and the page doesn't move. Fall back to the block's
            // document-flow position (the sum of preceding rows' strides),
            // which is layout-form-invariant, and scroll that into view.
            if let Some(block_bounds) =
                self.estimated_block_bounds(focused_block.entity_id(), scroll_offset)
            {
                if block_bounds.top() < top_limit {
                    offset.y += top_limit - block_bounds.top();
                    changed = true;
                } else if block_bounds.bottom() > bottom_limit {
                    offset.y -= block_bounds.bottom() - bottom_limit;
                    changed = true;
                }
            }
        }

        if changed {
            let max_offset_y = self.scroll_handle.max_offset().y.max(px(0.0));
            offset.y = offset.y.min(px(0.0)).max(-max_offset_y);
            self.scroll_handle.set_offset(offset);
        }

        true
    }

    /// Approximate the screen-space bounds of a block by summing the cached
    /// strides of every preceding visible row and adding the current scroll
    /// offset. Used as a fallback when a block has no captured text layout
    /// (`last_bounds` is `None`, e.g. a rendered HtmlBlock), so find navigation
    /// can still scroll the block into view. Returns `None` if the block isn't
    /// a visible row's first block or has no cached stride yet.
    fn estimated_block_bounds(
        &self,
        entity_id: EntityId,
        scroll_offset: Point<Pixels>,
    ) -> Option<Bounds<Pixels>> {
        let visible = self.document.visible_blocks();
        let estimate = visible
            .first()
            .and_then(|v| self.row_stride_cache.get(&v.entity.entity_id()).copied())
            .unwrap_or(28.0);
        let mut top = f32::from(scroll_offset.y);
        for visible in visible {
            let stride = self
                .row_stride_cache
                .get(&visible.entity.entity_id())
                .copied()
                .unwrap_or(estimate);
            if visible.entity.entity_id() == entity_id {
                return Some(Bounds::new(
                    point(px(0.0), px(top)),
                    size(px(0.0), px(stride.max(estimate))),
                ));
            }
            top += stride;
        }
        None
    }

    fn apply_pending_scroll_into_view(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.scrollbar_drag.is_some() {
            return;
        }

        if !self.pending_scroll_active_block_into_view {
            return;
        }

        // scroll_to_item indexed children by position, which the spacers break;
        // the focused block is always mounted, so pixel math on its bounds works.
        let has_bounds = self.ensure_focused_caret_visible(window, cx);
        if self.pending_scroll_recheck_after_layout {
            self.pending_scroll_recheck_after_layout = false;
            self.schedule_scroll_recheck(cx);
            return;
        }

        // `has_bounds` can be `true` via a layout-form estimate (no real text
        // layout yet), so gate completion on the authoritative signal: the
        // focused block having a captured `last_bounds` (i.e. its text element
        // has actually been measured and painted). On the first frame(s) after
        // opening a document the body's text layouts are still pending, and
        // only the title is mounted — without this gate the single-shot retry
        // would settle and the body would stay blank until an external repaint.
        let focused_has_real_bounds = self
            .focused_edit_target_entity_id(window, cx)
            .and_then(|id| self.focusable_entity_by_id(id))
            .map(|block| block.read(cx).last_bounds.is_some())
            .unwrap_or(true);

        if !has_bounds || !focused_has_real_bounds {
            // Keep retrying until the body is measured. The first retries run at
            // ~one frame (16 ms); later retries widen the interval so a layout
            // that never completes cannot spin the CPU. Give up after a bounded
            // number of attempts and settle on the current state — an external
            // repaint (scroll, resize) will still correct it.
            const MAX_RECHECKS: u32 = 60;
            if self.scroll_layout_recheck_count >= MAX_RECHECKS {
                self.pending_scroll_active_block_into_view = false;
                self.scroll_layout_recheck_count = 0;
                return;
            }
            self.scroll_layout_recheck_count += 1;
            self.pending_scroll_recheck_after_layout = true;
            let delay = if self.scroll_layout_recheck_count <= 8 {
                Duration::from_millis(16)
            } else {
                Duration::from_millis(40)
            };
            self.schedule_scroll_recheck_with_delay(delay, cx);
            return;
        }

        self.pending_scroll_active_block_into_view = false;
        self.scroll_layout_recheck_count = 0;
        self.scroll_recheck_task = None;
    }

    /// Cancel the view jump GPUI's scroll-offset clamp introduces when a click
    /// focuses a source-raw block (HtmlBlock, …) and shrinks the document.
    /// `focus_block_from_click` stashed the pre-click offset; this restores it
    /// (clamped to the *new* smaller range) so the clicked block stays where
    /// the user pointed. The anchor is consumed on the second render after the
    /// click — by then the focus flip has happened, the block has been laid out
    /// in its focused form, and GPUI's prepaint clamp has already moved the
    /// offset, so this is the correction.
    ///
    /// After restoring the offset, scroll the caret into view. This matters
    /// for source-raw blocks (HtmlBlock, …): the click landed on the *rendered*
    /// form, but the caret ends up in the *raw-source* form (its hit-test
    /// returns 0, i.e. the start of the raw source, since the rendered form has
    /// no captured text layout). Holding the view at the click offset would
    /// then leave the caret off-screen — the user would have to scroll to find
    /// it. Scrolling the caret into view after the restore keeps plain-text
    /// clicks still (the caret is already visible, so this is a no-op) while
    /// bringing the caret on screen for HTML blocks.
    fn apply_click_scroll_anchor(&mut self, window: &Window, cx: &mut Context<Self>) {
        if self.scrollbar_drag.is_some() {
            return;
        }
        let Some(anchor) = self.click_scroll_anchor.as_mut() else {
            return;
        };
        // One-frame deferral: the focus flip and GPUI's offset clamp both land
        // in the prepaint of the first render after the click, so the
        // compensation can only run on the second render. Clearing `pending`
        // here (render N+1, the clamp already fired on render N's prepaint)
        // lets the next render do the restore.
        if anchor.pending {
            anchor.pending = false;
            return;
        }
        let anchor = self.click_scroll_anchor.take().unwrap();
        let max_offset_y = f32::from(self.scroll_handle.max_offset().y.max(px(0.0)));
        // Restoring the pre-click offset, clamped to the (now smaller) valid
        // range, holds the clicked block's content-space position fixed: the
        // block top is invariant under scrolling, so the same offset paints it
        // at the same screen Y as at click time.
        let target = anchor.pre_offset_y.clamp(-max_offset_y, 0.0);
        let mut offset = self.scroll_handle.offset();
        if (f32::from(offset.y) - target).abs() > 0.5 {
            offset.y = px(target);
            self.scroll_handle.set_offset(offset);
        }
        // Now that the view is back where the user clicked, make sure the caret
        // (which may sit elsewhere for source-raw blocks) is actually visible.
        // For plain-text clicks the caret is at the click spot and already in
        // view, so this is a no-op; for HTML it nudges the view the minimal
        // amount to show the caret.
        self.ensure_focused_caret_visible(window, cx);
    }

    /// Requests a repaint one frame out so a still-pending scroll-into-view can
    /// retry once the target block has been laid out. `cx.notify()` is swallowed
    /// when called from within `render`, so without this the retry would wait
    /// for the next external notify (e.g. the cursor blink, ~0.5s later).
    fn schedule_scroll_recheck(&mut self, cx: &mut Context<Self>) {
        self.schedule_scroll_recheck_with_delay(Duration::from_millis(16), cx);
    }

    /// Like [`Self::schedule_scroll_recheck`] but with an explicit delay. Used by
    /// [`Self::apply_pending_scroll_into_view`] to keep retrying until the just-
    /// opened document body has actually been measured (a block's text layout is
    /// computed asynchronously via `request_measured_layout`, so on the first
    /// frame after `set_content`/`from_markdown`/file-open only the title has a
    /// real height). The single-shot 16 ms retry can fire before that deferred
    /// measurement commits — especially in a release build — leaving only the
    /// first block mounted until an external repaint (scroll wheel) re-layouts.
    /// Re-arming here until the focused block reports a real `last_bounds` closes
    /// that race. The delay grows each iteration so a truly stuck layout cannot
    /// spin the CPU.
    fn schedule_scroll_recheck_with_delay(&mut self, delay: Duration, cx: &mut Context<Self>) {
        self.scroll_recheck_task = Some(cx.spawn(async move |this: WeakEntity<Self>, cx| {
            cx.background_executor().timer(delay).await;
            let _ = this.update(cx, |_this, cx| cx.notify());
        }));
    }

    fn sync_pending_save(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_save {
            self.pending_save = false;
            self.save_document(window, cx);
        }
    }

    fn sync_pending_save_as(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_save_as {
            self.pending_save_as = false;
            self.save_document_as(window, cx);
        }
    }

    fn sync_pending_open_link(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(link) = self.pending_open_link.take() else {
            return;
        };

        let strings = cx.global::<I18nManager>().strings_arc();
        let buttons = [
            strings.open_link_open.as_str(),
            strings.open_link_cancel.as_str(),
        ];
        let prompt = window.prompt(
            PromptLevel::Info,
            &strings.open_link_title,
            Some(&link.prompt_target),
            &buttons,
            cx,
        );
        let window_handle = window.window_handle();
        cx.spawn(async move |_this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Ok(choice) = prompt.await else {
                return;
            };
            if choice == 0 {
                let _ = cx.update_window(window_handle, |_view: AnyView, _window, cx| {
                    cx.open_url(&link.open_target);
                });
            }
        })
        .detach();
    }

    fn sync_window_edited_state(&mut self, window: &mut Window) {
        if self.pending_window_edited {
            self.pending_window_edited = false;
            window.set_window_edited(true);
        }
    }

    fn sync_scroll_viewport(&mut self, viewport_size: Size<Pixels>, cx: &mut Context<Self>) {
        match self.last_scroll_viewport_size {
            Some(previous) if Self::viewport_size_changed(previous, viewport_size) => {
                self.last_scroll_viewport_size = Some(viewport_size);
                self.request_active_block_scroll_into_view(cx);
            }
            Some(_) => {}
            None => {
                self.last_scroll_viewport_size = Some(viewport_size);
            }
        }
    }

    fn sync_window_title(&mut self, window: &mut Window, strings: &I18nStrings) {
        if self.pending_window_title_refresh {
            self.pending_window_title_refresh = false;
            let title = Self::window_title(self.file_path.as_deref(), self.document_dirty, strings);
            window.set_window_title(&title);
        }
    }

    /// Renders the in-window fallback menu bar backed by the app menus
    /// registered through `App::set_menus`. `menus` and `menu_labels` are
    /// fetched and computed once at the caller and shared with
    /// [`Self::render_in_window_menu_panel`].
    fn render_in_window_menu_bar(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
        menus: Option<&[gpui::OwnedMenu]>,
        menu_labels: &[SharedString],
        top_offset: f32,
    ) -> Option<AnyElement> {
        let menus = menus?;
        if menus.is_empty() {
            return None;
        }

        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let editor = cx.entity().downgrade();
        let button_widths = menu_labels
            .iter()
            .map(|label| menu_bar_button_width(label, d))
            .collect::<Vec<_>>();

        Some(
            div()
                .id("app-menu-bar")
                .absolute()
                .top(px(top_offset))
                .left_0()
                .right_0()
                .h(px(d.menu_bar_height))
                .occlude()
                .flex()
                .items_center()
                .gap(px(d.menu_bar_gap))
                .px(px(d.menu_bar_padding_x))
                .py(px(d.menu_bar_padding_y))
                .bg(c.dialog_surface)
                .border_b(px(theme.dimensions.dialog_border_width))
                .border_color(c.dialog_border)
                .on_hover(cx.listener(Self::on_menu_bar_hover))
                .children(menu_labels.iter().enumerate().map(|(index, label)| {
                    let label = label.clone();
                    let is_open = self.menu_bar_open == Some(index);
                    let button_editor = editor.clone();
                    let button_width = button_widths[index];

                    div()
                        .id(("app-menu-button", index))
                        .h(px(d.menu_bar_button_height))
                        .w(px(button_width))
                        .px(px(d.menu_bar_button_padding_x))
                        .flex()
                        .flex_shrink_0()
                        .items_center()
                        .justify_center()
                        .rounded(px(d.menu_bar_button_radius))
                        .bg(if is_open {
                            c.dialog_secondary_button_hover
                        } else {
                            c.dialog_surface
                        })
                        .hover(|this| this.bg(c.dialog_secondary_button_hover))
                        .active(|this| this.opacity(0.92))
                        .cursor_pointer()
                        .text_size(px(d.menu_text_size))
                        .font_weight(t.dialog_button_weight.to_font_weight())
                        .text_color(c.dialog_secondary_button_text)
                        .whitespace_nowrap()
                        .child(label)
                        .on_hover(move |hovered, _window, cx| {
                            if *hovered {
                                let _ = button_editor
                                    .update(cx, |editor, cx| editor.open_menu_bar(index, cx));
                            }
                        })
                }))
                .into_any_element(),
        )
    }

    fn render_in_window_menu_item(
        &self,
        item: OwnedMenuItem,
        item_index: usize,
        theme: &Theme,
        editor: WeakEntity<Self>,
    ) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;

        match item {
            OwnedMenuItem::Separator => div()
                .id(("app-menu-separator", item_index))
                .flex_shrink_0()
                .mx(px(d.menu_separator_margin_x))
                .my(px(d.menu_separator_margin_y))
                .h(px(d.menu_separator_height))
                .bg(c.dialog_border)
                .into_any_element(),
            OwnedMenuItem::Action { name, action, .. } => {
                let is_disabled = action.as_ref().as_any().is::<NoRecentFiles>();
                let click_editor = editor.clone();
                let hover_editor = editor.clone();
                let base = div()
                    .id(("app-menu-item", item_index))
                    .w_full()
                    .h(px(d.menu_item_height))
                    .flex_shrink_0()
                    .px(px(d.menu_item_padding_x))
                    .flex()
                    .items_center()
                    .rounded(px(d.menu_item_radius))
                    .bg(c.dialog_surface)
                    .text_size(px(d.menu_text_size))
                    .font_weight(t.dialog_body_weight.to_font_weight())
                    .text_color(if is_disabled {
                        c.dialog_muted
                    } else {
                        c.dialog_secondary_button_text
                    })
                    .child(name)
                    .on_hover(move |hovered, _window, cx| {
                        if *hovered {
                            let _ =
                                hover_editor.update(cx, |editor, cx| editor.close_menu_submenu(cx));
                        }
                    });

                if is_disabled {
                    base.into_any_element()
                } else {
                    base.hover(|this| this.bg(c.dialog_secondary_button_hover))
                        .active(|this| this.opacity(0.92))
                        .cursor_pointer()
                        .on_click(move |_, window, cx| {
                            let _ = click_editor.update(cx, |editor, cx| editor.close_menu_bar(cx));
                            dispatch_menu_action_for_editor(
                                action.as_ref(),
                                &click_editor,
                                window,
                                cx,
                            );
                        })
                        .into_any_element()
                }
            }
            OwnedMenuItem::Submenu(submenu) => {
                let is_open = self.menu_submenu_open == Some(item_index);
                let hover_editor = editor.clone();
                div()
                    .id(("app-menu-submenu", item_index))
                    .w_full()
                    .h(px(d.menu_item_height))
                    .flex_shrink_0()
                    .px(px(d.menu_item_padding_x))
                    .flex()
                    .items_center()
                    .justify_between()
                    .rounded(px(d.menu_item_radius))
                    .bg(if is_open {
                        c.dialog_secondary_button_hover
                    } else {
                        c.dialog_surface
                    })
                    .hover(|this| this.bg(c.dialog_secondary_button_hover))
                    .cursor_pointer()
                    .text_size(px(d.menu_text_size))
                    .font_weight(t.dialog_body_weight.to_font_weight())
                    .text_color(c.dialog_secondary_button_text)
                    .child(submenu.name.to_string())
                    .child(">")
                    .on_hover(move |hovered, _window, cx| {
                        if *hovered {
                            let _ = hover_editor
                                .update(cx, |editor, cx| editor.open_menu_submenu(item_index, cx));
                        }
                    })
                    .into_any_element()
            }
            OwnedMenuItem::SystemMenu(os_menu) => div()
                .id(("app-menu-system", item_index))
                .w_full()
                .h(px(d.menu_item_height))
                .flex_shrink_0()
                .px(px(d.menu_item_padding_x))
                .flex()
                .items_center()
                .rounded(px(d.menu_item_radius))
                .bg(c.dialog_surface)
                .text_size(px(d.menu_text_size))
                .text_color(c.dialog_muted)
                .child(os_menu.name.to_string())
                .into_any_element(),
        }
    }

    /// Renders the currently open in-window fallback menu as a floating
    /// panel. `menus` and `menu_labels` are fetched and computed once at
    /// the caller and shared with [`Self::render_in_window_menu_bar`].
    fn render_in_window_menu_panel(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
        menus: Option<&[gpui::OwnedMenu]>,
        menu_labels: &[SharedString],
        top_offset: f32,
        viewport_height: f32,
    ) -> Option<AnyElement> {
        let open_index = self.menu_bar_open?;
        let menus = menus?;
        let menu = menus.get(open_index)?.clone();
        let menu_items = menu.items.clone();
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let editor = cx.entity().downgrade();
        let menu_item_labels = owned_menu_item_labels(&menu_items);
        let menu_panel_width = menu_panel_width_for_labels(&menu_item_labels, d);
        let submenu_bridge = self.menu_submenu_open.and_then(|submenu_index| {
            match menu_items.get(submenu_index)? {
                OwnedMenuItem::Submenu(submenu) => {
                    let submenu_labels = owned_menu_item_labels(&submenu.items);
                    let geometry = submenu_bridge_geometry(
                        open_index,
                        menu_labels,
                        &menu_items,
                        submenu_index,
                        &submenu_labels,
                        d,
                    )?;
                    Some(
                        div()
                            .id(("app-submenu-bridge", open_index * 1000 + submenu_index))
                            .absolute()
                            .occlude()
                            .top(px(top_offset + geometry.top))
                            .left(px(geometry.left))
                            .w(px(geometry.width))
                            .h(px(geometry.height))
                            .bg(hsla(0.0, 0.0, 0.0, 0.0))
                            .on_hover(cx.listener(Self::on_menu_submenu_bridge_hover))
                            .into_any_element(),
                    )
                }
                _ => None,
            }
        });
        let submenu_panel =
            self.menu_submenu_open.and_then(|submenu_index| {
                match menu_items.get(submenu_index)? {
                    OwnedMenuItem::Submenu(submenu) => {
                        let submenu_labels = owned_menu_item_labels(&submenu.items);
                        let left = menu_panel_left(open_index, menu_labels, d)
                            + menu_panel_width
                            + d.menu_panel_gap;
                        let top = submenu_panel_top(&menu_items, submenu_index, d);
                        let submenu_width = menu_panel_width_for_labels(&submenu_labels, d);
                        let submenu_items = submenu.items.clone().into_iter().enumerate().map(
                            |(item_index, item)| match item {
                                OwnedMenuItem::Separator => div()
                                    .id((
                                        "app-submenu-separator",
                                        submenu_index * 1000 + item_index,
                                    ))
                                    .mx(px(d.menu_separator_margin_x))
                                    .my(px(d.menu_separator_margin_y))
                                    .h(px(d.menu_separator_height))
                                    .bg(c.dialog_border)
                                    .into_any_element(),
                                OwnedMenuItem::Action { name, action, .. } => {
                                    let is_disabled =
                                        action.as_ref().as_any().is::<NoRecentFiles>();
                                    let editor = editor.clone();
                                    let base = div()
                                        .id(("app-submenu-item", submenu_index * 1000 + item_index))
                                        .w_full()
                                        .h(px(d.menu_item_height))
                                        .px(px(d.menu_item_padding_x))
                                        .flex()
                                        .items_center()
                                        .rounded(px(d.menu_item_radius))
                                        .bg(c.dialog_surface)
                                        .text_size(px(d.menu_text_size))
                                        .font_weight(t.dialog_body_weight.to_font_weight())
                                        .text_color(if is_disabled {
                                            c.dialog_muted
                                        } else {
                                            c.dialog_secondary_button_text
                                        })
                                        .child(name);

                                    if is_disabled {
                                        base.into_any_element()
                                    } else {
                                        base.hover(|this| this.bg(c.dialog_secondary_button_hover))
                                            .active(|this| this.opacity(0.92))
                                            .cursor_pointer()
                                            .on_click(move |_, window, cx| {
                                                let _ = editor.update(cx, |editor, cx| {
                                                    editor.close_menu_bar(cx)
                                                });
                                                dispatch_menu_action_for_editor(
                                                    action.as_ref(),
                                                    &editor,
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .into_any_element()
                                    }
                                }
                                OwnedMenuItem::Submenu(submenu) => div()
                                    .id(("app-submenu-nested", submenu_index * 1000 + item_index))
                                    .w_full()
                                    .h(px(d.menu_item_height))
                                    .px(px(d.menu_item_padding_x))
                                    .flex()
                                    .items_center()
                                    .rounded(px(d.menu_item_radius))
                                    .bg(c.dialog_surface)
                                    .text_size(px(d.menu_text_size))
                                    .text_color(c.dialog_muted)
                                    .child(submenu.name.to_string())
                                    .into_any_element(),
                                OwnedMenuItem::SystemMenu(os_menu) => div()
                                    .id(("app-submenu-system", submenu_index * 1000 + item_index))
                                    .w_full()
                                    .h(px(d.menu_item_height))
                                    .px(px(d.menu_item_padding_x))
                                    .flex()
                                    .items_center()
                                    .rounded(px(d.menu_item_radius))
                                    .bg(c.dialog_surface)
                                    .text_size(px(d.menu_text_size))
                                    .text_color(c.dialog_muted)
                                    .child(os_menu.name.to_string())
                                    .into_any_element(),
                            },
                        );

                        Some(
                            div()
                                .id(("app-submenu-panel", open_index * 1000 + submenu_index))
                                .absolute()
                                .occlude()
                                .top(px(top_offset + top))
                                .left(px(left))
                                .w(px(submenu_width))
                                .p(px(d.menu_panel_padding))
                                .flex()
                                .flex_col()
                                .gap(px(d.menu_panel_gap))
                                .bg(c.dialog_surface)
                                .border(px(d.dialog_border_width))
                                .border_color(c.dialog_border)
                                .rounded(px(d.menu_panel_radius))
                                .shadow_lg()
                                .on_hover(cx.listener(Self::on_menu_submenu_panel_hover))
                                .children(submenu_items)
                                .into_any_element(),
                        )
                    }
                    _ => None,
                }
            });

        let main_panel = div()
            .id(("app-menu-panel", open_index))
            .absolute()
            .occlude()
            .top(px(top_offset + d.menu_panel_top))
            .left(px(menu_panel_left(open_index, menu_labels, d)))
            .w(px(menu_panel_width))
            .p(px(d.menu_panel_padding))
            .flex()
            .flex_col()
            .gap(px(d.menu_panel_gap))
            .bg(c.dialog_surface)
            .border(px(d.dialog_border_width))
            .border_color(c.dialog_border)
            .rounded(px(d.menu_panel_radius))
            .shadow_lg()
            .on_hover(cx.listener(Self::on_menu_panel_hover));
        let main_panel = if let Some(split_index) = import_menu_split_index(&menu_items) {
            let scroll_items = &menu_items[..split_index];
            let footer_items = &menu_items[split_index..];
            let scroll_height = scrollable_import_menu_scroll_height(
                scroll_items,
                footer_items,
                viewport_height,
                top_offset,
                d,
            );
            let scroll_area = (!scroll_items.is_empty()).then(|| {
                div()
                    .id(("app-menu-scroll-area", open_index))
                    .w_full()
                    .h(px(scroll_height))
                    .flex_shrink_0()
                    .min_h(px(0.0))
                    .overflow_y_scroll()
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(d.menu_panel_gap))
                            .children(scroll_items.iter().cloned().enumerate().map(
                                |(item_index, item)| {
                                    self.render_in_window_menu_item(
                                        item,
                                        item_index,
                                        theme,
                                        editor.clone(),
                                    )
                                },
                            )),
                    )
                    .into_any_element()
            });
            let footer_elements =
                footer_items
                    .iter()
                    .cloned()
                    .enumerate()
                    .map(|(footer_index, item)| {
                        self.render_in_window_menu_item(
                            item,
                            split_index + footer_index,
                            theme,
                            editor.clone(),
                        )
                    });

            main_panel
                .children(scroll_area)
                .children(footer_elements)
                .into_any_element()
        } else {
            let items = menu_items
                .iter()
                .cloned()
                .enumerate()
                .map(|(item_index, item)| {
                    self.render_in_window_menu_item(item, item_index, theme, editor.clone())
                });

            main_panel.children(items).into_any_element()
        };

        let layer = div()
            .id(("app-menu-panel-layer", open_index))
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .child(main_panel);
        let layer = if let Some(submenu_bridge) = submenu_bridge {
            layer.child(submenu_bridge)
        } else {
            layer
        };
        let layer = if let Some(submenu_panel) = submenu_panel {
            layer.child(submenu_panel)
        } else {
            layer
        };

        Some(layer.into_any_element())
    }

    /// Builds the unsaved-changes dialog with backdrop, message, and three
    /// action buttons (cancel, discard, save-and-close).
    fn render_unsaved_changes_overlay(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let strings = cx.global::<I18nManager>().strings();

        div()
            .id("unsaved-changes-overlay")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(c.dialog_backdrop)
            .child(
                div()
                    .w_full()
                    .px(px(d.editor_padding))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id("unsaved-changes-dialog")
                            .w(px(d.dialog_width))
                            .max_w(relative(1.0))
                            .flex()
                            .flex_col()
                            .gap(px(d.dialog_gap))
                            .p(px(d.dialog_padding))
                            .bg(c.dialog_surface)
                            .border(px(d.dialog_border_width))
                            .border_color(c.dialog_border)
                            .rounded(px(d.dialog_radius))
                            .shadow_lg()
                            .child(
                                div()
                                    .text_size(px(t.dialog_title_size))
                                    .font_weight(t.dialog_title_weight.to_font_weight())
                                    .text_color(c.dialog_title)
                                    .child(strings.unsaved_changes_title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(t.dialog_body_size))
                                    .font_weight(t.dialog_body_weight.to_font_weight())
                                    .line_height(rems(t.text_line_height))
                                    .text_color(c.dialog_body)
                                    .child(strings.unsaved_changes_message.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(d.dialog_button_gap))
                                    .child(
                                        div()
                                            .id("cancel-close-dialog")
                                            .h(px(d.dialog_button_height))
                                            .px(px(d.dialog_button_padding_x))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                            .border(px(d.dialog_border_width))
                                            .border_color(c.dialog_border)
                                            .bg(c.dialog_secondary_button_bg)
                                            .hover(|this| this.bg(c.dialog_secondary_button_hover))
                                            .active(|this| this.opacity(0.92))
                                            .cursor_pointer()
                                            .text_size(px(t.dialog_button_size))
                                            .font_weight(t.dialog_button_weight.to_font_weight())
                                            .text_color(c.dialog_secondary_button_text)
                                            .child(strings.unsaved_changes_cancel.clone())
                                            .on_click(cx.listener(Self::on_cancel_close_dialog)),
                                    )
                                    .child(
                                        div()
                                            .id("discard-and-close-dialog")
                                            .h(px(d.dialog_button_height))
                                            .px(px(d.dialog_button_padding_x))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                            .border(px(d.dialog_border_width))
                                            .border_color(c.dialog_border)
                                            .bg(c.dialog_danger_button_bg)
                                            .hover(|this| this.bg(c.dialog_danger_button_hover))
                                            .active(|this| this.opacity(0.92))
                                            .cursor_pointer()
                                            .text_size(px(t.dialog_button_size))
                                            .font_weight(t.dialog_button_weight.to_font_weight())
                                            .text_color(c.dialog_danger_button_text)
                                            .child(
                                                strings.unsaved_changes_discard_and_close.clone(),
                                            )
                                            .on_click(cx.listener(Self::on_discard_and_close)),
                                    )
                                    .child(
                                        div()
                                            .id("save-and-close-dialog")
                                            .h(px(d.dialog_button_height))
                                            .px(px(d.dialog_button_padding_x))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                            .bg(c.dialog_primary_button_bg)
                                            .hover(|this| this.bg(c.dialog_primary_button_hover))
                                            .active(|this| this.opacity(0.92))
                                            .cursor_pointer()
                                            .text_size(px(t.dialog_button_size))
                                            .font_weight(t.dialog_button_weight.to_font_weight())
                                            .text_color(c.dialog_primary_button_text)
                                            .child(strings.unsaved_changes_save_and_close.clone())
                                            .on_click(cx.listener(Self::on_save_and_close)),
                                    ),
                            ),
                    ),
            )
    }

    /// Builds the dropped-file replacement dialog shown when the current
    /// document has unsaved changes.
    fn render_drop_replace_overlay(
        &self,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let strings = cx.global::<I18nManager>().strings();

        div()
            .id("drop-replace-overlay")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(c.dialog_backdrop)
            .child(
                div()
                    .w_full()
                    .px(px(d.editor_padding))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id("drop-replace-dialog")
                            .w(px(d.dialog_width))
                            .max_w(relative(1.0))
                            .flex()
                            .flex_col()
                            .gap(px(d.dialog_gap))
                            .p(px(d.dialog_padding))
                            .bg(c.dialog_surface)
                            .border(px(d.dialog_border_width))
                            .border_color(c.dialog_border)
                            .rounded(px(d.dialog_radius))
                            .shadow_lg()
                            .child(
                                div()
                                    .text_size(px(t.dialog_title_size))
                                    .font_weight(t.dialog_title_weight.to_font_weight())
                                    .text_color(c.dialog_title)
                                    .child(strings.drop_replace_title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(t.dialog_body_size))
                                    .font_weight(t.dialog_body_weight.to_font_weight())
                                    .line_height(rems(t.text_line_height))
                                    .text_color(c.dialog_body)
                                    .child(strings.drop_replace_message.clone()),
                            )
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(d.dialog_button_gap))
                                    .child(
                                        div()
                                            .id("cancel-drop-replace-dialog")
                                            .h(px(d.dialog_button_height))
                                            .px(px(d.dialog_button_padding_x))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                            .border(px(d.dialog_border_width))
                                            .border_color(c.dialog_border)
                                            .bg(c.dialog_secondary_button_bg)
                                            .hover(|this| this.bg(c.dialog_secondary_button_hover))
                                            .active(|this| this.opacity(0.92))
                                            .cursor_pointer()
                                            .text_size(px(t.dialog_button_size))
                                            .font_weight(t.dialog_button_weight.to_font_weight())
                                            .text_color(c.dialog_secondary_button_text)
                                            .child(strings.drop_replace_cancel.clone())
                                            .on_click(
                                                cx.listener(Self::on_cancel_drop_replace_dialog),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("discard-and-replace-drop-dialog")
                                            .h(px(d.dialog_button_height))
                                            .px(px(d.dialog_button_padding_x))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                            .border(px(d.dialog_border_width))
                                            .border_color(c.dialog_border)
                                            .bg(c.dialog_danger_button_bg)
                                            .hover(|this| this.bg(c.dialog_danger_button_hover))
                                            .active(|this| this.opacity(0.92))
                                            .cursor_pointer()
                                            .text_size(px(t.dialog_button_size))
                                            .font_weight(t.dialog_button_weight.to_font_weight())
                                            .text_color(c.dialog_danger_button_text)
                                            .child(strings.drop_replace_discard_and_replace.clone())
                                            .on_click(
                                                cx.listener(Self::on_discard_and_replace_drop),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .id("save-and-replace-drop-dialog")
                                            .h(px(d.dialog_button_height))
                                            .px(px(d.dialog_button_padding_x))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                            .bg(c.dialog_primary_button_bg)
                                            .hover(|this| this.bg(c.dialog_primary_button_hover))
                                            .active(|this| this.opacity(0.92))
                                            .cursor_pointer()
                                            .text_size(px(t.dialog_button_size))
                                            .font_weight(t.dialog_button_weight.to_font_weight())
                                            .text_color(c.dialog_primary_button_text)
                                            .child(strings.drop_replace_save_and_replace.clone())
                                            .on_click(cx.listener(Self::on_save_and_replace_drop)),
                                    ),
                            ),
                    ),
            )
    }

    fn info_dialog_title<'a>(&self, strings: &'a I18nStrings, kind: InfoDialogKind) -> &'a str {
        match kind {
            InfoDialogKind::CheckForUpdates => &strings.help_check_updates_title,
            InfoDialogKind::About => &strings.help_about_title,
        }
    }

    pub(crate) fn about_dialog_body_lines(strings: &I18nStrings) -> Vec<String> {
        vec![
            format!("Velotype {}", env!("CARGO_PKG_VERSION")),
            strings.help_about_message.clone(),
            format!("{}: {}", strings.help_about_github_label, ABOUT_GITHUB_URL),
            strings.help_about_star_message.clone(),
        ]
    }

    fn info_dialog_body(&self, strings: &I18nStrings, kind: InfoDialogKind) -> String {
        match kind {
            InfoDialogKind::CheckForUpdates => strings.help_check_updates_message.clone(),
            InfoDialogKind::About => Self::about_dialog_body_lines(strings).join("\n"),
        }
    }

    fn render_info_dialog_body(
        &self,
        theme: &Theme,
        strings: &I18nStrings,
        kind: InfoDialogKind,
    ) -> AnyElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let body_style = |this: Div| {
            this.text_size(px(t.dialog_body_size))
                .font_weight(t.dialog_body_weight.to_font_weight())
                .line_height(rems(t.text_line_height))
                .text_color(c.dialog_body)
        };

        match kind {
            InfoDialogKind::CheckForUpdates => div()
                .flex()
                .flex_col()
                .gap(px(d.dialog_gap * 0.5))
                .child(
                    body_style(div()).children(
                        self.info_dialog_body(strings, kind)
                            .lines()
                            .map(|line| div().child(line.to_string())),
                    ),
                )
                .into_any_element(),
            InfoDialogKind::About => div()
                .flex()
                .flex_col()
                .gap(px(d.dialog_gap * 0.5))
                .child(body_style(div()).child(format!("Velotype {}", env!("CARGO_PKG_VERSION"))))
                .child(body_style(div()).child(strings.help_about_message.clone()))
                .child(
                    body_style(div())
                        .flex()
                        .flex_wrap()
                        .gap(px(4.0))
                        .child(format!("{}:", strings.help_about_github_label))
                        .child(
                            div()
                                .id("about-github-link")
                                .cursor_pointer()
                                .text_color(c.text_link)
                                .underline()
                                .child(ABOUT_GITHUB_URL)
                                .on_click(move |_, _, cx| {
                                    open_about_github_url(cx);
                                }),
                        ),
                )
                .child(body_style(div()).child(strings.help_about_star_message.clone()))
                .into_any_element(),
        }
    }

    fn on_dismiss_info_dialog(
        &mut self,
        _: &ClickEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.hide_info_dialog(cx);
    }

    fn render_info_dialog_overlay(
        &self,
        theme: &Theme,
        kind: InfoDialogKind,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let c = &theme.colors;
        let d = &theme.dimensions;
        let t = &theme.typography;
        let strings = cx.global::<I18nManager>().strings();

        div()
            .id("info-dialog-overlay")
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .bottom_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(c.dialog_backdrop)
            .child(
                div()
                    .w_full()
                    .px(px(d.editor_padding))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .id("info-dialog")
                            .w(px(d.dialog_width))
                            .max_w(relative(1.0))
                            .flex()
                            .flex_col()
                            .gap(px(d.dialog_gap))
                            .p(px(d.dialog_padding))
                            .bg(c.dialog_surface)
                            .border(px(d.dialog_border_width))
                            .border_color(c.dialog_border)
                            .rounded(px(d.dialog_radius))
                            .shadow_lg()
                            .child(
                                div()
                                    .text_size(px(t.dialog_title_size))
                                    .font_weight(t.dialog_title_weight.to_font_weight())
                                    .text_color(c.dialog_title)
                                    .child(self.info_dialog_title(strings, kind).to_string()),
                            )
                            .child(self.render_info_dialog_body(theme, strings, kind))
                            .child(
                                div()
                                    .flex()
                                    .justify_end()
                                    .gap(px(d.dialog_button_gap))
                                    .child(
                                        div()
                                            .id("dismiss-info-dialog")
                                            .h(px(d.dialog_button_height))
                                            .px(px(d.dialog_button_padding_x))
                                            .flex()
                                            .items_center()
                                            .justify_center()
                                            .rounded(px((d.dialog_radius - 4.0).max(0.0)))
                                            .bg(c.dialog_primary_button_bg)
                                            .hover(|this| this.bg(c.dialog_primary_button_hover))
                                            .active(|this| this.opacity(0.92))
                                            .cursor_pointer()
                                            .text_size(px(t.dialog_button_size))
                                            .font_weight(t.dialog_button_weight.to_font_weight())
                                            .text_color(c.dialog_primary_button_text)
                                            .child(strings.info_dialog_ok.clone())
                                            .on_click(cx.listener(Self::on_dismiss_info_dialog)),
                                    ),
                            ),
                    ),
            )
    }

    /// Top-right find bar overlay (Cmd/Ctrl+F). Shows the query input, a
    /// `n/total` match counter, prev/next buttons, and a close button. The bar
    /// carries the `EditorFindBar` key context so FindNext/FindPrevious
    /// bindings fire while the query field is focused.
    fn render_find_bar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<AnyElement> {
        use gpui_component::button::Button;
        use gpui_component::h_flex;
        use gpui_component::input::Input;
        use gpui_component::v_flex;

        let state = self.find_state.as_ref()?;
        let c = &theme.colors;
        let total = state.matches.len();
        let current_label = if total == 0 {
            "0/0".to_string()
        } else {
            format!("{}/{}", state.current + 1, total)
        };
        let input = state.input.clone();
        let replace_input = state.replace_input.clone();
        let replace_mode = state.replace_mode;

        // Optional replacement row: replacement input + Replace / Replace-all
        // buttons. Only rendered while the bar is in replace mode (Cmd/Ctrl+H).
        let replace_row = replace_mode.then(|| {
            h_flex()
                .items_center()
                .gap(px(6.))
                .child(
                    h_flex()
                        .w(px(220.))
                        .child(Input::new(&replace_input).small()),
                )
                .child(
                    Button::new("find-replace")
                        .small()
                        .ghost()
                        .label("Replace")
                        .disabled(total == 0)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_replace_one(window, cx);
                        })),
                )
                .child(
                    Button::new("find-replace-all")
                        .small()
                        .ghost()
                        .label("All")
                        .disabled(total == 0)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.on_replace_all(&ReplaceAll, window, cx);
                        })),
                )
        });

        Some(
            div()
                .id("editor-find-bar")
                .absolute()
                .top(px(8.))
                .right(px(16.))
                .occlude()
                // Scope FindNext/FindPrevious bindings to the bar.
                .key_context("EditorFindBar")
                // Esc closes the bar. DismissTransientUi's escape binding is
                // context: None and gets dropped in the embedded build, so we
                // also handle the raw key here to make Esc reliable.
                .on_key_down(cx.listener(|this, event: &KeyDownEvent, window, cx| {
                    if event.keystroke.key == "escape" {
                        this.close_find_bar(window, cx);
                        cx.stop_propagation();
                    }
                }))
                .child(
                    v_flex()
                        .gap(px(6.))
                        .pl(px(8.))
                        .pr(px(4.))
                        .py(px(4.))
                        .rounded(px(6.))
                        .border(px(1.))
                        .border_color(c.dialog_border)
                        .bg(c.dialog_surface)
                        // Find row on top, replace row below.
                        .child(
                            h_flex()
                                .items_center()
                                .gap(px(6.))
                                .child(h_flex().w(px(220.)).child(Input::new(&input).small()))
                                .child(
                                    div()
                                        .min_w(px(40.))
                                        .text_size(px(12.))
                                        .text_color(c.dialog_body)
                                        .child(current_label),
                                )
                                .child(
                                    Button::new("find-prev")
                                        .small()
                                        .ghost()
                                        .label("↑")
                                        .disabled(total == 0)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.on_find_previous(&FindPrevious, window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("find-next")
                                        .small()
                                        .ghost()
                                        .label("↓")
                                        .disabled(total == 0)
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.on_find_next(&FindNext, window, cx);
                                        })),
                                )
                                .child(
                                    Button::new("find-close")
                                        .small()
                                        .ghost()
                                        .label("×")
                                        .on_click(cx.listener(|this, _, window, cx| {
                                            this.close_find_bar(window, cx);
                                        })),
                                ),
                        )
                        .children(replace_row),
                )
                .on_action(cx.listener(|this, _: &DismissTransientUi, window, cx| {
                    this.close_find_bar(window, cx);
                }))
                .into_any_element(),
        )
    }
}

impl Render for Editor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Only an editor that owns its window (standalone velotype app mode)
        // mutates the host window: close guard, "edited" dot, window-title
        // sync, save-via-prompt, in-window menu bar. Embedded hosts (notes
        // panel, markdown side-view) set `owns_window = false` and drive those
        // concerns themselves.
        if self.owns_window {
            self.install_close_guard(cx, window);
        }
        // Sync wrapper runtimes FIRST (before apply_pending_focus) so that a
        // first-click on an editable wrapper resolves the precise child + caret
        // and sets pending_focus to the correct target in a single frame,
        // avoiding the 2-frame flash of child 0 focused at offset 0.
        self.sync_html_wrapper_runtimes(window, cx);
        self.apply_pending_focus(window, cx);
        self.apply_pending_scroll_into_view(window, cx);
        self.apply_click_scroll_anchor(window, cx);
        self.last_selection_snapshot = self.capture_source_selection_snapshot(cx);
        if self.owns_window {
            self.sync_pending_save(window, cx);
            self.sync_pending_save_as(window, cx);
        }
        self.sync_pending_open_link(window, cx);
        if self.owns_window {
            self.sync_window_edited_state(window);
        }

        let viewport_bounds = self.scroll_handle.bounds();
        let viewport_size = viewport_bounds.size;
        self.sync_scroll_viewport(viewport_size, cx);

        let theme = cx.global::<ThemeManager>().current_arc();
        let strings = cx.global::<I18nManager>().strings_arc();
        if self.owns_window {
            self.sync_window_title(window, &strings);
        }

        let d = &theme.dimensions;
        let visible_blocks = self.document.visible_blocks().to_vec();
        let editor = cx.entity().downgrade();
        // The in-window menu bar only belongs to a window-owning editor.
        let has_menus = if self.owns_window {
            cx.get_menus()
                .map(|menus| !menus.is_empty())
                .unwrap_or(false)
        } else {
            false
        };
        let titlebar_height = custom_titlebar_height(window, d);
        let menu_bar_height =
            in_window_menu_bar_height_for_target_os(std::env::consts::OS, has_menus, d);
        let scroll_trigger_padding = (d.block_min_height * 0.75).max(16.0);
        let max_scroll_y = f32::from(self.scroll_handle.max_offset().y.max(px(0.0)));
        let viewport_height = f32::from(viewport_bounds.size.height.max(px(1.0)));
        // Extra room below the last block so the lowest line can be scrolled up
        // to the viewport center instead of being pinned to the bottom edge.
        let scroll_beyond_bottom = viewport_height * 0.5;
        let viewport_width = f32::from(viewport_bounds.size.width.max(px(1.0)));
        let has_overflow = max_scroll_y > 0.5;

        let centered_width = Self::centered_column_width(viewport_width, &theme.dimensions);
        let current_scroll_y = (-f32::from(self.scroll_handle.offset().y)).clamp(0.0, max_scroll_y);
        let scrollbar_geometry =
            Self::scrollbar_geometry(viewport_height, max_scroll_y, current_scroll_y);
        let track_height = scrollbar_geometry.track_height;
        let thumb_height = scrollbar_geometry.thumb_height;
        let thumb_top = scrollbar_geometry.thumb_top;

        let show_custom_scrollbar = has_overflow
            && (self.scrollbar_drag.is_some()
                || self.scrollbar_hovered
                || Instant::now() <= self.scrollbar_visible_until);

        // Spacing metadata is read on demand instead of pre-collected into a
        // Vec<RenderedRowSpacingInfo> sized to all visible blocks. For long
        // documents this skips a ~tens-of-KB allocation per frame; per-block
        // entity.read_with is a cheap immutable lock + 7-field struct copy.
        let spacing_for = |index: usize| -> RenderedRowSpacingInfo {
            visible_blocks[index]
                .entity
                .read_with(cx, |block, _cx| RenderedRowSpacingInfo::from_block(block))
        };
        let mut previous_row_spacing = None;
        // One entry per render row; off-screen rows are dropped after windowing.
        let mut row_elements: Vec<AnyElement> = Vec::new();
        let mut row_starts: Vec<usize> = Vec::new();
        // Each row's leading `mt` gap; the top spacer subtracts the first mounted
        // row's, since that row re-applies it.
        let mut row_top_gaps: Vec<f32> = Vec::new();
        let mut index = 0usize;
        while index < visible_blocks.len() {
            let first_visible = visible_blocks[index].clone();
            let first_spacing = spacing_for(index);
            let top_gap = rendered_row_top_gap(previous_row_spacing, first_spacing, d.block_gap);

            if let (Some(callout_anchor), Some(callout_variant)) =
                (first_spacing.callout_anchor, first_spacing.callout_variant)
            {
                let mut group_children = Vec::new();
                let mut group_end = index;
                let mut previous_callout_row = None;
                while group_end < visible_blocks.len()
                    && spacing_for(group_end).callout_anchor == Some(callout_anchor)
                {
                    let row_spacing = spacing_for(group_end);
                    if let Some(footnote_anchor) = row_spacing.footnote_anchor {
                        let mut footnote_children = Vec::new();
                        let mut footnote_end = group_end;
                        let mut previous_footnote_row = None;
                        while footnote_end < visible_blocks.len()
                            && spacing_for(footnote_end).callout_anchor == Some(callout_anchor)
                            && spacing_for(footnote_end).footnote_anchor == Some(footnote_anchor)
                        {
                            let footnote_spacing = spacing_for(footnote_end);
                            let entity = visible_blocks[footnote_end].entity.clone();
                            let row = div()
                                .w_full()
                                .flex_shrink_0()
                                .mt(px(footnote_row_top_gap(previous_footnote_row, d.block_gap)))
                                .child(entity.clone());
                            let row = if self.view_mode == super::ViewMode::Rendered {
                                let row_editor = editor.clone();
                                let entity_id = entity.entity_id();
                                row.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                                    let _ = row_editor.update(cx, |editor, cx| {
                                        editor.on_block_context_menu_mouse_down(
                                            entity_id, event, window, cx,
                                        );
                                    });
                                })
                            } else {
                                row
                            };
                            footnote_children.push(row.into_any_element());
                            previous_footnote_row = Some(footnote_spacing);
                            footnote_end += 1;
                        }

                        group_children.push(
                            div()
                                .w_full()
                                .flex_shrink_0()
                                .mt(px(callout_row_top_gap(
                                    previous_callout_row,
                                    row_spacing,
                                    d,
                                )))
                                .child(footnote_group_shell(footnote_children, &theme, d))
                                .into_any_element(),
                        );
                        previous_callout_row = Some(spacing_for(footnote_end - 1));
                        group_end = footnote_end;
                        continue;
                    }

                    let entity = visible_blocks[group_end].entity.clone();
                    let row = div()
                        .w_full()
                        .flex_shrink_0()
                        .mt(px(callout_row_top_gap(
                            previous_callout_row,
                            row_spacing,
                            d,
                        )))
                        .child(entity.clone());
                    let row = if self.view_mode == super::ViewMode::Rendered {
                        let row_editor = editor.clone();
                        let entity_id = entity.entity_id();
                        row.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                            let _ = row_editor.update(cx, |editor, cx| {
                                editor
                                    .on_block_context_menu_mouse_down(entity_id, event, window, cx);
                            });
                        })
                    } else {
                        row
                    };
                    group_children.push(row.into_any_element());
                    previous_callout_row = Some(row_spacing);
                    group_end += 1;
                }

                let (accent, background) = callout_colors(callout_variant, &theme);
                row_starts.push(index);
                row_top_gaps.push(top_gap);
                row_elements.push(
                    div()
                        .w(px(centered_width))
                        .max_w(relative(1.0))
                        .flex_shrink_0()
                        .mt(px(top_gap))
                        .flex()
                        .flex_col()
                        .gap(px(0.0))
                        .px(px(d.callout_padding_x))
                        .py(px(d.callout_padding_y))
                        .rounded(px(d.callout_radius))
                        .border_l(px(d.callout_border_width))
                        .border_color(accent)
                        .bg(background)
                        .children(group_children)
                        .into_any_element(),
                );
                previous_row_spacing = Some(spacing_for(group_end - 1));
                index = group_end;
                continue;
            }

            if let Some(footnote_anchor) = first_spacing.footnote_anchor {
                let mut group_children = Vec::new();
                let mut group_end = index;
                let mut previous_footnote_row = None;
                while group_end < visible_blocks.len()
                    && spacing_for(group_end).footnote_anchor == Some(footnote_anchor)
                {
                    let row_spacing = spacing_for(group_end);
                    let entity = visible_blocks[group_end].entity.clone();
                    let row = div()
                        .w_full()
                        .flex_shrink_0()
                        .mt(px(footnote_row_top_gap(previous_footnote_row, d.block_gap)))
                        .child(entity.clone());
                    let row = if self.view_mode == super::ViewMode::Rendered {
                        let row_editor = editor.clone();
                        let entity_id = entity.entity_id();
                        row.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                            let _ = row_editor.update(cx, |editor, cx| {
                                editor
                                    .on_block_context_menu_mouse_down(entity_id, event, window, cx);
                            });
                        })
                    } else {
                        row
                    };
                    group_children.push(row.into_any_element());
                    previous_footnote_row = Some(row_spacing);
                    group_end += 1;
                }

                row_starts.push(index);
                row_top_gaps.push(top_gap);
                row_elements.push(
                    div()
                        .w(px(centered_width))
                        .max_w(relative(1.0))
                        .flex_shrink_0()
                        .mt(px(top_gap))
                        .child(footnote_group_shell(group_children, &theme, d))
                        .into_any_element(),
                );
                previous_row_spacing = Some(spacing_for(group_end - 1));
                index = group_end;
                continue;
            }

            let entity = first_visible.entity.clone();
            let entity_id_for_capture = entity.entity_id();
            let editor_for_capture = editor.clone();
            let row = div()
                .w(px(centered_width))
                .max_w(relative(1.0))
                .flex_shrink_0()
                .mt(px(top_gap))
                // A zero-size canvas placed as the first child captures the
                // row's painted content top in screen space during prepaint.
                // This is authoritative for the row's real rendered height —
                // critical for HtmlBlock, whose rendered form (with images) is
                // far taller than its `last_bounds` (which describes the
                // focused raw-source form). Without this, `row_stride_cache`
                // underestimates HTML rows and every row after them is
                // misplaced by windowing/find-navigation. Stride is derived
                // from adjacent rows' captured tops, so it reflects the true
                // painted gap (including the full image height).
                .child(canvas(
                    move |_, _, _| (),
                    move |bounds, _, _window, _cx| {
                        let _ = editor_for_capture.update(_cx, |editor, _cx| {
                            editor
                                .row_painted_tops
                                .insert(entity_id_for_capture, f32::from(bounds.top()));
                        });
                    },
                ))
                .child(entity.clone());
            let row = if self.view_mode == super::ViewMode::Rendered {
                let row_editor = editor.clone();
                let entity_id = entity.entity_id();
                row.on_mouse_down(MouseButton::Right, move |event, window, cx| {
                    let _ = row_editor.update(cx, |editor, cx| {
                        editor.on_block_context_menu_mouse_down(entity_id, event, window, cx);
                    });
                })
            } else {
                row
            };
            row_starts.push(index);
            row_top_gaps.push(top_gap);
            row_elements.push(row.into_any_element());
            previous_row_spacing = Some(first_spacing);
            index += 1;
        }

        // The focused row is always kept mounted so its caret is not blurred; a
        // table cell maps to its containing table block's row.
        let focus_row = self
            .focused_edit_target_entity_id(window, cx)
            .and_then(|id| {
                self.document.visible_index_for_entity_id(id).or_else(|| {
                    self.table_cell_binding(id).and_then(|binding| {
                        self.document
                            .visible_index_for_entity_id(binding.table_block.entity_id())
                    })
                })
            })
            .map(|visible_index| {
                row_starts
                    .partition_point(|&start| start <= visible_index)
                    .saturating_sub(1)
            });

        // A row's first block keys its cached height; its painted top (from last
        // frame) feeds the footprints below. Prefer the canvas-captured
        // `row_painted_tops` (authoritative for the row's real rendered height,
        // including rendered HtmlBlock images) and fall back to the block's own
        // `last_bounds` for rows that didn't paint a canvas last frame.
        let row_first_ids: Vec<EntityId> = row_starts
            .iter()
            .map(|&start| visible_blocks[start].entity.entity_id())
            .collect();
        let row_tops: Vec<Option<f32>> = row_starts
            .iter()
            .map(|&start| {
                let entity_id = visible_blocks[start].entity.entity_id();
                if let Some(top) = self.row_painted_tops.get(&entity_id).copied() {
                    return Some(top);
                }
                visible_blocks[start]
                    .entity
                    .read_with(cx, |block, _cx| block.last_bounds)
                    .map(|bounds| f32::from(bounds.top()))
            })
            .collect();

        // On a structural edit the row indices no longer match last frame, so the
        // cache refresh below is skipped; its block-keyed entries still hold.
        let structural_change = visible_blocks.len() != self.prev_visible_block_ids.len()
            || visible_blocks
                .iter()
                .zip(&self.prev_visible_block_ids)
                .any(|(visible, prev)| visible.entity.entity_id() != *prev);
        if structural_change {
            self.prev_visible_block_ids = visible_blocks
                .iter()
                .map(|v| v.entity.entity_id())
                .collect();
            // Bound the stride cache against block churn. The live set only
            // changes on a structural edit, so eviction only needs to run here
            // — not every frame. ("live" = every document block, not the
            // mounted window: keying eviction on the narrow viewport window
            // would discard every measured row the moment the viewport scrolls
            // into fresh territory, collapsing the median stride estimate and
            // dropping the visible band into a gap — the "black editor" bug.)
            if self.row_stride_cache.len() > self.prev_visible_block_ids.len().saturating_mul(2) {
                let live_row_ids: std::collections::HashSet<EntityId> =
                    self.prev_visible_block_ids.iter().copied().collect();
                self.row_stride_cache
                    .retain(|id, _| live_row_ids.contains(id));
            }
        }

        // Rows mounted together last frame shared one scroll offset, so their
        // adjacent painted-top differences are scroll-free heights. Caching those,
        // not raw positions, is what keeps the window stable while scrolling.
        if !structural_change {
            if let Some((prev_start, prev_end)) = self.prev_render_window {
                let prev_end = prev_end.min(row_first_ids.len());
                for row in prev_start..prev_end.saturating_sub(1) {
                    if let (Some(top), Some(next_top)) = (row_tops[row], row_tops[row + 1]) {
                        let stride = next_top - top;
                        if stride > 0.0 && stride.is_finite() {
                            self.row_stride_cache.insert(row_first_ids[row], stride);
                        }
                    }
                }
            }
        }

        // Unmeasured rows fall back to an estimate. Two failure modes, both seen
        // in production as the "scroll into a black/blank editor" bug:
        //
        //   • Estimate too LARGE: the stride accumulator runs ahead of the real
        //     content, so the total content height (`top_h + mounted + bottom_h`)
        //     balloons and `max_offset` (the scroll range) inflates far beyond the
        //     real document. Scrolling to a scrollbar position that maps into that
        //     phantom space leaves the viewport on a spacer (editor background =
        //     dark) with no row mounted there → black screen. The scrollbar thumb
        //     looks normal because the offset itself is valid within the inflated
        //     range.
        //
        //   • Estimate too SMALL: the accumulator lags behind real content during
        //     a fast scroll, briefly leaving a gap between rows that re-measures
        //     on the next frame — a transient jitter, far less severe than black.
        //
        // Of the two, a too-large estimate is catastrophic (persistent black until
        // the rows in the phantom region get measured, which never happens because
        // the window is computed against the inflated positions). So the estimate
        // must be a strict LOWER BOUND on a typical row: never larger than what a
        // short paragraph actually measures. We use the median of the *measured*
        // rows, but only of those that are "short" — the document's many paragraph
        // rows — never letting a few towering code/table blocks inflate it. When
        // nothing is measured yet, fall back to a small multiple of the minimum
        // block height (a guaranteed lower bound). Measured rows always keep their
        // true stride, so the total height converges to the real value as the user
        // scrolls and more rows are learned — the inflated range collapses as rows
        // get measured, and the scrollbar never reaches phantom space.
        let measured_strides: Vec<f32> = self
            .row_stride_cache
            .values()
            .copied()
            .filter(|s| *s > 0.0 && s.is_finite())
            .collect();
        // A short row is one at or below ~3× the minimum block height (a typical
        // 1-3 line paragraph). The estimate uses the median of short rows only;
        // tall blocks (code fences, tables) are excluded so they can't inflate it.
        let short_ceiling = d.block_min_height.max(1.0) * 3.0;
        let short_strides: Vec<f32> = measured_strides
            .iter()
            .copied()
            .filter(|s| *s <= short_ceiling)
            .collect();
        let estimated_stride = if short_strides.is_empty() {
            // No short row measured yet — use the conservative lower bound. This
            // is intentionally small: under-estimating makes the scroll range too
            // short (content gets clipped, never black), the safe direction.
            short_ceiling
        } else {
            let mut sorted = short_strides;
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted[sorted.len() / 2]
        };
        let strides: Vec<f32> = row_first_ids
            .iter()
            .map(|id| {
                self.row_stride_cache
                    .get(id)
                    .copied()
                    .unwrap_or(estimated_stride)
            })
            .collect();

        // For documents small enough to mount entirely, skip windowing and mount
        // every row. The virtualization's row-height estimate can be wildly wrong
        // for unmeasured rows (a document whose head is dominated by tall tables/
        // code fences ends up with an inflated total height, so the scrollbar
        // reaches "phantom space" past the real content and the viewport lands on
        // a spacer — perceived as a black screen that only fills in after enough
        // scrolling to measure those rows). Mounting all rows makes every height
        // real, so the scroll range is exact and content always paints. The cost
        // is layout of a few hundred blocks per frame, negligible for typical
        // notes; very large documents still window.
        const FULL_MOUNT_ROW_LIMIT: usize = 400;
        let render_window = if row_first_ids.len() <= FULL_MOUNT_ROW_LIMIT {
            RenderWindow {
                run_start: 0,
                run_end: row_first_ids.len(),
                top_h: 0.0,
                bottom_h: 0.0,
            }
        } else {
            Self::rendered_window(
                &strides,
                current_scroll_y,
                viewport_height,
                RENDER_OVERDRAW_PX,
                focus_row,
            )
        };
        self.prev_render_window = Some((render_window.run_start, render_window.run_end));

        // The first mounted row re-applies its `mt`, so drop it from the top
        // spacer to avoid shifting content down by a gap.
        let top_h = match row_top_gaps.get(render_window.run_start) {
            Some(gap) => (render_window.top_h - gap).max(0.0),
            None => render_window.top_h,
        };
        let mut block_rows: Vec<AnyElement> =
            Vec::with_capacity(render_window.run_end - render_window.run_start + 2);
        if top_h > 0.5 {
            block_rows.push(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .h(px(top_h))
                    // The spacers stand in for culled (unmounted) rows. Without
                    // an explicit background a windowing miss leaves them
                    // transparent, letting the underlying window surface
                    // (often the default black clear) show through — perceived
                    // as the editor "going black" during a fast scroll. Paint
                    // them with the editor background so a miss reads as a
                    // same-colour blank that the next frame's re-measure fills.
                    .bg(theme.colors.editor_background)
                    .into_any_element(),
            );
        }
        for (row_index, element) in row_elements.into_iter().enumerate() {
            if row_index >= render_window.run_start && row_index < render_window.run_end {
                block_rows.push(element);
            }
        }
        if render_window.bottom_h > 0.5 {
            block_rows.push(
                div()
                    .w_full()
                    .flex_shrink_0()
                    .h(px(render_window.bottom_h))
                    .bg(theme.colors.editor_background)
                    .into_any_element(),
            );
        }

        let scroll_content = div()
            .id("editor-scroll-inner")
            .flex()
            .flex_col()
            .flex_grow(1.0)
            .h_full()
            .items_center()
            .bg(theme.colors.editor_background)
            .overflow_y_scroll()
            .scrollbar_width(px(0.0))
            .track_scroll(&self.scroll_handle)
            .on_hover(cx.listener(Self::on_editor_hover))
            .capture_any_mouse_down(cx.listener(Self::on_editor_capture_mouse_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_editor_mouse_down))
            .on_mouse_move(cx.listener(Self::on_editor_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_editor_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_editor_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_editor_scroll_wheel))
            .p(px(d.editor_padding))
            .pb(px(d.editor_padding
                + scroll_trigger_padding
                + scroll_beyond_bottom))
            .children(block_rows);
        let scroll_content = if self.view_mode == super::ViewMode::Rendered {
            scroll_content.on_mouse_down(
                MouseButton::Right,
                cx.listener(Self::on_editor_context_menu_mouse_down),
            )
        } else {
            scroll_content
        };

        let content_area = div()
            .id("editor-scroll")
            .w_full()
            .h_full()
            .flex_1()
            .min_w(px(0.0))
            .bg(theme.colors.editor_background)
            .relative()
            .child(scroll_content);

        let content_area = if show_custom_scrollbar {
            let scrollbar_editor = editor.clone();
            let track_origin_y = f32::from(viewport_bounds.origin.y);
            content_area.child(
                div()
                    .id("editor-scrollbar-thumb")
                    .absolute()
                    .occlude()
                    .top(px(thumb_top))
                    .right(px(d.scrollbar_right))
                    .w(px(d.scrollbar_width))
                    .h(px(thumb_height))
                    .rounded(px(999.0))
                    .bg(theme.colors.scrollbar_thumb)
                    .cursor_pointer()
                    .on_hover(cx.listener(Self::on_editor_hover))
                    .on_mouse_down(MouseButton::Left, move |event, _window, cx| {
                        let pointer_offset_y =
                            f32::from(event.position.y) - track_origin_y - thumb_top;
                        let _ = scrollbar_editor.update(cx, |editor, cx| {
                            cx.stop_propagation();
                            editor.start_scrollbar_drag(
                                pointer_offset_y,
                                track_height,
                                thumb_height,
                                max_scroll_y,
                                cx,
                            );
                        });
                    })
                    .child(
                        canvas(
                            |_, _, _| (),
                            move |_thumb_bounds, _, window, _| {
                                window.on_mouse_event({
                                    let editor = editor.clone();
                                    move |_event: &MouseUpEvent, phase, _window, cx| {
                                        if !phase.bubble() {
                                            return;
                                        }
                                        let _ = editor.update(cx, |editor, cx| {
                                            editor.end_scrollbar_drag(cx);
                                        });
                                    }
                                });

                                window.on_mouse_event({
                                    let editor = editor.clone();
                                    move |event: &MouseMoveEvent, phase, _window, cx| {
                                        if !phase.bubble() || !event.dragging() {
                                            return;
                                        }

                                        let pointer_y_in_track =
                                            f32::from(event.position.y) - track_origin_y;
                                        let _ = editor.update(cx, |editor, cx| {
                                            editor.update_scrollbar_drag(pointer_y_in_track, cx);
                                        });
                                    }
                                });
                            },
                        )
                        .size_full(),
                    ),
            )
        } else {
            content_area
        };

        // Repaint when the Cmd/Ctrl follow modifier toggles so a hovered link's
        // hand cursor updates without moving the pointer. `ModifiersChanged` is
        // dispatched along the focused element's path to the root, and this root
        // is an ancestor of every block, so one listener here covers a link in any
        // block while editing. Gated to the secondary modifier so Shift during
        // selection does not repaint.
        let follow_modifier_active = window.modifiers().secondary();

        let base = div()
            .w_full()
            .h_full()
            .flex()
            .flex_col()
            .relative()
            .bg(theme.colors.editor_background)
            .font(editor_text_font())
            .on_modifiers_changed(move |event, window, _| {
                if event.modifiers.secondary() != follow_modifier_active {
                    window.refresh();
                }
            })
            .capture_action(cx.listener(Self::on_copy_capture))
            .capture_action(cx.listener(Self::on_cut_capture))
            .capture_action(cx.listener(Self::on_delete_capture))
            .capture_action(cx.listener(Self::on_delete_back_capture))
            .capture_key_down(cx.listener(Self::on_editor_key_down_capture))
            .can_drop(|dragged, _window, _cx| dragged.is::<ExternalPaths>())
            .on_drop::<ExternalPaths>(cx.listener(Self::on_external_paths_drop))
            .on_action(cx.listener(Self::on_undo))
            .on_action(cx.listener(Self::on_redo))
            .on_action(cx.listener(Self::on_save_document))
            .on_action(cx.listener(Self::on_save_document_as))
            .on_action(cx.listener(Self::on_export_html))
            .on_action(cx.listener(Self::on_export_pdf))
            .on_action(cx.listener(Self::on_quit_application))
            .on_action(cx.listener(Self::on_close_window))
            .on_action(cx.listener(Self::on_toggle_view_mode_action))
            .on_action(cx.listener(Self::on_toggle_workspace_action))
            .on_action(cx.listener(Self::on_insert_horizontal_rule))
            .on_action(cx.listener(Self::on_insert_table))
            .on_action(cx.listener(Self::on_delete_table_shortcut))
            .on_action(cx.listener(Self::on_insert_image))
            .on_action(cx.listener(Self::on_insert_media))
            .on_action(cx.listener(Self::on_find))
            .on_action(cx.listener(Self::on_find_next))
            .on_action(cx.listener(Self::on_find_previous))
            .on_action(cx.listener(Self::on_replace))
            .on_action(cx.listener(Self::on_replace_all))
            .on_action(cx.listener(Self::on_page_up))
            .on_action(cx.listener(Self::on_page_down))
            .on_action(cx.listener(Self::on_jump_to_top))
            .on_action(cx.listener(Self::on_jump_to_bottom))
            .on_action(cx.listener(Self::on_dismiss_transient_ui))
            .on_action(cx.listener(Self::on_install_cli_tool))
            .on_action(cx.listener(Self::on_uninstall_cli_tool));
        // Fetch menus + collect labels once for both renderers; previously each
        // of render_in_window_menu_bar / render_in_window_menu_panel called
        // cx.get_menus() and walked menus.iter().map(|m| m.name.to_string())
        // independently — two redundant Vec<OwnedMenu> + two redundant
        // Vec<String>-of-N-allocations per frame.
        let menus = supports_in_window_menu()
            .then(|| cx.get_menus())
            .flatten()
            .filter(|m| !m.is_empty());
        let menu_labels: Vec<SharedString> = menus
            .as_ref()
            .map(|m| m.iter().map(|menu| menu.name.clone()).collect())
            .unwrap_or_default();
        let window_title =
            Self::window_title(self.file_path.as_deref(), self.document_dirty, &strings);
        let base = if let Some(titlebar) = render_custom_titlebar(
            "editor-titlebar",
            window_title.into(),
            &theme,
            window,
            cx,
            Self::on_titlebar_close,
        ) {
            base.child(titlebar)
        } else {
            base
        };
        let base = if let Some(menu_bar) = self.render_in_window_menu_bar(
            &theme,
            cx,
            menus.as_deref(),
            &menu_labels,
            titlebar_height,
        ) {
            base.child(menu_bar)
        } else {
            base
        };
        let main_content = div()
            .w_full()
            .flex_1()
            .min_h(px(0.0))
            .pt(px(titlebar_height + menu_bar_height))
            .flex()
            .min_w(px(0.0));
        let main_content =
            if let Some(workspace_panel) = self.render_workspace_panel(&theme, &strings, cx) {
                // Resizable workspace drawer: drag the right edge to resize.
                let saved = crate::state::persistence::load_layout()
                    .and_then(|l| l.markdown_drawer_width)
                    .filter(|&w| w > 0.0)
                    .map(gpui::px)
                    .unwrap_or_else(|| gpui::px(240.));
                let state = self.workspace_resize.clone();
                main_content.child(
                    gpui_component::resizable::h_resizable("editor-workspace")
                        .with_state(&state)
                        .on_resize(|state, _, cx| {
                            if let Some(w) = state.read(cx).sizes().first() {
                                let mut layout =
                                    crate::state::persistence::load_layout().unwrap_or_default();
                                layout.markdown_drawer_width = Some(w.as_f32());
                                let _ = crate::state::persistence::save_layout(&layout);
                            }
                        })
                        .child(
                            gpui_component::resizable::resizable_panel()
                                .size(saved)
                                .size_range(gpui::px(180.)..gpui::px(520.))
                                .child(workspace_panel),
                        )
                        .child(
                            gpui_component::resizable::resizable_panel()
                                .overflow_hidden()
                                .child(content_area),
                        ),
                )
            } else if self.owns_window {
                // Workspace drawer is closed and the editor owns its window
                // (standalone mode, no host title bar): show a thin expand strip so
                // the user can reopen the outline. Mirrors the Notes panel's
                // collapsed-list strip (same width, border, bg, button).
                //
                // In the side-view (`show_chrome && !owns_window`) the host title
                // bar already provides the outline toggle, so we collapse fully —
                // no residual border, no redundant expand button.
                let app = cx.theme();
                let strip_border = app.border;
                let strip_bg = app.muted.opacity(0.3);
                main_content
                    .child(
                        div()
                            .w(px(28.))
                            .h_full()
                            .flex_shrink_0()
                            .border_r_1()
                            .border_color(strip_border)
                            .bg(strip_bg)
                            .items_center()
                            .pt_2()
                            .child(
                                gpui_component::button::Button::new("editor-outline-expand")
                                    .ghost()
                                    .xsmall()
                                    .icon(gpui_component::IconName::PanelLeftOpen)
                                    .tooltip(strings.workspace_tab_outline.clone())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_workspace_drawer(window, cx);
                                    })),
                            ),
                    )
                    .child(content_area)
            } else {
                main_content.child(content_area)
            };
        let base = base.child(main_content);
        // The status bar (word count, view-mode switch, cursor position,
        // sidebar toggle) is rendered when `show_chrome` is set — i.e. for the
        // standalone markdown side-view. The notes panel disables it (it has
        // its own header) and a window-owning editor always shows it.
        let base = if self.show_chrome {
            if let Some(status_bar) = self.render_status_bar(&theme, &strings, window, cx) {
                base.child(status_bar)
            } else {
                base
            }
        } else {
            base
        };
        let base = if let Some(menu_panel) = self.render_in_window_menu_panel(
            &theme,
            cx,
            menus.as_deref(),
            &menu_labels,
            titlebar_height,
            f32::from(window.viewport_size().height.max(px(1.0))),
        ) {
            base.child(menu_panel)
        } else {
            base
        };
        let base = if let Some(context_menu) = self.render_context_menu_overlay(&theme, cx) {
            base.child(context_menu)
        } else {
            base
        };
        let base = if let Some(table_dialog) = self.render_table_insert_dialog_overlay(&theme, cx) {
            base.child(table_dialog)
        } else {
            base
        };
        let base = if let Some(image_dialog) = self.render_image_insert_dialog_overlay(&theme, cx) {
            base.child(image_dialog)
        } else {
            base
        };
        let base = if let Some(find_bar) = self.render_find_bar(&theme, cx) {
            base.child(find_bar)
        } else {
            base
        };
        if let Some(kind) = self.info_dialog {
            base.child(self.render_info_dialog_overlay(&theme, kind, cx))
        } else if self.show_drop_replace_dialog {
            base.child(self.render_drop_replace_overlay(&theme, cx))
        } else if self.show_unsaved_changes_dialog {
            base.child(self.render_unsaved_changes_overlay(&theme, cx))
        } else {
            base
        }
    }
}

#[cfg(all(test, feature = "wysiwyg-gpui-tests"))]
mod tests {
    use super::{
        NoRecentFiles, RenderedRowSpacingInfo, callout_row_top_gap, editor_text_font,
        import_menu_split_index, in_window_menu_bar_height_for_target_os, menu_bar_button_width,
        menu_items_visual_height_with_gaps, menu_panel_left, menu_panel_width_for_labels,
        owned_menu_item_labels, rendered_row_top_gap, scrollable_import_menu_scroll_height,
        submenu_bridge_geometry, supports_in_window_menu_for_target_os,
        tibetan_font_fallbacks_for_target_os,
    };
    use crate::wysiwyg::components::{AddLanguageConfig, AddThemeConfig};
    use crate::wysiwyg::theme::Theme;
    use gpui::{OwnedMenu, OwnedMenuItem};
    use uuid::Uuid;

    fn disabled_menu_action(name: &str) -> OwnedMenuItem {
        OwnedMenuItem::Action {
            name: name.into(),
            action: Box::new(NoRecentFiles),
            os_action: None,
            checked: false,
            disabled: true,
        }
    }

    fn add_theme_menu_action() -> OwnedMenuItem {
        OwnedMenuItem::Action {
            name: "Add Theme Config".into(),
            action: Box::new(AddThemeConfig),
            os_action: None,
            checked: false,
            disabled: false,
        }
    }

    fn add_language_menu_action() -> OwnedMenuItem {
        OwnedMenuItem::Action {
            name: "Add Language Config".into(),
            action: Box::new(AddLanguageConfig),
            os_action: None,
            checked: false,
            disabled: false,
        }
    }

    #[test]
    fn contiguous_quote_rows_collapse_inter_row_gap() {
        let group = Uuid::new_v4();
        let gap = rendered_row_top_gap(
            Some(RenderedRowSpacingInfo {
                quote_group_anchor: Some(group),
                ..RenderedRowSpacingInfo::default()
            }),
            RenderedRowSpacingInfo {
                quote_group_anchor: Some(group),
                ..RenderedRowSpacingInfo::default()
            },
            4.0,
        );
        assert_eq!(gap, 0.0);
    }

    #[test]
    fn editor_text_font_keeps_system_ui_as_primary_family() {
        assert_eq!(editor_text_font().family.to_string(), ".SystemUIFont");
    }

    #[test]
    fn tibetan_font_fallbacks_prioritize_platform_defaults() {
        assert_eq!(
            tibetan_font_fallbacks_for_target_os("windows")
                .first()
                .map(String::as_str),
            Some("Microsoft Himalaya")
        );
        assert_eq!(
            tibetan_font_fallbacks_for_target_os("macos")
                .first()
                .map(String::as_str),
            Some("Kailasa")
        );
        assert_eq!(
            tibetan_font_fallbacks_for_target_os("linux")
                .first()
                .map(String::as_str),
            Some("Noto Serif Tibetan")
        );
        assert_eq!(
            tibetan_font_fallbacks_for_target_os("unknown")
                .first()
                .map(String::as_str),
            Some("Noto Serif Tibetan")
        );
    }

    #[test]
    fn nested_quote_separator_row_keeps_outer_group_gap_collapsed() {
        let group = Uuid::new_v4();
        let gap = rendered_row_top_gap(
            Some(RenderedRowSpacingInfo {
                quote_group_anchor: Some(group),
                ..RenderedRowSpacingInfo::default()
            }),
            RenderedRowSpacingInfo {
                quote_group_anchor: Some(group),
                ..RenderedRowSpacingInfo::default()
            },
            4.0,
        );
        assert_eq!(gap, 0.0);
    }

    #[test]
    fn distinct_quote_groups_keep_default_gap() {
        let gap = rendered_row_top_gap(
            Some(RenderedRowSpacingInfo {
                quote_group_anchor: Some(Uuid::new_v4()),
                ..RenderedRowSpacingInfo::default()
            }),
            RenderedRowSpacingInfo {
                quote_group_anchor: Some(Uuid::new_v4()),
                ..RenderedRowSpacingInfo::default()
            },
            4.0,
        );
        assert_eq!(gap, 4.0);
    }

    #[test]
    fn non_quote_rows_keep_default_gap() {
        let gap = rendered_row_top_gap(
            Some(RenderedRowSpacingInfo {
                quote_group_anchor: None,
                ..RenderedRowSpacingInfo::default()
            }),
            RenderedRowSpacingInfo {
                quote_group_anchor: Some(Uuid::new_v4()),
                ..RenderedRowSpacingInfo::default()
            },
            4.0,
        );
        assert_eq!(gap, 4.0);
    }

    #[test]
    fn callout_inner_spacing_uses_header_and_body_tokens() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;

        let header_gap = callout_row_top_gap(
            Some(RenderedRowSpacingInfo {
                is_callout_header: true,
                ..RenderedRowSpacingInfo::default()
            }),
            RenderedRowSpacingInfo::default(),
            dimensions,
        );
        let body_gap = callout_row_top_gap(
            Some(RenderedRowSpacingInfo {
                is_callout_header: false,
                ..RenderedRowSpacingInfo::default()
            }),
            RenderedRowSpacingInfo::default(),
            dimensions,
        );

        assert_eq!(header_gap, dimensions.callout_header_margin_bottom);
        assert_eq!(body_gap, dimensions.callout_body_gap);
    }

    #[test]
    fn nested_quote_rows_inside_callout_collapse_body_gap() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;
        let group = Uuid::new_v4();

        let gap = callout_row_top_gap(
            Some(RenderedRowSpacingInfo {
                is_callout_header: false,
                visible_quote_group_anchor: Some(group),
                ..RenderedRowSpacingInfo::default()
            }),
            RenderedRowSpacingInfo {
                visible_quote_group_anchor: Some(group),
                ..RenderedRowSpacingInfo::default()
            },
            dimensions,
        );

        assert_eq!(gap, 0.0);
    }

    #[test]
    fn menu_button_width_expands_for_long_ascii_labels() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;

        assert_eq!(
            menu_bar_button_width("文件", dimensions),
            dimensions.menu_bar_button_width
        );
        assert!(menu_bar_button_width("Language", dimensions) > dimensions.menu_bar_button_width);
    }

    #[test]
    fn in_window_menu_is_enabled_for_every_target_except_macos() {
        for target_os in [
            "windows",
            "linux",
            "freebsd",
            "openbsd",
            "netbsd",
            "dragonfly",
            "solaris",
            "illumos",
            "android",
            "unknown",
        ] {
            assert!(
                supports_in_window_menu_for_target_os(target_os),
                "{target_os} should use the in-window fallback menu"
            );
        }
        assert!(!supports_in_window_menu_for_target_os("macos"));
    }

    #[test]
    fn in_window_menu_height_depends_on_platform_and_menu_presence() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;

        assert_eq!(
            in_window_menu_bar_height_for_target_os("linux", true, dimensions),
            dimensions.menu_bar_height
        );
        assert_eq!(
            in_window_menu_bar_height_for_target_os("windows", true, dimensions),
            dimensions.menu_bar_height
        );
        assert_eq!(
            in_window_menu_bar_height_for_target_os("linux", false, dimensions),
            0.0
        );
        assert_eq!(
            in_window_menu_bar_height_for_target_os("macos", true, dimensions),
            0.0
        );
    }

    #[test]
    fn menu_panel_left_uses_accumulated_dynamic_button_widths() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;
        let labels = vec![
            "File".to_string(),
            "Language".to_string(),
            "Theme".to_string(),
            "Help".to_string(),
        ];

        let left = menu_panel_left(2, &labels, dimensions);
        let expected = dimensions.menu_bar_padding_x
            + menu_bar_button_width("File", dimensions)
            + dimensions.menu_bar_gap
            + menu_bar_button_width("Language", dimensions)
            + dimensions.menu_bar_gap;
        let old_fixed_left = dimensions.menu_bar_padding_x
            + 2.0 * (dimensions.menu_bar_button_width + dimensions.menu_bar_gap);

        assert_eq!(left, expected);
        assert!(left > old_fixed_left);
    }

    #[test]
    fn menu_panel_width_expands_for_long_recent_paths() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;
        let short_labels = vec!["Save".to_string()];
        let long_labels = vec![r"C:\Users\someone\Documents\Very Long Folder\notes.md".to_string()];

        assert_eq!(
            menu_panel_width_for_labels(&short_labels, dimensions),
            dimensions.menu_panel_width
        );
        assert!(
            menu_panel_width_for_labels(&long_labels, dimensions) > dimensions.menu_panel_width
        );
    }

    #[test]
    fn import_menu_split_detects_theme_and_language_import_tails() {
        let theme_items = vec![
            disabled_menu_action("Velotype"),
            OwnedMenuItem::Separator,
            add_theme_menu_action(),
        ];
        let language_items = vec![
            disabled_menu_action("English"),
            OwnedMenuItem::Separator,
            add_language_menu_action(),
        ];
        let regular_items = vec![
            disabled_menu_action("Open"),
            OwnedMenuItem::Separator,
            disabled_menu_action("Save"),
        ];
        let malformed_import_items =
            vec![disabled_menu_action("Velotype"), add_theme_menu_action()];

        assert_eq!(import_menu_split_index(&theme_items), Some(1));
        assert_eq!(import_menu_split_index(&language_items), Some(1));
        assert_eq!(import_menu_split_index(&regular_items), None);
        assert_eq!(import_menu_split_index(&malformed_import_items), None);
    }

    #[test]
    fn scrollable_import_menu_height_caps_visible_items_and_clamps_to_viewport() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;
        let scroll_items = (0..20)
            .map(|index| disabled_menu_action(&format!("Custom Theme {index}")))
            .collect::<Vec<_>>();
        let footer_items = vec![OwnedMenuItem::Separator, add_theme_menu_action()];
        let expected_large_height =
            menu_items_visual_height_with_gaps(&scroll_items[..12], dimensions);
        let full_scroll_content_height =
            menu_items_visual_height_with_gaps(&scroll_items, dimensions);
        let footer_height = menu_items_visual_height_with_gaps(&footer_items, dimensions);

        let large_height = scrollable_import_menu_scroll_height(
            &scroll_items,
            &footer_items,
            2000.0,
            0.0,
            dimensions,
        );
        let small_height = scrollable_import_menu_scroll_height(
            &scroll_items,
            &footer_items,
            180.0,
            0.0,
            dimensions,
        );

        assert!((large_height - expected_large_height).abs() < f32::EPSILON);
        assert!(full_scroll_content_height > large_height);
        assert!(large_height < expected_large_height + footer_height);
        assert!(small_height < large_height);
        assert!(small_height >= dimensions.menu_item_height);
    }

    #[test]
    fn submenu_bridge_spans_parent_child_menu_gap() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;
        let labels = vec!["File".to_string()];
        let items = vec![
            OwnedMenuItem::Separator,
            OwnedMenuItem::Submenu(OwnedMenu {
                name: "Recent".into(),
                items: vec![OwnedMenuItem::Action {
                    name: r"C:\Users\someone\Documents\notes.md".into(),
                    action: Box::new(NoRecentFiles),
                    os_action: None,
                    checked: false,
                    disabled: false,
                }],
                disabled: false,
            }),
        ];
        let submenu_labels = match &items[1] {
            OwnedMenuItem::Submenu(submenu) => owned_menu_item_labels(&submenu.items),
            _ => Vec::new(),
        };

        let bridge = submenu_bridge_geometry(0, &labels, &items, 1, &submenu_labels, dimensions)
            .expect("submenu bridge geometry should be available");
        let submenu_width = menu_panel_width_for_labels(&submenu_labels, dimensions);

        assert_eq!(
            bridge.left,
            dimensions.menu_bar_padding_x + dimensions.menu_panel_width
        );
        assert_eq!(bridge.width, dimensions.menu_panel_gap + submenu_width);
        assert!(bridge.height > dimensions.menu_item_height);
        let item_top = dimensions.menu_panel_top
            + dimensions.menu_panel_padding
            + dimensions.menu_separator_height
            + dimensions.menu_separator_margin_y * 2.0
            + dimensions.menu_panel_gap;
        assert!(bridge.top < item_top);
        assert!(bridge.top >= dimensions.menu_panel_top);
    }

    #[test]
    fn submenu_bridge_uses_dynamic_main_menu_width() {
        let theme = Theme::default_theme();
        let dimensions = &theme.dimensions;
        let labels = vec!["File".to_string()];
        let items = vec![OwnedMenuItem::Submenu(OwnedMenu {
            name: "Open Recently Used Markdown File".into(),
            items: vec![OwnedMenuItem::Action {
                name: r"C:\Users\someone\Documents\Very Long Folder\notes.md".into(),
                action: Box::new(NoRecentFiles),
                os_action: None,
                checked: false,
                disabled: false,
            }],
            disabled: false,
        })];
        let submenu_labels = match &items[0] {
            OwnedMenuItem::Submenu(submenu) => owned_menu_item_labels(&submenu.items),
            _ => Vec::new(),
        };

        let bridge = submenu_bridge_geometry(0, &labels, &items, 0, &submenu_labels, dimensions)
            .expect("submenu bridge geometry should be available");

        assert!(bridge.left > dimensions.menu_bar_padding_x + dimensions.menu_panel_width);
        assert!(bridge.width > dimensions.menu_panel_gap + dimensions.menu_panel_width);
    }
}
