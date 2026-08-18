//! The help-center window: a searchable, comprehensive reference covering
//! every feature view and keyboard shortcut in the app.
//!
//! The editor-shortcut table is derived from the same code path that
//! registers the real bindings (`effective_shortcuts`, fed with the user's
//! Settings → Shortcuts overrides), so what is documented is what is
//! actually pressed — including custom keybindings and per-platform key
//! display.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::kbd::Kbd;
use gpui_component::{
    ActiveTheme, IconName, Sizable as _, Theme, TitleBar,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

use crate::ui::app::{SideView, VerveApp};
use crate::wysiwyg::components::ShortcutCategory;

/// A selectable section in the help-center nav.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum HelpSection {
    GettingStarted,
    Features,
    Shortcuts,
    About,
}

impl HelpSection {
    const ALL: [HelpSection; 4] = [
        HelpSection::GettingStarted,
        HelpSection::Features,
        HelpSection::Shortcuts,
        HelpSection::About,
    ];

    fn id(self) -> &'static str {
        match self {
            HelpSection::GettingStarted => "start",
            HelpSection::Features => "features",
            HelpSection::Shortcuts => "shortcuts",
            HelpSection::About => "about",
        }
    }

    fn label(self) -> String {
        match self {
            HelpSection::GettingStarted => rust_i18n::t!("help.nav_start").to_string(),
            HelpSection::Features => rust_i18n::t!("help.nav_features").to_string(),
            HelpSection::Shortcuts => rust_i18n::t!("help.nav_shortcuts").to_string(),
            HelpSection::About => rust_i18n::t!("help.nav_about").to_string(),
        }
    }
}

/// One shortcut row: translated label, display keystrokes, optional note.
struct ShortcutRow {
    label: String,
    keys: Vec<Keystroke>,
    note: Option<String>,
    /// Lowercased label + formatted keys + raw key strings, for search.
    haystack: String,
}

impl ShortcutRow {
    fn new(label: String, key_strs: Vec<String>, note: Option<String>) -> Self {
        let keys = parse_display_keys(&key_strs);
        let formatted = keys
            .iter()
            .map(|k| Kbd::format(k))
            .collect::<Vec<_>>()
            .join(" ");
        let raw = key_strs.join(" ");
        let haystack = format!("{label} {formatted} {raw}").to_lowercase();
        Self {
            label,
            keys,
            note,
            haystack,
        }
    }
}

/// One feature-guide card: a rail view plus its translated description.
struct FeatureEntry {
    view: SideView,
    desc: String,
    haystack: String,
}

/// Whether two keystrokes are the same key with the ⌘/Ctrl modifier swapped
/// (e.g. `cmd-a` vs `ctrl-a`): both are registered, but only the flavor
/// matching the current platform is displayed.
fn is_cmd_ctrl_twin(a: &Keystroke, b: &Keystroke) -> bool {
    a.key == b.key
        && a.modifiers.shift == b.modifiers.shift
        && a.modifiers.alt == b.modifiers.alt
        && a.modifiers.function == b.modifiers.function
        && a.modifiers.control != b.modifiers.control
        && a.modifiers.platform != b.modifiers.platform
}

/// Parse key strings into display keystrokes, collapsing ⌘/Ctrl twin pairs
/// down to the platform-preferred flavor. Genuinely different alternatives
/// (e.g. `cmd-g` / `f3`) are all kept.
fn parse_display_keys(key_strs: &[String]) -> Vec<Keystroke> {
    let parsed: Vec<Keystroke> = key_strs
        .iter()
        .filter_map(|s| Keystroke::parse(s).ok())
        .collect();
    let prefer_platform = cfg!(target_os = "macos");
    parsed
        .iter()
        .filter(|key| {
            let key: &Keystroke = key;
            let has_twin = parsed
                .iter()
                .any(|other| !std::ptr::eq(key, other) && is_cmd_ctrl_twin(key, other));
            if !has_twin {
                return true;
            }
            if prefer_platform {
                key.modifiers.platform
            } else {
                key.modifiers.control
            }
        })
        .cloned()
        .collect()
}

/// Translated label for an editor-shortcut id; falls back to the built-in
/// English label when the id has no locale entry, so a row never renders as
/// a raw dotted key.
///
/// Static keys only — rust-i18n 4's `t!` macro doesn't support runtime key
/// expressions in this build, so every id is matched explicitly.
fn sc_label(id: &str) -> String {
    match id {
        "newline" => rust_i18n::t!("help.sc.newline"),
        "delete_back" => rust_i18n::t!("help.sc.delete_back"),
        "delete" => rust_i18n::t!("help.sc.delete"),
        "word_delete_back" => rust_i18n::t!("help.sc.word_delete_back"),
        "word_delete_forward" => rust_i18n::t!("help.sc.word_delete_forward"),
        "focus_prev" => rust_i18n::t!("help.sc.focus_prev"),
        "focus_next" => rust_i18n::t!("help.sc.focus_next"),
        "move_left" => rust_i18n::t!("help.sc.move_left"),
        "move_right" => rust_i18n::t!("help.sc.move_right"),
        "word_move_left" => rust_i18n::t!("help.sc.word_move_left"),
        "word_move_right" => rust_i18n::t!("help.sc.word_move_right"),
        "home" => rust_i18n::t!("help.sc.home"),
        "end" => rust_i18n::t!("help.sc.end"),
        "block_up" => rust_i18n::t!("help.sc.block_up"),
        "block_down" => rust_i18n::t!("help.sc.block_down"),
        "page_up" => rust_i18n::t!("help.sc.page_up"),
        "page_down" => rust_i18n::t!("help.sc.page_down"),
        "jump_to_top" => rust_i18n::t!("help.sc.jump_to_top"),
        "jump_to_bottom" => rust_i18n::t!("help.sc.jump_to_bottom"),
        "select_left" => rust_i18n::t!("help.sc.select_left"),
        "select_right" => rust_i18n::t!("help.sc.select_right"),
        "word_select_left" => rust_i18n::t!("help.sc.word_select_left"),
        "word_select_right" => rust_i18n::t!("help.sc.word_select_right"),
        "select_home" => rust_i18n::t!("help.sc.select_home"),
        "select_end" => rust_i18n::t!("help.sc.select_end"),
        "select_all" => rust_i18n::t!("help.sc.select_all"),
        "copy" => rust_i18n::t!("help.sc.copy"),
        "cut" => rust_i18n::t!("help.sc.cut"),
        "paste" => rust_i18n::t!("help.sc.paste"),
        "paste_plain_text" => rust_i18n::t!("help.sc.paste_plain_text"),
        "undo" => rust_i18n::t!("help.sc.undo"),
        "redo" => rust_i18n::t!("help.sc.redo"),
        "bold_selection" => rust_i18n::t!("help.sc.bold_selection"),
        "italic_selection" => rust_i18n::t!("help.sc.italic_selection"),
        "underline_selection" => rust_i18n::t!("help.sc.underline_selection"),
        "strikethrough_selection" => rust_i18n::t!("help.sc.strikethrough_selection"),
        "highlight_selection" => rust_i18n::t!("help.sc.highlight_selection"),
        "superscript_selection" => rust_i18n::t!("help.sc.superscript_selection"),
        "subscript_selection" => rust_i18n::t!("help.sc.subscript_selection"),
        "code_selection" => rust_i18n::t!("help.sc.code_selection"),
        "indent_block" => rust_i18n::t!("help.sc.indent_block"),
        "outdent_block" => rust_i18n::t!("help.sc.outdent_block"),
        "exit_code_block" => rust_i18n::t!("help.sc.exit_code_block"),
        "heading_1" => rust_i18n::t!("help.sc.heading_1"),
        "heading_2" => rust_i18n::t!("help.sc.heading_2"),
        "heading_3" => rust_i18n::t!("help.sc.heading_3"),
        "heading_4" => rust_i18n::t!("help.sc.heading_4"),
        "heading_5" => rust_i18n::t!("help.sc.heading_5"),
        "heading_6" => rust_i18n::t!("help.sc.heading_6"),
        "convert_to_paragraph" => rust_i18n::t!("help.sc.convert_to_paragraph"),
        "convert_to_code_block" => rust_i18n::t!("help.sc.convert_to_code_block"),
        "convert_to_bulleted_list" => rust_i18n::t!("help.sc.convert_to_bulleted_list"),
        "convert_to_numbered_list" => rust_i18n::t!("help.sc.convert_to_numbered_list"),
        "convert_to_task_list" => rust_i18n::t!("help.sc.convert_to_task_list"),
        "convert_to_quote" => rust_i18n::t!("help.sc.convert_to_quote"),
        "insert_link" => rust_i18n::t!("help.sc.insert_link"),
        "insert_image" => rust_i18n::t!("help.sc.insert_image"),
        "insert_horizontal_rule" => rust_i18n::t!("help.sc.insert_horizontal_rule"),
        "insert_table" => rust_i18n::t!("help.sc.insert_table"),
        "insert_table_row_below" => rust_i18n::t!("help.sc.insert_table_row_below"),
        "insert_table_column_right" => rust_i18n::t!("help.sc.insert_table_column_right"),
        "delete_table" => rust_i18n::t!("help.sc.delete_table"),
        "find" => rust_i18n::t!("help.sc.find"),
        "find_next" => rust_i18n::t!("help.sc.find_next"),
        "find_previous" => rust_i18n::t!("help.sc.find_previous"),
        "replace" => rust_i18n::t!("help.sc.replace"),
        "toggle_view_mode" => rust_i18n::t!("help.sc.toggle_view_mode"),
        "toggle_workspace" => rust_i18n::t!("help.sc.toggle_workspace"),
        _ => return crate::wysiwyg::components::shortcut_label(id).to_string(),
    }
    .to_string()
}

/// Translated description for a rail view (empty string when the locale
/// entry is missing — the card then shows the label only).
fn view_desc(view: SideView) -> String {
    match view {
        SideView::Api => rust_i18n::t!("help.view_desc.api"),
        SideView::History => rust_i18n::t!("help.view_desc.history"),
        SideView::Mock => rust_i18n::t!("help.view_desc.mock"),
        SideView::ProjectManage => rust_i18n::t!("help.view_desc.projectmanage"),
        SideView::Share => rust_i18n::t!("help.view_desc.share"),
        SideView::AutoTest => rust_i18n::t!("help.view_desc.autotest"),
        SideView::Stress => rust_i18n::t!("help.view_desc.stress"),
        SideView::Proxy => rust_i18n::t!("help.view_desc.proxy"),
        SideView::K8s => rust_i18n::t!("help.view_desc.k8s"),
        SideView::Docker => rust_i18n::t!("help.view_desc.docker"),
        SideView::Ssh => rust_i18n::t!("help.view_desc.ssh"),
        SideView::Hosts => rust_i18n::t!("help.view_desc.hosts"),
        SideView::Notes => rust_i18n::t!("help.view_desc.notes"),
        SideView::MarkdownEditor => rust_i18n::t!("help.view_desc.markdowneditor"),
        SideView::JsonFormat => rust_i18n::t!("help.view_desc.jsonformat"),
        SideView::PdfEditor => rust_i18n::t!("help.view_desc.pdfeditor"),
        SideView::Relocate => rust_i18n::t!("help.view_desc.relocate"),
    }
    .to_string()
}

fn editor_group_label(category: ShortcutCategory) -> String {
    match category {
        ShortcutCategory::File => rust_i18n::t!("help.group_cat_file").to_string(),
        ShortcutCategory::Edit => rust_i18n::t!("help.group_cat_edit").to_string(),
        ShortcutCategory::Navigation => rust_i18n::t!("help.group_cat_navigation").to_string(),
        ShortcutCategory::Markdown => rust_i18n::t!("help.group_cat_markdown").to_string(),
        ShortcutCategory::Block => rust_i18n::t!("help.group_cat_block").to_string(),
        ShortcutCategory::Other => rust_i18n::t!("help.group_cat_other").to_string(),
    }
}

/// Platform-appropriate primary-modifier prefix for building global-binding
/// display strings ("cmd" on macOS, "ctrl" elsewhere).
fn primary_modifier() -> &'static str {
    if cfg!(target_os = "macos") {
        "cmd"
    } else {
        "ctrl"
    }
}

/// The app-level shortcuts registered in `main.rs` (labels describe the
/// per-view routing in the action handlers).
fn build_global_rows() -> Vec<ShortcutRow> {
    let m = primary_modifier();
    vec![
        ShortcutRow::new(
            rust_i18n::t!("help.sc_g_help").to_string(),
            vec![format!("{m}-/"), "f1".to_string()],
            None,
        ),
        ShortcutRow::new(
            rust_i18n::t!("help.sc_g_send").to_string(),
            vec![format!("{m}-enter")],
            None,
        ),
        ShortcutRow::new(
            rust_i18n::t!("help.sc_g_save").to_string(),
            vec![format!("{m}-s")],
            None,
        ),
        ShortcutRow::new(
            rust_i18n::t!("help.sc_g_new_request").to_string(),
            vec![format!("{m}-shift-n")],
            None,
        ),
        ShortcutRow::new(
            rust_i18n::t!("help.sc_g_new_file").to_string(),
            vec![format!("{m}-n")],
            None,
        ),
        ShortcutRow::new(
            rust_i18n::t!("help.sc_g_open").to_string(),
            vec![format!("{m}-o")],
            None,
        ),
        ShortcutRow::new(
            rust_i18n::t!("help.sc_g_open_new_tab").to_string(),
            vec![format!("{m}-shift-o")],
            None,
        ),
        ShortcutRow::new(
            rust_i18n::t!("help.sc_g_close").to_string(),
            vec![format!("{m}-w")],
            None,
        ),
    ]
}

/// The fixed rail-switch shortcuts (⌘/Ctrl + 1..=5) plus the Esc unlock.
fn build_rail_rows() -> Vec<ShortcutRow> {
    let m = primary_modifier();
    let mut rows: Vec<ShortcutRow> = SideView::SHORTCUT_VIEWS
        .iter()
        .enumerate()
        .map(|(i, &view)| ShortcutRow::new(view.label(), vec![format!("{m}-{}", i + 1)], None))
        .collect();
    rows.push(ShortcutRow::new(
        rust_i18n::t!("help.sc_esc").to_string(),
        vec!["escape".to_string()],
        None,
    ));
    rows
}

fn section_heading(text: String, fg: Hsla) -> Div {
    div()
        .text_base()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .child(text)
}

fn sub_heading(text: String, muted: Hsla) -> Div {
    div()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(muted)
        .child(text)
}

fn note_box(text: String, theme: &Theme) -> Div {
    div()
        .text_xs()
        .text_color(theme.muted_foreground)
        .px(px(8.))
        .py(px(5.))
        .rounded_md()
        .bg(theme.muted)
        .child(text)
}

fn render_shortcut_row(row: &ShortcutRow, theme: &Theme) -> Div {
    let mut keys = h_flex().gap_1().flex_wrap().justify_end();
    for (i, key) in row.keys.iter().enumerate() {
        if i > 0 {
            keys = keys.child(div().text_xs().text_color(theme.muted_foreground).child("/"));
        }
        keys = keys.child(Kbd::new(key.clone()));
    }
    h_flex()
        .w_full()
        .py(px(6.))
        .px(px(8.))
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .gap(px(12.))
        .items_center()
        .child(div().flex_1().min_w_0().text_sm().child(row.label.clone()))
        .child(
            v_flex()
                .gap(px(2.))
                .items_end()
                .child(keys)
                .when_some(row.note.clone(), |col, note| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(note),
                    )
                }),
        )
}

fn render_feature_card(entry: &FeatureEntry, theme: &Theme) -> Div {
    // Fixed-shortcut views show their ⌘N hint next to the label.
    let label = match SideView::SHORTCUT_VIEWS.iter().position(|&v| v == entry.view) {
        Some(i) => format!("{} ({})", entry.view.label(), crate::ui::app::rail_slot_hint(i)),
        None => entry.view.label(),
    };
    h_flex()
        .gap(px(12.))
        .items_start()
        .w_full()
        .py(px(8.))
        .px(px(10.))
        .rounded_md()
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .flex_none()
                .mt(px(2.))
                .text_color(theme.muted_foreground)
                .child(VerveApp::rail_icon_for(entry.view)),
        )
        .child(
            v_flex()
                .gap(px(2.))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.foreground)
                        .child(label),
                )
                .when(!entry.desc.is_empty(), |card| {
                    card.child(
                        div()
                            .text_sm()
                            .text_color(theme.muted_foreground)
                            .child(entry.desc.clone()),
                    )
                }),
        )
}

pub struct HelpCenter {
    /// Search query (lowercased); empty means "browse current section".
    query: String,
    search: Entity<InputState>,
    active_section: HelpSection,
    /// App-level shortcut rows.
    global_rows: Vec<ShortcutRow>,
    /// View-switching rows (⌘1..⌘5 plus the Esc unlock).
    rail_rows: Vec<ShortcutRow>,
    /// Editor shortcut rows grouped by category, in definition order.
    editor_groups: Vec<(String, Vec<ShortcutRow>)>,
    /// Feature-guide cards, in rail order.
    features: Vec<FeatureEntry>,
    _sub: gpui::Subscription,
}

impl HelpCenter {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| {
            let mut input =
                InputState::new(window, cx).placeholder(rust_i18n::t!("help.search_ph"));
            input.set_value(String::new(), window, cx);
            input
        });
        let search_sub = cx.subscribe(&search, |this, input, event: &InputEvent, cx| {
            if matches!(event, InputEvent::Change) {
                this.query = input.read(cx).value().to_lowercase();
                cx.notify();
            }
        });

        // The editor table mirrors the actually-registered bindings,
        // including the user's customizations from Settings → Shortcuts.
        let config = crate::state::persistence::load_shortcuts();
        let effective = crate::wysiwyg::components::actions::effective_shortcuts(&config);
        let mut editor_groups: Vec<(String, Vec<ShortcutRow>)> = Vec::new();
        for shortcut in &effective {
            let group = editor_group_label(shortcut.definition.category);
            let row = ShortcutRow::new(
                sc_label(shortcut.definition.id),
                shortcut.keys.clone(),
                None,
            );
            match editor_groups.iter_mut().find(|(label, _)| *label == group) {
                Some((_, rows)) => rows.push(row),
                None => editor_groups.push((group, vec![row])),
            }
        }

        let features = SideView::ALL
            .iter()
            .map(|&view| {
                let desc = view_desc(view);
                let haystack = format!("{} {}", view.label(), desc).to_lowercase();
                FeatureEntry {
                    view,
                    desc,
                    haystack,
                }
            })
            .collect();

        Self {
            query: String::new(),
            search,
            active_section: HelpSection::GettingStarted,
            global_rows: build_global_rows(),
            rail_rows: build_rail_rows(),
            editor_groups,
            features,
            _sub: search_sub,
        }
    }

    fn render_start(&self, theme: &Theme) -> Div {
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let heading = |d: Div| d.text_sm().font_weight(FontWeight::SEMIBOLD).text_color(fg).mt(px(10.));
        let para = |d: Div| d.text_sm().text_color(muted);
        v_flex()
            .gap_2()
            .p_6()
            .max_w(px(800.))
            .child(heading(div()).child(rust_i18n::t!("help.start_t1").to_string()))
            .child(para(div()).child(rust_i18n::t!("help.start_p1").to_string()))
            .child(heading(div()).child(rust_i18n::t!("help.start_t2").to_string()))
            .child(para(div()).child(rust_i18n::t!("help.start_p2").to_string()))
            .child(heading(div()).child(rust_i18n::t!("help.start_t3").to_string()))
            .child(para(div()).child(rust_i18n::t!("help.start_p3").to_string()))
            .child(heading(div()).child(rust_i18n::t!("help.start_t4").to_string()))
            .child(para(div()).child(rust_i18n::t!("help.start_p4").to_string()))
            .child(heading(div()).child(rust_i18n::t!("help.start_t5").to_string()))
            .child(para(div()).child(rust_i18n::t!("help.start_p5").to_string()))
            .child(heading(div()).child(rust_i18n::t!("help.start_t6").to_string()))
            .child(para(div()).child(rust_i18n::t!("help.start_p6").to_string()))
    }

    fn render_features(&self, theme: &Theme) -> Div {
        let muted = theme.muted_foreground;
        let mut content = v_flex().gap_2().p_6().max_w(px(860.));
        content = content.child(
            div()
                .text_sm()
                .text_color(muted)
                .child(rust_i18n::t!("help.view_desc_intro").to_string()),
        );
        for entry in &self.features {
            content = content.child(render_feature_card(entry, theme));
        }
        content
    }

    fn render_shortcuts(&self, theme: &Theme) -> Div {
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let mut content = v_flex().gap_2().p_6().max_w(px(860.));

        content = content.child(section_heading(
            rust_i18n::t!("help.group_global").to_string(),
            fg,
        ));
        for row in &self.global_rows {
            content = content.child(render_shortcut_row(row, theme));
        }

        content = content.child(
            section_heading(rust_i18n::t!("help.group_rail").to_string(), fg).mt(px(16.)),
        );
        content = content.child(note_box(
            rust_i18n::t!("help.sc_rail_note").to_string(),
            theme,
        ));
        for row in &self.rail_rows {
            content = content.child(render_shortcut_row(row, theme));
        }

        content = content.child(
            section_heading(rust_i18n::t!("help.group_editor").to_string(), fg).mt(px(16.)),
        );
        content = content.child(note_box(
            rust_i18n::t!("help.sc_custom_note").to_string(),
            theme,
        ));
        for (group, rows) in &self.editor_groups {
            content = content.child(sub_heading(group.clone(), muted).mt(px(10.)));
            for row in rows {
                content = content.child(render_shortcut_row(row, theme));
            }
        }
        content
    }

    fn render_about(&self, theme: &Theme) -> Div {
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        v_flex()
            .gap_2()
            .p_6()
            .max_w(px(800.))
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(fg)
                    .child(format!(
                        "{}：{}",
                        rust_i18n::t!("help.about_version"),
                        env!("CARGO_PKG_VERSION")
                    )),
            )
            .child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(rust_i18n::t!("help.about_desc").to_string()),
            )
    }

    fn render_search_results(&self, theme: &Theme) -> Div {
        let fg = theme.foreground;
        let muted = theme.muted_foreground;
        let query = self.query.trim();
        let editor_group_prefix = rust_i18n::t!("help.group_editor").to_string();

        // (group label, row) pairs in display order.
        let mut matches: Vec<(String, &ShortcutRow)> = Vec::new();
        for row in &self.global_rows {
            if row.haystack.contains(query) {
                matches.push((rust_i18n::t!("help.group_global").to_string(), row));
            }
        }
        for row in &self.rail_rows {
            if row.haystack.contains(query) {
                matches.push((rust_i18n::t!("help.group_rail").to_string(), row));
            }
        }
        for (group, rows) in &self.editor_groups {
            for row in rows {
                if row.haystack.contains(query) {
                    matches.push((format!("{editor_group_prefix} · {group}"), row));
                }
            }
        }
        let feature_matches: Vec<&FeatureEntry> = self
            .features
            .iter()
            .filter(|entry| entry.haystack.contains(query))
            .collect();

        if matches.is_empty() && feature_matches.is_empty() {
            return v_flex().gap_2().p_6().child(
                div()
                    .text_sm()
                    .text_color(muted)
                    .child(rust_i18n::t!("help.no_results").to_string()),
            );
        }

        let mut content = v_flex().gap_2().p_6().max_w(px(860.));
        content = content.child(section_heading(
            rust_i18n::t!("help.search_results").to_string(),
            fg,
        ));

        // Rows sharing a group stay under one sub-heading.
        let mut body = v_flex().gap_2();
        let mut current_group: Option<String> = None;
        for (group, row) in &matches {
            if current_group.as_deref() != Some(group.as_str()) {
                body = body.child(sub_heading(group.clone(), muted));
                current_group = Some(group.clone());
            }
            body = body.child(render_shortcut_row(row, theme));
        }
        if !feature_matches.is_empty() {
            body = body.child(sub_heading(
                rust_i18n::t!("help.nav_features").to_string(),
                muted,
            ));
            for entry in feature_matches {
                body = body.child(render_feature_card(entry, theme));
            }
        }
        content.child(body)
    }
}

impl Render for HelpCenter {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let active = self.active_section;
        let browsing = self.query.trim().is_empty();

        let sheet_layer = gpui_component::Root::render_sheet_layer(window, cx);
        let dialog_layer = gpui_component::Root::render_dialog_layer(window, cx);
        let notification_layer = gpui_component::Root::render_notification_layer(window, cx);

        // Secondary nav, same look as the settings window.
        let mut nav = v_flex()
            .w(px(160.))
            .flex_shrink_0()
            .h_full()
            .border_r_1()
            .border_color(border)
            .bg(theme.muted)
            .py_2()
            .gap_1()
            .px_2()
            .child(
                div()
                    .text_xs()
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(muted)
                    .px_2()
                    .py_1()
                    .child(rust_i18n::t!("help.title").to_string()),
            );
        for section in HelpSection::ALL {
            let is_active = active == section;
            let fg = theme.foreground;
            let accent = theme.accent;
            let mf = muted;
            nav = nav.child(
                div()
                    .id(format!("help-nav-{}", section.id()))
                    .px_2()
                    .py_1p5()
                    .rounded_md()
                    .cursor_pointer()
                    .text_sm()
                    .when(is_active, |d| {
                        d.bg(accent.opacity(0.5)).text_color(fg)
                    })
                    .when(!is_active, |d| {
                        d.text_color(mf).hover(|s| s.bg(accent.opacity(0.2)))
                    })
                    .child(section.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.active_section = section;
                        this.query.clear();
                        this.search.update(cx, |input, cx| {
                            input.set_value(String::new(), window, cx);
                        });
                        cx.notify();
                    })),
            );
        }

        let content = if browsing {
            match self.active_section {
                HelpSection::GettingStarted => self.render_start(&theme),
                HelpSection::Features => self.render_features(&theme),
                HelpSection::Shortcuts => self.render_shortcuts(&theme),
                HelpSection::About => self.render_about(&theme),
            }
        } else {
            self.render_search_results(&theme)
        };

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(theme.background)
            .text_color(theme.foreground)
            // ---- Title bar (same self-drawn chrome as the settings window) ----
            .child(
                TitleBar::new()
                    .h(px(38.))
                    .pl_3()
                    .bg(theme.muted)
                    .border_color(border)
                    .on_close_window(|_, window, _cx| window.remove_window())
                    .child(
                        h_flex()
                            .h_full()
                            .w_full()
                            .min_w_0()
                            .pr_3()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(rust_i18n::t!("help.title").to_string()),
                            )
                            .child(div().flex_1())
                            .child(Input::new(&self.search).small().w(px(240.)))
                            .when(cfg!(target_os = "macos"), |row| {
                                row.child(
                                    Button::new("help-close")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip(rust_i18n::t!("settings.close").to_string())
                                        .on_click(|_, window, _cx: &mut App| {
                                            window.remove_window();
                                        }),
                                )
                            }),
                    ),
            )
            // ---- Body: nav (left) + scrollable content (right) ----
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .overflow_hidden()
                    .child(nav)
                    .child(
                        div()
                            .id("help-content")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .child(content),
                    ),
            )
            // Overlay layers (dialogs/sheets/notifications) must be attached
            // by the consumer; the help window itself uses none, but keep
            // them wired for consistency with the settings window.
            .children(sheet_layer)
            .children(dialog_layer)
            .children(notification_layer)
    }
}

#[cfg(test)]
mod tests {
    // No `use super::*` glob — it would pull in gpui's own `test` attribute
    // macro (via the parent's `use gpui::*`) and shadow builtin `#[test]`.
    use super::{parse_display_keys, sc_label};
    use std::collections::BTreeMap;

    #[test]
    fn cmd_ctrl_twins_collapse_to_one() {
        let keys = parse_display_keys(&["cmd-a".to_string(), "ctrl-a".to_string()]);
        assert_eq!(keys.len(), 1);
    }

    #[test]
    fn distinct_alternatives_are_kept() {
        let keys = parse_display_keys(&["cmd-g".to_string(), "f3".to_string()]);
        assert_eq!(keys.len(), 2);
    }

    /// The help center's editor table is built from the same function that
    /// registers the real keybindings, so it must expose every effective
    /// shortcut with keys and a translatable label, and the embedded
    /// chrome-toggle re-keying must be reflected.
    #[test]
    fn effective_shortcuts_feed_the_help_table() {
        let config: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let shortcuts = crate::wysiwyg::components::actions::effective_shortcuts(&config);
        assert!(!shortcuts.is_empty());
        for shortcut in &shortcuts {
            assert!(
                !shortcut.keys.is_empty(),
                "shortcut {} must have keys",
                shortcut.definition.id
            );
            let label = sc_label(shortcut.definition.id);
            assert!(
                !label.is_empty(),
                "shortcut {} must have a label",
                shortcut.definition.id
            );
        }

        // Headings show a single platform-preferred key (⌘1/⌘6 collapse to one).
        let heading = shortcuts
            .iter()
            .find(|s| s.definition.id == "heading_1")
            .expect("heading_1 is registered");
        assert_eq!(parse_display_keys(&heading.keys).len(), 1);

        // The chrome toggles carry the embedded-build re-keyed defaults.
        let view_mode = shortcuts
            .iter()
            .find(|s| s.definition.id == "toggle_view_mode")
            .expect("toggle_view_mode is registered");
        assert_eq!(view_mode.keys, vec!["ctrl-shift-m".to_string()]);
        let workspace = shortcuts
            .iter()
            .find(|s| s.definition.id == "toggle_workspace")
            .expect("toggle_workspace is registered");
        assert_eq!(workspace.keys, vec!["cmd-\\".to_string()]);

        // Window-level File actions are owned by the app, not the editor.
        assert!(shortcuts
            .iter()
            .all(|s| s.definition.id != "save_document"));
    }
}
