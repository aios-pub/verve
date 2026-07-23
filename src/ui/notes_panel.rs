//! Notepad panel — markdown notes with live render/source modes, slash command palette, and nested sub-documents.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui::actions;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Enter, Escape, Input, InputEvent, InputState, MoveDown, MoveUp};
use gpui_component::list::ListItem;
use gpui_component::popover::Popover;
use gpui_component::tree::{TreeItem, TreeState, tree};
use gpui_component::{h_flex, v_flex, ActiveTheme, Disableable as _, Icon, IconName, Sizable as _, Selectable as _};

use crate::notes::{self, children_of, count_descendants, create_child_note, delete_note_cascade, has_children, push_recent_command, Note, NotesStore};
use crate::state::{AppEvent, AppState};
use crate::ui::markdown_view;

actions!(verve, [ToggleViewMode]);

// ---------------------------------------------------------------------------
// Slash command catalog
// ---------------------------------------------------------------------------

/// A single slash command in the palette.
#[derive(Clone)]
struct SlashCommandDef {
    id: &'static str,
    label: &'static str,
    desc: &'static str,
    icon: IconName,
    /// Edits the InputState to apply this command. Called inside
    /// `editor.update(..., |ed, cx| apply(ed, window, cx))` so `cx` is
    /// `&mut Context<InputState>`.
    apply: fn(&mut InputState, &mut Window, &mut Context<InputState>),
}

/// All slash commands, display order.
fn all_slash_commands() -> &'static [SlashCommandDef] {
    &[
        SlashCommandDef {
            id: "bold",
            label: "Bold",
            desc: "Bold text with ** **",
            icon: IconName::Asterisk,
            apply: |ed, w, cx| wrap_selection(ed, "**", "**", "bold text", w, cx),
        },
        SlashCommandDef {
            id: "italic",
            label: "Italic",
            desc: "Italic text with * *",
            icon: IconName::ALargeSmall,
            apply: |ed, w, cx| wrap_selection(ed, "*", "*", "italic text", w, cx),
        },
        SlashCommandDef {
            id: "strikethrough",
            label: "Strikethrough",
            desc: "Strike text with ~~ ~~",
            icon: IconName::Minus,
            apply: |ed, w, cx| wrap_selection(ed, "~~", "~~", "strikethrough", w, cx),
        },
        SlashCommandDef {
            id: "code",
            label: "Inline Code",
            desc: "Inline code with ` `",
            icon: IconName::SquareTerminal,
            apply: |ed, w, cx| wrap_selection(ed, "`", "`", "code", w, cx),
        },
        SlashCommandDef {
            id: "link",
            label: "Link",
            desc: "Insert a hyperlink",
            icon: IconName::ExternalLink,
            apply: insert_link,
        },
        SlashCommandDef {
            id: "h1",
            label: "Heading 1",
            desc: "Large section heading",
            icon: IconName::BookOpen,
            apply: |ed, w, cx| set_line_prefix(ed, "# ", w, cx),
        },
        SlashCommandDef {
            id: "h2",
            label: "Heading 2",
            desc: "Medium section heading",
            icon: IconName::BookOpen,
            apply: |ed, w, cx| set_line_prefix(ed, "## ", w, cx),
        },
        SlashCommandDef {
            id: "h3",
            label: "Heading 3",
            desc: "Small section heading",
            icon: IconName::BookOpen,
            apply: |ed, w, cx| set_line_prefix(ed, "### ", w, cx),
        },
        SlashCommandDef {
            id: "bullet_list",
            label: "Bullet List",
            desc: "Create a bulleted list item",
            icon: IconName::Ellipsis,
            apply: |ed, w, cx| set_line_prefix(ed, "- ", w, cx),
        },
        SlashCommandDef {
            id: "numbered_list",
            label: "Numbered List",
            desc: "Create a numbered list item",
            icon: IconName::SortAscending,
            apply: |ed, w, cx| set_line_prefix(ed, "1. ", w, cx),
        },
        SlashCommandDef {
            id: "task_list",
            label: "Task List",
            desc: "Add a checkbox task item",
            icon: IconName::CircleCheck,
            apply: |ed, w, cx| set_line_prefix(ed, "- [ ] ", w, cx),
        },
        SlashCommandDef {
            id: "blockquote",
            label: "Quote",
            desc: "Insert a blockquote line",
            icon: IconName::Frame,
            apply: |ed, w, cx| set_line_prefix(ed, "> ", w, cx),
        },
        SlashCommandDef {
            id: "code_block",
            label: "Code Block",
            desc: "Insert a fenced code block",
            icon: IconName::SquareTerminal,
            apply: |ed, w, cx| insert_block(ed, "\n```\n", "\n```\n", "code", w, cx),
        },
        SlashCommandDef {
            id: "hr",
            label: "Divider",
            desc: "Insert a horizontal rule",
            icon: IconName::Dash,
            apply: |ed, w, cx| insert_block(ed, "\n---\n", "", "", w, cx),
        },
        SlashCommandDef {
            id: "table",
            label: "Table",
            desc: "Insert a 3×3 table",
            icon: IconName::GalleryVerticalEnd,
            apply: |ed, w, cx| {
                insert_block(
                    ed,
                    "\n| Column A | Column B | Column C |\n| --- | --- | --- |\n|  |  |  |\n",
                    "",
                    "",
                    w,
                    cx,
                )
            },
        },
        SlashCommandDef {
            id: "image",
            label: "Image",
            desc: "Insert an image",
            icon: IconName::Eye,
            apply: |ed, w, cx| insert_block(ed, "![", "](url)", "alt", w, cx),
        },
    ]
}

fn wrap_selection(
    ed: &mut InputState,
    before: &str,
    after: &str,
    placeholder: &str,
    window: &mut Window,
    cx: &mut Context<InputState>,
) {
    let sel = ed.selected_value().to_string();
    let replacement = if sel.is_empty() {
        format!("{}{}{}", before, placeholder, after)
    } else {
        format!("{}{}{}", before, sel, after)
    };
    ed.replace(replacement, window, cx);
}

fn insert_block(
    ed: &mut InputState,
    open: &str,
    close: &str,
    placeholder: &str,
    window: &mut Window,
    cx: &mut Context<InputState>,
) {
    let text = if placeholder.is_empty() {
        format!("{}{}", open, close)
    } else {
        format!("{}{}{}", open, placeholder, close)
    };
    ed.insert(text, window, cx);
}

fn insert_link(ed: &mut InputState, window: &mut Window, cx: &mut Context<InputState>) {
    let sel = ed.selected_value().to_string();
    let text = if sel.is_empty() {
        "[link text](url)".to_string()
    } else {
        format!("[{}](url)", sel)
    };
    ed.replace(text, window, cx);
}

fn set_line_prefix(
    ed: &mut InputState,
    prefix: &str,
    window: &mut Window,
    cx: &mut Context<InputState>,
) {
    let value = ed.value().to_string();
    let cursor = ed.cursor();

    let line_start = value[..cursor].rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_end = value[cursor..]
        .find('\n')
        .map(|p| cursor + p)
        .unwrap_or(value.len());

    let line = &value[line_start..line_end];
    let stripped = strip_block_prefix(line);

    let new_line = format!("{}{}", prefix, stripped);
    let new_value = format!("{}{}{}", &value[..line_start], new_line, &value[line_end..]);
    ed.set_value(new_value, window, cx);
}

/// Strip any known markdown block prefix (heading, list marker, quote) from a line.
fn strip_block_prefix(line: &str) -> &str {
    let s = line.trim_start();
    let s = s.strip_prefix("- [ ] ").unwrap_or(s);
    let s = s.strip_prefix("- [x] ").unwrap_or(s);
    let s = s.strip_prefix("- ").unwrap_or(s);
    let s = s.strip_prefix("* ").unwrap_or(s);
    let s = s.strip_prefix("> ").unwrap_or(s);
    let s = s.strip_prefix("###### ").unwrap_or(s);
    let s = s.strip_prefix("##### ").unwrap_or(s);
    let s = s.strip_prefix("#### ").unwrap_or(s);
    let s = s.strip_prefix("### ").unwrap_or(s);
    let s = s.strip_prefix("## ").unwrap_or(s);
    let s = s.strip_prefix("# ").unwrap_or(s);
    // Strip "1. ", "2. ", ... prefix (digits followed by ". ").
    let trimmed = s.trim_start_matches(|c: char| c.is_ascii_digit());
    let s = if let Some(stripped) = trimmed.strip_prefix(". ") {
        stripped
    } else {
        s
    };
    s.trim_start_matches(' ')
}

// ---------------------------------------------------------------------------
// The panel
// ---------------------------------------------------------------------------

pub struct NotesPanel {
    state: Entity<AppState>,
    store: NotesStore,
    editor: Entity<InputState>,
    search: Entity<InputState>,
    title_input: Entity<InputState>,
    editing_title_id: Option<String>,
    pending_delete: Option<String>,
    save_timer: Option<Task<()>>,
    _subs: Vec<Subscription>,
    // Nested-notes tree state.
    tree: Entity<TreeState>,
    // Slash palette state
    slash_open: bool,
    slash_query: String,
    slash_selected: usize,
    slash_trigger_pos: usize,
    /// When set, execute this command at the start of render (deferred so we
    /// have `&mut Window` access which subscribe callbacks don't provide).
    pending_slash_execute: Option<&'static str>,
    /// When true, reload editor content from active note on next render (which
    /// has &mut Window access — used by tree selection observer).
    editor_needs_reload: bool,
    /// Whether the per-note export popover (in the editor header) is open.
    export_popover_open: bool,
    /// Whether the left-side note tree is collapsed (hidden).
    list_collapsed: bool,
    /// Per-row open "..." menu, by node id (e.g. "note:<uuid>").
    row_menu_open: Option<String>,
}

fn note_node(id: &str) -> String {
    format!("note:{id}")
}
fn parse_note_id(node: &str) -> Option<&str> {
    node.strip_prefix("note:")
}

impl NotesPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = crate::state::persistence::load_notes();

        let editor = cx.new(|cx| {
            let mut input = InputState::new(window, cx).multi_line(true);
            input.set_value(String::new(), window, cx);
            input
        });

        let search = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(String::new(), window, cx);
            input
        });

        let title_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(String::new(), window, cx);
            input
        });

        let tree = cx.new(|cx| TreeState::new(cx));

        let initial_content = store
            .active_note
            .as_ref()
            .and_then(|id| store.notes.iter().find(|n| &n.id == id))
            .map(|n| n.content.clone())
            .unwrap_or_default();
        editor.update(cx, |e, cx| {
            e.set_value(initial_content, window, cx);
        });

        let editor_sub = cx.subscribe(&editor, Self::on_editor_event);
        let search_sub = cx.subscribe(&search, Self::on_search_event);
        let title_sub = cx.subscribe(&title_input, Self::on_title_event);
        let tree_sub = cx.observe(&tree, Self::on_tree_notify);

        let mut panel = Self {
            state,
            store,
            editor,
            search,
            title_input,
            tree,
            editing_title_id: None,
            pending_delete: None,
            save_timer: None,
            _subs: vec![editor_sub, search_sub, title_sub, tree_sub],
            slash_open: false,
            slash_query: String::new(),
            slash_selected: 0,
            slash_trigger_pos: 0,
            pending_slash_execute: None,
            editor_needs_reload: false,
            export_popover_open: false,
            list_collapsed: false,
            row_menu_open: None,
        };
        panel.rebuild_tree(cx);
        panel
    }

    // -- editor change subscription ------------------------------------------

    fn on_editor_event(
        &mut self,
        _: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::Change => {
                if self.slash_open {
                    self.update_slash_query_from_editor(cx);
                } else {
                    // Detect a `/` typed at word boundary → open palette.
                    let info: Option<(String, usize)> = self.editor.read_with(cx, |ed, _cx| {
                        let val = ed.value().to_string();
                        let cursor = ed.cursor();
                        Some((val, cursor))
                    });
                    let should_open = info
                        .as_ref()
                        .map(|(val, cursor)| {
                            if *cursor == 0 {
                                return false;
                            }
                            if val[..*cursor].chars().next_back() != Some('/') {
                                return false;
                            }
                            let prev_char = if *cursor >= 2 {
                                val[..*cursor - 1].chars().next_back()
                            } else {
                                None
                            };
                            matches!(
                                prev_char,
                                None | Some(' ') | Some('\n') | Some('\t') | Some('\r')
                            )
                        })
                        .unwrap_or(false);
                    if should_open {
                        let pos = info.map(|(_, c)| c.saturating_sub(1)).unwrap_or(0);
                        self.slash_open = true;
                        self.slash_query.clear();
                        self.slash_selected = 0;
                        self.slash_trigger_pos = pos;
                    }
                }
                self.schedule_save(cx);
                cx.notify();
            }
            InputEvent::PressEnter { .. } => {
                if self.slash_open {
                    self.queue_selected_command(cx);
                }
            }
            _ => {}
        }
    }

    /// Read editor text after `/` to update the filter query; close if invalid.
    fn update_slash_query_from_editor(&mut self, cx: &mut Context<Self>) {
        let info = self.editor.update(cx, |ed, _cx| {
            let val = ed.value().to_string();
            let cursor = ed.cursor();
            if cursor <= self.slash_trigger_pos {
                return None;
            }
            if val[self.slash_trigger_pos..].chars().next() != Some('/') {
                return None;
            }
            Some((val, cursor))
        });
        let Some((val, cursor)) = info else {
            self.close_slash_palette();
            cx.notify();
            return;
        };
        let query = val[self.slash_trigger_pos + 1..cursor].to_string();
        if query.chars().any(|c| c.is_whitespace()) {
            self.close_slash_palette();
            cx.notify();
            return;
        }
        self.slash_query = query;
        self.slash_selected = 0;
    }

    fn close_slash_palette(&mut self) {
        self.slash_open = false;
        self.slash_query.clear();
        self.slash_selected = 0;
        self.pending_slash_execute = None;
    }

    /// Queue the selected command to run during render (where we have &mut Window).
    fn queue_selected_command(&mut self, cx: &mut Context<Self>) {
        let filtered = self.filtered_command_defs();
        if filtered.is_empty() {
            self.close_slash_palette();
            cx.notify();
            return;
        }
        let cmd = filtered[self.slash_selected.min(filtered.len() - 1)];
        self.pending_slash_execute = Some(cmd.id);
        cx.notify();
    }

    /// Execute a queued command: delete the `/query` trigger, apply the
    /// command, record recency, close palette. Called from render where we have Window.
    fn run_pending_command(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(cmd_id) = self.pending_slash_execute.take() else {
            return;
        };
        // 1) Delete the `/<query>` substring from the editor.
        self.editor.update(cx, |ed, cx| {
            let val = ed.value().to_string();
            let cursor = ed.cursor();
            let delete_end = cursor;
            let delete_start = self.slash_trigger_pos;
            let new_val = format!("{}{}", &val[..delete_start], &val[delete_end..]);
            ed.set_value(new_val, window, cx);
        });
        // 2) Find the command def and apply it (apply takes &mut Context<InputState>
        //    which editor.update provides).
        let apply_fn = all_slash_commands()
            .iter()
            .find(|c| c.id == cmd_id)
            .map(|c| c.apply);
        if let Some(apply) = apply_fn {
            self.editor.update(cx, |ed, cx| apply(ed, window, cx));
        }
        // 3) Record in recent commands and persist.
        push_recent_command(&mut self.store.recent_commands, cmd_id);
        let _ = crate::state::persistence::save_notes(&self.store);
        self.close_slash_palette();
        self.schedule_save(cx);
    }

    fn move_slash_selection(&mut self, delta: i32, cx: &mut Context<Self>) {
        if !self.slash_open {
            return;
        }
        let filtered = self.filtered_command_defs();
        let n = filtered.len() as i32;
        if n == 0 {
            return;
        }
        let current = self.slash_selected as i32;
        let new_idx = (current + delta).rem_euclid(n);
        self.slash_selected = new_idx as usize;
        cx.notify();
    }

    /// Return the filtered command defs (matching query), split into
    /// (recent_matches, all_matches) — both as static references.
    fn filtered_command_defs(&self) -> Vec<&'static SlashCommandDef> {
        let cmds = all_slash_commands();
        let query = self.slash_query.to_lowercase();
        // Build a score-ordered list: recent matches first, then alphabetic.
        let mut recent_matched: Vec<&'static SlashCommandDef> = Vec::new();
        let mut other_matched: Vec<&'static SlashCommandDef> = Vec::new();

        for cmd in cmds {
            if !query.is_empty()
                && !cmd.label.to_lowercase().contains(&query)
                && !cmd.id.to_lowercase().contains(&query)
            {
                continue;
            }
            if self.store.recent_commands.iter().any(|r| r == cmd.id) {
                recent_matched.push(cmd);
            } else {
                other_matched.push(cmd);
            }
        }
        // Sort recent list by recent order.
        recent_matched.sort_by_key(|c| {
            self.store
                .recent_commands
                .iter()
                .position(|r| r == c.id)
                .unwrap_or(usize::MAX)
        });
        let mut result = recent_matched;
        result.extend(other_matched);
        result
    }

    // -- other callbacks -----------------------------------------------------

    fn on_search_event(
        &mut self,
        _: Entity<InputState>,
        _event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        self.rebuild_tree(cx);
        cx.notify();
    }

    // -- tree / hierarchy ---------------------------------------------------

    fn on_tree_notify(&mut self, _tree: Entity<TreeState>, cx: &mut Context<Self>) {
        let Some(item) = self.tree.read(cx).selected_item().cloned() else {
            return;
        };
        let node = item.id.to_string();
        if let Some(note_id) = parse_note_id(&node) {
            let note_id = note_id.to_string();
            if self.store.active_note.as_deref() != Some(note_id.as_str()) {
                // Update active_note and request editor reload on next render.
                if self.editing_title_id.is_some() {
                    self.commit_title_edit(cx);
                }
                self.close_slash_palette();
                self.save_current(cx);
                self.store.active_note = Some(note_id);
                // Mark that editor needs reloading with the new note's content.
                self.editor_needs_reload = true;
                cx.notify();
            }
        }
    }

    /// Rebuild the tree's flat `Vec<TreeItem>` from `self.store.notes` and the
    /// current search query.
    fn rebuild_tree(&mut self, cx: &mut Context<Self>) {
        let query = self.search.read(cx).value().to_string().to_lowercase();
        let items = self.build_note_children(None, &query);
        self.tree.update(cx, |t, cx| t.set_items(items, cx));
        cx.notify();
    }

    /// Toggle a note's collapsed state (driven by chevron clicks) and persist.
    fn toggle_note_collapsed(&mut self, note_id: &str, cx: &mut Context<Self>) {
        if self.store.collapsed.iter().any(|c| c == note_id) {
            self.store.collapsed.retain(|c| c != note_id);
        } else {
            self.store.collapsed.push(note_id.to_string());
        }
        let _ = crate::state::persistence::save_notes(&self.store);
        self.rebuild_tree(cx);
    }

    /// Recursively build TreeItems for children of `parent_id` (None = root).
    fn build_note_children(&self, parent_id: Option<&str>, query: &str) -> Vec<TreeItem> {
        let kids = children_of(&self.store, parent_id);
        let mut items = Vec::new();
        for note in kids {
            let id = note_node(&note.id);
            let has_kids = has_children(&self.store, &note.id);
            let descendants_match = if !query.is_empty() {
                self.descendants_match(&note.id, query)
            } else {
                false
            };
            let self_matches = query.is_empty()
                || note.title.to_lowercase().contains(query)
                || note.content.to_lowercase().contains(query);
            if !query.is_empty() && !self_matches && !descendants_match {
                continue;
            }
            let children = self.build_note_children(Some(&note.id), query);
            let is_collapsed = self.store.collapsed.contains(&note.id);
            let mut label = note.title.clone();
            if label.is_empty() {
                label = rust_i18n::t!("notes.untitled").to_string();
            }
            let item = TreeItem::new(id.clone(), label)
                .children(children)
                .expanded(!is_collapsed);
            items.push(item);
        }
        items
    }

    /// True if any descendant of `note_id` matches the search query.
    fn descendants_match(&self, note_id: &str, query: &str) -> bool {
        for note in &self.store.notes {
            if note.parent_id.as_deref() == Some(note_id) {
                let self_matches = note.title.to_lowercase().contains(query)
                    || note.content.to_lowercase().contains(query);
                if self_matches || self.descendants_match(&note.id, query) {
                    return true;
                }
            }
        }
        false
    }

    /// Create a new sub-note under the currently active note.
    fn new_child_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(parent_id) = self.store.active_note.clone() else {
            // No active note — create a root note instead.
            self.new_note(window, cx);
            return;
        };
        self.close_slash_palette();
        let new_id = create_child_note(&mut self.store, &parent_id, String::new());
        self.editor.update(cx, |e, cx| {
            e.set_value(String::new(), window, cx);
        });
        let _ = crate::state::persistence::save_notes(&self.store);
        self.rebuild_tree(cx);
        // Select the new note in the tree and start title editing.
        let target = TreeItem::new(note_node(&new_id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        self.store.active_note = Some(new_id.clone());
        self.editor_needs_reload = false;
        self.start_title_edit(&new_id, "", window, cx);
    }

    fn on_title_event(
        &mut self,
        _: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => self.commit_title_edit(cx),
            InputEvent::Blur => self.commit_title_edit(cx),
            _ => {}
        }
    }

    fn commit_title_edit(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editing_title_id.take() else {
            return;
        };
        let new_title = self.title_input.read(cx).value().to_string();
        notes::update_note_title(&mut self.store, &id, new_title);
        // Write to disk for local safety, but do NOT emit Persisted — git sync
        // follows the configured interval, not every edit.
        let _ = crate::state::persistence::save_notes(&self.store);
        self.rebuild_tree(cx);
        cx.notify();
    }

    fn start_title_edit(
        &mut self,
        id: &str,
        current: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editing_title_id = Some(id.to_string());
        self.title_input.update(cx, |s, cx| {
            s.set_value(current.to_string(), window, cx);
            s.focus(window, cx);
        });
        cx.notify();
    }

    fn schedule_save(&mut self, cx: &mut Context<Self>) {
        let weak = cx.weak_entity();
        self.save_timer = Some(cx.spawn(async move |_this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(500))
                .await;
            let _ = weak.update(cx, |this, cx| {
                this.save_current(cx);
            });
        }));
    }

    fn save_current(&mut self, cx: &mut Context<Self>) {
        let editor_val = self.editor.read(cx).value().to_string();
        if let Some(active_id) = self.store.active_note.clone() {
            notes::update_note_content(&mut self.store, &active_id, editor_val);
            // Write to local disk for crash safety, but do NOT emit Persisted.
            // Git sync runs on the configured interval (settings.git.sync_interval)
            // rather than firing on every edit.
            let _ = crate::state::persistence::save_notes(&self.store);
        }
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.close_slash_palette();
        let note = notes::create_note(&mut self.store, String::new());
        let new_id = note.id.clone();
        self.store.active_note = Some(new_id.clone());
        self.editor.update(cx, |e, cx| {
            e.set_value(String::new(), window, cx);
        });
        let _ = crate::state::persistence::save_notes(&self.store);
        self.rebuild_tree(cx);
        let target = TreeItem::new(note_node(&new_id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        self.start_title_edit(&new_id, "", window, cx);
    }

    /// Select a note (called from list-row click handlers — has Window access).
    fn select_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_title_id.is_some() {
            self.commit_title_edit(cx);
        }
        self.close_slash_palette();
        self.save_current(cx);
        self.store.active_note = Some(id.to_string());
        self.editor_needs_reload = false;
        if let Some(note) = self.store.notes.iter().find(|n| n.id == id) {
            self.editor.update(cx, |e, cx| {
                e.set_value(note.content.clone(), window, cx);
            });
        }
        self.rebuild_tree(cx);
        let target = TreeItem::new(note_node(id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        cx.notify();
    }

    fn delete_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_title_id.is_some() {
            self.commit_title_edit(cx);
        }
        delete_note_cascade(&mut self.store, id);
        if let Some(note) = self.store.notes.first() {
            self.store.active_note = Some(note.id.clone());
            self.editor.update(cx, |e, cx| {
                e.set_value(note.content.clone(), window, cx);
            });
        } else {
            self.editor.update(cx, |e, cx| {
                e.set_value(String::new(), window, cx);
            });
        }
        let _ = crate::state::persistence::save_notes(&self.store);
        self.pending_delete = None;
        self.rebuild_tree(cx);
        cx.notify();
    }

    pub fn set_view_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        self.store.view_mode = mode.to_string();
        let _ = crate::state::persistence::save_notes(&self.store);
        cx.notify();
    }

    fn toggle_view_mode(&mut self, _: &ToggleViewMode, cx: &mut Context<Self>) {
        let new_mode = if self.store.view_mode == "source" {
            "live"
        } else {
            "source"
        };
        self.set_view_mode(new_mode, cx);
    }

    /// Number of notes in the store (used for the title-bar badge).
    pub fn note_count(&self) -> usize {
        self.store.notes.len()
    }

    /// Whether the editor is currently in source (raw markdown) mode.
    pub fn is_source_mode(&self) -> bool {
        self.store.view_mode == "source"
    }

    /// Toggle whether the note list sidebar is collapsed (hidden).
    pub fn toggle_list_collapsed(&mut self, cx: &mut Context<Self>) {
        self.list_collapsed = !self.list_collapsed;
        cx.notify();
    }

    /// Whether the note list sidebar is currently collapsed.
    pub fn is_list_collapsed(&self) -> bool {
        self.list_collapsed
    }

    /// Duplicate a note by id (creates a copy with "(Copy)" suffix, selects it).
    fn duplicate_note_by_id(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(new_id) = notes::duplicate_note(&mut self.store, id) {
            self.row_menu_open = None;
            let _ = crate::state::persistence::save_notes(&self.store);
            self.rebuild_tree(cx);
            self.select_note(&new_id, window, cx);
        }
    }

    /// Toggle pin for a note by id.
    fn toggle_pin_by_id(&mut self, id: &str, cx: &mut Context<Self>) {
        notes::toggle_pin(&mut self.store, id);
        self.row_menu_open = None;
        let _ = crate::state::persistence::save_notes(&self.store);
        self.rebuild_tree(cx);
        cx.notify();
    }

    /// Move a note to root level by id.
    fn move_note_to_root_by_id(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        notes::move_note_to_root(&mut self.store, id);
        self.row_menu_open = None;
        let _ = crate::state::persistence::save_notes(&self.store);
        self.rebuild_tree(cx);
        // Keep the note selected after move.
        if self.store.active_note.as_deref() == Some(id) {
            self.select_note(id, window, cx);
        } else {
            cx.notify();
        }
    }

    /// Create a new sub-note under an arbitrary parent (not necessarily active).
    fn new_child_note_for_id(
        &mut self,
        parent_id: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_slash_palette();
        let new_id = create_child_note(&mut self.store, parent_id, String::new());
        self.editor.update(cx, |e, cx| {
            e.set_value(String::new(), window, cx);
        });
        let _ = crate::state::persistence::save_notes(&self.store);
        self.rebuild_tree(cx);
        let target = TreeItem::new(note_node(&new_id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        self.store.active_note = Some(new_id.clone());
        self.editor_needs_reload = false;
        self.start_title_edit(&new_id, "", window, cx);
    }

    /// Export a specific note (by id) in the given format.
    fn export_note_as(
        &mut self,
        note_id: &str,
        fmt: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(note) = self.store.notes.iter().find(|n| n.id == note_id).cloned() else {
            return;
        };
        self.row_menu_open = None;
        self.export_popover_open = false;

        let suggested = sanitize_filename(&if note.title.is_empty() {
            rust_i18n::t!("notes.untitled").to_string()
        } else {
            note.title.clone()
        });
        let title = note.title.clone();
        let content = note.content.clone();
        let format = fmt.to_string();
        let format_for_dialog = format.clone();
        let format_for_save = format.clone();

        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let (filename, filter_name, ext) = match format_for_dialog.as_str() {
                "pdf" => (format!("{suggested}.pdf"), "PDF", "pdf"),
                _ => (format!("{suggested}.md"), "Markdown", "md"),
            };
            let res = rfd::FileDialog::new()
                .set_file_name(&filename)
                .add_filter(filter_name, &[ext])
                .save_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let result = match format_for_save.as_str() {
                            "pdf" => crate::notes_pdf::markdown_to_pdf(&content, &title, &path),
                            _ => std::fs::write(&path, &content).map_err(anyhow::Error::from),
                        };
                        match result {
                            Ok(()) => log::info!("exported note to {:?}", path),
                            Err(e) => log::error!("export failed: {e:?}"),
                        }
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
        cx.notify();
    }

    // -- import / export ---------------------------------------------------

    /// Open a native file picker and import the chosen `.md` file as a new note.
    pub fn import_markdown_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .add_filter("Markdown", &["md", "markdown", "txt"])
                .pick_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let ent = cx.entity().downgrade();
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let _ = ent.update(cx, |panel, cx| {
                            panel.import_note_from_path(&path, cx);
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

    fn import_note_from_path(&mut self, path: &std::path::Path, cx: &mut Context<Self>) {
        let content = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                log::error!("import markdown read failed: {e:?}");
                return;
            }
        };
        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Untitled")
            .to_string();
        let note = notes::create_note(&mut self.store, title.clone());
        let new_id = note.id.clone();
        notes::update_note_content(&mut self.store, &new_id, content);
        self.store.active_note = Some(new_id.clone());
        let _ = crate::state::persistence::save_notes(&self.store);
        self.editor_needs_reload = true;
        self.rebuild_tree(cx);
        let target = TreeItem::new(note_node(&new_id), "");
        self.tree.update(cx, |t, cx| {
            t.set_selected_item(Some(&target), cx);
        });
        log::info!("imported markdown note from {:?}", path);
        cx.notify();
    }

    /// Open a native save dialog and export the currently active note as Markdown.
    pub fn export_active_note_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.export_active_note_as("md", window, cx);
    }

    /// Export the active note in a given format: `"md"` or `"pdf"`.
    fn export_active_note_as(&mut self, fmt: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_id) = self.store.active_note.clone() else {
            log::warn!("export: no active note");
            return;
        };
        let Some(note) = self.store.notes.iter().find(|n| n.id == active_id).cloned() else {
            return;
        };
        self.export_popover_open = false;

        let suggested = sanitize_filename(&if note.title.is_empty() {
            rust_i18n::t!("notes.untitled").to_string()
        } else {
            note.title.clone()
        });
        let title = note.title.clone();
        let content = note.content.clone();
        let format = fmt.to_string();
        let format_for_dialog = format.clone();
        let format_for_save = format.clone();

        let (tx, rx) = std::sync::mpsc::channel::<Option<std::path::PathBuf>>();
        std::thread::spawn(move || {
            let (filename, filter_name, ext) = match format_for_dialog.as_str() {
                "pdf" => (format!("{suggested}.pdf"), "PDF", "pdf"),
                _ => (format!("{suggested}.md"), "Markdown", "md"),
            };
            let res = rfd::FileDialog::new()
                .set_file_name(&filename)
                .add_filter(filter_name, &[ext])
                .save_file();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        cx.spawn(async move |_this, cx| {
            loop {
                match rx.try_recv() {
                    Ok(Some(path)) => {
                        let result = match format_for_save.as_str() {
                            "pdf" => crate::notes_pdf::markdown_to_pdf(&content, &title, &path),
                            _ => std::fs::write(&path, &content).map_err(anyhow::Error::from),
                        };
                        match result {
                            Ok(()) => log::info!("exported note to {:?}", path),
                            Err(e) => log::error!("export failed: {e:?}"),
                        }
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
        cx.notify();
    }

    // -- rendering -----------------------------------------------------------

    fn render_note_list(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let view = cx.entity().clone();

        v_flex()
            .w(px(260.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.3))
            .child(
                h_flex()
                    .w_full()
                    .p_2()
                    .gap_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("notes-new")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .flex_1()
                            .child(rust_i18n::t!("notes.new_note").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_note(window, cx);
                            })),
                    )
                    .child(
                        Button::new("notes-new-child")
                            .small()
                            .ghost()
                            .icon(IconName::ChevronRight)
                            .tooltip(rust_i18n::t!("notes.new_sub_note").to_string())
                            .disabled(self.store.active_note.is_none())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_child_note(window, cx);
                            })),
                    )
                    .child(Input::new(&self.search).w(px(100.)).small()),
            )
            .child(
                div()
                    .id("notes-list-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .py_1()
                    .child(
                        tree(&self.tree, move |ix, entry, _selected, _window, cx| {
                            let item = entry.item();
                            let note_id = parse_note_id(item.id.as_ref()).unwrap_or("").to_string();
                            let note = view
                                .read(cx)
                                .store
                                .notes
                                .iter()
                                .find(|n| n.id == note_id);
                            let is_active = view.read(cx).store.active_note.as_deref()
                                == Some(note_id.as_str());
                            let is_folder = entry.is_folder();
                            let is_expanded = entry.is_expanded();
                            let depth = entry.depth();
                            let title = note
                                .map(|n| {
                                    if n.title.is_empty() {
                                        rust_i18n::t!("notes.untitled").to_string()
                                    } else {
                                        n.title.clone()
                                    }
                                })
                                .unwrap_or_default();
                            let updated = note
                                .map(|n| {
                                    rust_i18n::t!(
                                        "notes.last_updated",
                                        time = n.updated_at.as_str()
                                    )
                                    .to_string()
                                })
                                .unwrap_or_default();

                            let delete_id = note_id.clone();
                            let chevron_target = note_id.clone();
                            let view_for_chevron = view.clone();
                            let view_for_select = view.clone();
                            let view_for_delete = view.clone();
                            let view_for_add = view.clone();
                            let view_for_menu = view.clone();
                            let is_pinned = note.map(|n| n.pinned).unwrap_or(false);
                            let group = format!("notes-row-{ix}");
                            let add_nid = note_id.clone();
                            let menu_nid = note_id.clone();
                            let menu_nid2 = note_id.clone();
                            let menu_nid3 = note_id.clone();
                            let menu_nid4 = note_id.clone();
                            let menu_nid5 = note_id.clone();
                            let menu_nid_export = note_id.clone();
                            let row_menu_open = view.read(cx).row_menu_open.as_deref()
                                == Some(note_id.as_str());

                            ListItem::new(ix)
                                .w_full()
                                .selected(is_active)
                                .rounded(px(4.))
                                .px_2()
                                .pl(px(16.) * depth + px(4.))
                                .when(is_active, |li| {
                                    li.bg(theme.accent.opacity(0.3))
                                        .text_color(theme.foreground)
                                })
                                .child(
                                    h_flex()
                                        .w_full()
                                        .gap_1()
                                        .items_center()
                                        .group(group.clone())
                                        // Pin indicator
                                        .when(is_pinned, |c| {
                                            c.child(
                                                div()
                                                    .text_size(px(10.))
                                                    .text_color(theme.accent)
                                                    .child(IconName::Star),
                                            )
                                        })
                                        // Chevron (only for notes that can have children)
                                        .child(
                                            div()
                                                .w(px(16.))
                                                .h(px(16.))
                                                .flex()
                                                .items_center()
                                                .justify_center()
                                                .text_color(theme.muted_foreground)
                                                .when(is_folder, |c| {
                                                    c.child(if is_expanded {
                                                        IconName::ChevronDown
                                                    } else {
                                                        IconName::ChevronRight
                                                    })
                                                    .hover(|c| c.text_color(theme.foreground))
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        move |_, _window, cx: &mut App| {
                                                            cx.stop_propagation();
                                                            let _ = view_for_chevron.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.toggle_note_collapsed(
                                                                        &chevron_target,
                                                                        cx,
                                                                    );
                                                                },
                                                            );
                                                        },
                                                    )
                                                }),
                                        )
                                        // Note icon
                                        .child(
                                            div()
                                                .text_size(px(12.))
                                                .text_color(theme.muted_foreground)
                                                .child(if is_folder {
                                                    if is_expanded {
                                                        IconName::FolderOpen
                                                    } else {
                                                        IconName::Folder
                                                    }
                                                } else {
                                                    IconName::File
                                                }),
                                        )
                                        // Title + timestamp
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w_0()
                                                .gap(px(1.))
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    {
                                                        let nid = note_id.clone();
                                                        let view = view_for_select;
                                                        move |_, window, cx: &mut App| {
                                                            let _ = view.update(
                                                                cx,
                                                                |this, cx| {
                                                                    this.select_note(
                                                                        &nid, window, cx,
                                                                    );
                                                                },
                                                            );
                                                        }
                                                    },
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(13.))
                                                        .font_weight(FontWeight::MEDIUM)
                                                        .truncate()
                                                        .child(title),
                                                )
                                                .child(
                                                    div()
                                                        .text_size(px(10.))
                                                        .text_color(theme.muted_foreground)
                                                        .truncate()
                                                        .child(updated),
                                                ),
                                        )
                                        // Hover-revealed "+" button (new sub-note)
                                        .child(
                                            div()
                                                .id(format!("notes-add-{}", ix))
                                                .px_1()
                                                .rounded_sm()
                                                .opacity(0.0)
                                                .text_color(theme.muted_foreground)
                                                .group_hover(group.clone(), |this| {
                                                    this.opacity(1.0).text_color(theme.foreground)
                                                })
                                                .hover(|this| this.bg(theme.border))
                                                .cursor_pointer()
                                                .child(IconName::Plus)
                                                .on_mouse_down(
                                                    MouseButton::Left,
                                                    move |_, window, cx: &mut App| {
                                                        cx.stop_propagation();
                                                        let pid = add_nid.clone();
                                                        let _ = view_for_add.update(
                                                            cx,
                                                            |this, cx| {
                                                                this.new_child_note_for_id(
                                                                    &pid, window, cx,
                                                                );
                                                            },
                                                        );
                                                    },
                                                ),
                                        )
                                        // Hover-revealed "..." menu button
                                        .child({
                                            let popover_id = format!("notes-row-pop-{}", note_id);
                                            let view_for_open = view.clone();
                                            let open_nid = menu_nid5.clone();
                                            Popover::new(popover_id)
                                                .anchor(gpui::Anchor::BottomRight)
                                                .open(row_menu_open)
                                                .on_open_change(move |open: &bool, _window, cx: &mut App| {
                                                    let _ = view_for_open.update(cx, |this, cx| {
                                                        if *open {
                                                            this.row_menu_open =
                                                                Some(open_nid.clone());
                                                        } else if this.row_menu_open.as_deref()
                                                            == Some(open_nid.as_str())
                                                        {
                                                            this.row_menu_open = None;
                                                        }
                                                        cx.notify();
                                                    });
                                                })
                                                .trigger(
                                                    Button::new(format!("notes-more-btn-{}", ix))
                                                        .ghost()
                                                        .xsmall()
                                                        .icon(IconName::EllipsisVertical),
                                                )
                                                .p(px(4.))
                                                .child(
                                                    v_flex()
                                                        .w(px(180.))
                                                        .gap(px(2.))
                                                        // Duplicate
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(IconName::Copy)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("notes.duplicate").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let nid = menu_nid.clone();
                                                                        let view = view_for_menu.clone();
                                                                        move |_, window, cx: &mut App| {
                                                                            let nid = nid.clone();
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.duplicate_note_by_id(&nid, window, cx);
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                        // Move to Root
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(IconName::ArrowUp)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("notes.move_to_root").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let nid = menu_nid2.clone();
                                                                        let view = view_for_menu.clone();
                                                                        move |_, window, cx: &mut App| {
                                                                            let nid = nid.clone();
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.move_note_to_root_by_id(&nid, window, cx);
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                        // Pin/Unpin
                                                        .child({
                                                            let nid_pin = menu_nid3.clone();
                                                            let view_pin = view_for_menu.clone();
                                                            let pin_label = if is_pinned {
                                                                rust_i18n::t!("notes.unpin").to_string()
                                                            } else {
                                                                rust_i18n::t!("notes.pin").to_string()
                                                            };
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(if is_pinned { IconName::StarOff } else { IconName::Star })
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(pin_label),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    move |_, _, cx: &mut App| {
                                                                        let nid = nid_pin.clone();
                                                                        let _ = view_pin.update(cx, |this, cx| {
                                                                            this.toggle_pin_by_id(&nid, cx);
                                                                        });
                                                                    },
                                                                )
                                                        })
                                                        // Divider
                                                        .child(
                                                            div()
                                                                .w_full()
                                                                .h(px(1.))
                                                                .my(px(2.))
                                                                .bg(theme.border),
                                                        )
                                                        // Export MD
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(IconName::ExternalLink)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("notes.row_export_md").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let nid = menu_nid_export.clone();
                                                                        let view = view_for_menu.clone();
                                                                        move |_, window, cx: &mut App| {
                                                                            let nid = nid.clone();
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.export_note_as(&nid, "md", window, cx);
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                        // Export PDF
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.muted))
                                                                .child(IconName::ExternalLink)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("notes.row_export_pdf").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let nid = menu_nid4.clone();
                                                                        let view = view_for_menu.clone();
                                                                        move |_, window, cx: &mut App| {
                                                                            let nid = nid.clone();
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.export_note_as(&nid, "pdf", window, cx);
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                        // Divider
                                                        .child(
                                                            div()
                                                                .w_full()
                                                                .h(px(1.))
                                                                .my(px(2.))
                                                                .bg(theme.border),
                                                        )
                                                        // Delete
                                                        .child(
                                                            h_flex()
                                                                .w_full()
                                                                .px_2()
                                                                .py_1()
                                                                .gap_2()
                                                                .items_center()
                                                                .rounded_md()
                                                                .cursor_pointer()
                                                                .hover(|s| s.bg(theme.danger.opacity(0.2)))
                                                                .text_color(theme.danger)
                                                                .child(IconName::Delete)
                                                                .child(
                                                                    div()
                                                                        .text_size(px(12.))
                                                                        .child(rust_i18n::t!("common.delete").to_string()),
                                                                )
                                                                .on_mouse_down(
                                                                    MouseButton::Left,
                                                                    {
                                                                        let did = delete_id.clone();
                                                                        let view = view_for_delete.clone();
                                                                        move |_, _, cx: &mut App| {
                                                                            let _ = view.update(cx, |this, cx| {
                                                                                this.row_menu_open = None;
                                                                                this.pending_delete = Some(did.clone());
                                                                                cx.notify();
                                                                            });
                                                                        }
                                                                    },
                                                                ),
                                                        )
                                                )
                                        })
                                )
                        })
                        .into_any_element(),
                    ),
            )
            .into_any_element()
    }

    fn render_delete_confirm(&self, note_id: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let note = self.store.notes.iter().find(|n| n.id == note_id);
        let note_title = note.map(|n| n.title.clone()).unwrap_or_default();
        let descendant_count = note
            .map(|n| count_descendants(&self.store, &n.id))
            .unwrap_or(0);

        let msg = if descendant_count > 0 {
            rust_i18n::t!(
                "notes.confirm_delete_cascade",
                title = note_title.as_str(),
                count = descendant_count
            )
            .to_string()
        } else {
            rust_i18n::t!("notes.confirm_delete_msg", title = note_title.as_str()).to_string()
        };
        let id = note_id.to_string();

        div()
            .absolute()
            .inset_0()
            .bg(black().opacity(0.5))
            .flex()
            .items_center()
            .justify_center()
            .child(
                v_flex()
                    .w(px(360.))
                    .p_4()
                    .gap_3()
                    .bg(theme.background)
                    .border_1()
                    .border_color(theme.border)
                    .rounded_lg()
                    .child(
                        div()
                            .text_size(px(14.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(rust_i18n::t!("notes.confirm_delete").to_string()),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.muted_foreground)
                            .child(msg),
                    )
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .justify_end()
                            .child(
                                Button::new("del-cancel")
                                    .small()
                                    .ghost()
                                    .child(rust_i18n::t!("common.cancel").to_string())
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.pending_delete = None;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Button::new("del-confirm")
                                    .small()
                                    .danger()
                                    .child(rust_i18n::t!("common.delete").to_string())
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.delete_note(&id, window, cx);
                                    })),
                            ),
                    ),
            )
            .into_any_element()
    }

    /// Render the slash palette overlay as a deferred popover.
    fn render_slash_palette(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let view = cx.entity();
        let filtered = self.filtered_command_defs();

        let mut items: Vec<AnyElement> = Vec::new();

        if filtered.is_empty() {
            items.push(
                div()
                    .px_3()
                    .py_2()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("No matching commands")
                    .into_any_element(),
            );
        } else {
            // Split recent vs other if there are recent items.
            let mut shown_recent_header = false;
            let mut shown_all_header = false;
            for (idx, cmd) in filtered.iter().enumerate() {
                let is_recent = self.store.recent_commands.iter().any(|r| r == cmd.id);
                if is_recent && !shown_recent_header {
                    items.push(
                        div()
                            .px_3()
                            .pt(px(6.))
                            .pb(px(2.))
                            .text_size(px(10.))
                            .text_color(theme.muted_foreground)
                            .child("RECENT")
                            .into_any_element(),
                    );
                    shown_recent_header = true;
                } else if !is_recent && !shown_all_header && shown_recent_header {
                    items.push(
                        div()
                            .px_3()
                            .pt(px(6.))
                            .pb(px(2.))
                            .text_size(px(10.))
                            .text_color(theme.muted_foreground)
                            .child("COMMANDS")
                            .into_any_element(),
                    );
                    shown_all_header = true;
                }

                let selected = idx == self.slash_selected;
                let cmd_id = cmd.id;
                let view_for_click = view.clone();
                items.push(
                    h_flex()
                        .w_full()
                        .px_3()
                        .py_1()
                        .gap_2()
                        .items_center()
                        .rounded_sm()
                        .cursor_pointer()
                        .when(selected, |d| d.bg(theme.accent.opacity(0.4)))
                        .when(!selected, |d| d.hover(|d| d.bg(theme.muted)))
                        .on_mouse_down(
                            MouseButton::Left,
                            move |_ev, _window, cx: &mut App| {
                                cx.stop_propagation();
                                let _ = view_for_click.update(cx, |this, cx| {
                                    // Find this command's id in the current filtered list
                                    // and set it as selected, then queue execution.
                                    let filtered = this.filtered_command_defs();
                                    if let Some(pos) = filtered.iter().position(|c| c.id == cmd_id) {
                                        this.slash_selected = pos;
                                    }
                                    this.queue_selected_command(cx);
                                });
                            },
                        )
                        .child(
                            Icon::new(cmd.icon.clone())
                                .size(px(14.))
                                .text_color(if selected {
                                    theme.foreground
                                } else {
                                    theme.muted_foreground
                                }),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(1.))
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme.foreground)
                                        .child(cmd.label),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .child(cmd.desc),
                                ),
                        )
                        .into_any_element(),
                );
            }
        }

        let palette_left = if self.list_collapsed { px(40.) } else { px(270.) };
        deferred(
            div()
                .id("slash-palette")
                .absolute()
                .left(palette_left)
                .top(px(120.))
                .w(px(300.))
                .max_h(px(340.))
                .overflow_y_scroll()
                .py_1()
                .bg(theme.background)
                .border_1()
                .border_color(theme.border)
                .rounded_md()
                .shadow_md()
                .children(items)
                .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                    this.close_slash_palette();
                    cx.notify();
                })),
        )
        .into_any_element()
    }

    fn render_editor(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mode = self.store.view_mode.clone();
        let editor_content = self.editor.read(cx).value().to_string();

        let active_id = self.store.active_note.clone();
        let active_note = active_id
            .as_deref()
            .and_then(|id| self.store.notes.iter().find(|n| n.id == id));

        let editing_active =
            matches!(&self.editing_title_id, Some(eid) if Some(eid.as_str()) == active_id.as_deref());

        // Two modes: "source" (raw markdown editor) and "live" (default, rendered view).
        // Anything other than "source" (including legacy "split"/"preview"/"edit") is live.
        let is_source = mode == "source";

        // Build the single content pane.
        let content: AnyElement = if is_source {
            // Source mode: monospace markdown editor with slash-command palette.
            let mut editor_container = div()
                .w_full()
                .h_full()
                .id("notes-editor-scroll")
                .overflow_y_scroll()
                .p_3();
            if self.slash_open {
                // Intercept Input's own MoveUp/MoveDown/Enter/Escape when palette
                // is open so arrow keys navigate the palette instead of the
                // editor, and Enter selects the highlighted command.
                editor_container = editor_container
                    .on_action(cx.listener(|this: &mut Self, _: &MoveUp, _, cx| {
                        this.move_slash_selection(-1, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|this: &mut Self, _: &MoveDown, _, cx| {
                        this.move_slash_selection(1, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|this: &mut Self, _: &Enter, _, cx| {
                        this.queue_selected_command(cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|this: &mut Self, _: &Escape, _, cx| {
                        this.close_slash_palette();
                        cx.notify();
                        cx.stop_propagation();
                    }));
            }
            editor_container
                .child(
                    Input::new(&self.editor)
                        .w_full()
                        .h_full()
                        .font_family(theme.mono_font_family.clone())
                        .text_size(px(13.)),
                )
                .into_any_element()
        } else {
            // Live mode: horizontal split — source editor on the left, live
            // rendered preview on the right, separated by a draggable divider
            // (h_resizable). Arrow keys/Enter in the editor are intercepted
            // via on_action when the slash palette is open.
            use gpui_component::resizable::{h_resizable, resizable_panel};

            let mut left = div()
                .id("notes-live-editor")
                .w_full()
                .h_full()
                .overflow_y_scroll()
                .p_3();
            if self.slash_open {
                left = left
                    .on_action(cx.listener(|this: &mut Self, _: &MoveUp, _, cx| {
                        this.move_slash_selection(-1, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|this: &mut Self, _: &MoveDown, _, cx| {
                        this.move_slash_selection(1, cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|this: &mut Self, _: &Enter, _, cx| {
                        this.queue_selected_command(cx);
                        cx.stop_propagation();
                    }))
                    .on_action(cx.listener(|this: &mut Self, _: &Escape, _, cx| {
                        this.close_slash_palette();
                        cx.notify();
                        cx.stop_propagation();
                    }));
            }
            let left = left.child(
                Input::new(&self.editor)
                    .w_full()
                    .h_full()
                    .font_family(theme.mono_font_family.clone())
                    .text_size(px(13.)),
            );

            let right = div()
                .id("notes-live-preview")
                .w_full()
                .h_full()
                .overflow_y_scroll()
                .p_4()
                .child(markdown_view::render_markdown(&editor_content, cx));

            h_resizable("notes-live-split")
                .child(
                    resizable_panel()
                        .size(px(480.))
                        .size_range(px(200.)..px(2000.))
                        .child(left),
                )
                .child(resizable_panel().child(right))
                .into_any_element()
        };

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child({
                let mut header =
                    h_flex().w_full().px_3().py_2().border_b_1().border_color(theme.border).gap_2();
                if editing_active {
                    header = header.child(Input::new(&self.title_input).w_full().small().appearance(false));
                } else {
                    let (title_display, current_title) = match &active_note {
                        Some(n) if !n.title.is_empty() => (n.title.clone(), n.title.clone()),
                        Some(_) => (
                            rust_i18n::t!("notes.untitled").to_string(),
                            String::new(),
                        ),
                        None => (rust_i18n::t!("notes.empty_editor").to_string(), String::new()),
                    };
                    let can_edit = active_note.is_some();
                    let nid = active_id.clone().unwrap_or_default();
                    let export_open = self.export_popover_open;
                    header = header
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .cursor_pointer()
                                .when(can_edit, |d| {
                                    d.on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.start_title_edit(&nid, &current_title, window, cx);
                                        }),
                                    )
                                })
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .child(title_display),
                                ),
                        )
                        // Per-note export popover (MD/PDF).
                        .when(can_edit, |h| {
                            h.child(
                                Popover::new("notes-export-pop")
                                    .anchor(gpui::Anchor::BottomRight)
                                    .open(export_open)
                                    .on_open_change(cx.listener(|this, open: &bool, _, cx| {
                                        this.export_popover_open = *open;
                                        cx.notify();
                                    }))
                                    .trigger(
                                        Button::new("notes-export-trig")
                                            .ghost()
                                            .xsmall()
                                            .icon({
                                                let mut icon = Icon::new(IconName::ExternalLink);
                                                icon = icon.path(crate::assets::EXPORT);
                                                icon
                                            })
                                            .tooltip(rust_i18n::t!("notes.export").to_string()),
                                    )
                                    .p(px(4.))
                                    .child(
                                        v_flex()
                                            .w(px(170.))
                                            .gap(px(2.))
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(theme.muted))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .child(rust_i18n::t!("notes.export_md").to_string()),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, window, cx| {
                                                            this.export_popover_open = false;
                                                            this.export_active_note_as("md", window, cx);
                                                        }),
                                                    ),
                                            )
                                            .child(
                                                div()
                                                    .px_2()
                                                    .py_1()
                                                    .rounded_md()
                                                    .cursor_pointer()
                                                    .hover(|s| s.bg(theme.muted))
                                                    .child(
                                                        div()
                                                            .text_size(px(12.))
                                                            .child(rust_i18n::t!("notes.export_pdf").to_string()),
                                                    )
                                                    .on_mouse_down(
                                                        MouseButton::Left,
                                                        cx.listener(|this, _, window, cx| {
                                                            this.export_popover_open = false;
                                                            this.export_active_note_as("pdf", window, cx);
                                                        }),
                                                    ),
                                            ),
                                    ),
                            )
                        });
                }
                header
            })
            .child(div().w_full().flex_1().min_h_0().child(content))
            .into_any_element()
    }
}

impl Render for NotesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Run any deferred slash command BEFORE rendering so the editor has the
        // updated text (we have &mut Window here).
        if self.pending_slash_execute.is_some() {
            self.run_pending_command(window, cx);
        }

        // If the tree selection changed without a Window context (observer),
        // reload the editor content from the active note here where we have
        // window access.
        if self.editor_needs_reload {
            self.editor_needs_reload = false;
            if let Some(id) = self.store.active_note.clone() {
                if let Some(note) = self.store.notes.iter().find(|n| n.id == id) {
                    let content = note.content.clone();
                    self.editor.update(cx, |e, cx| {
                        e.set_value(content, window, cx);
                    });
                }
            }
        }

        let theme = cx.theme();

        let mut root = div()
            .w_full()
            .h_full()
            .relative()
            .bg(theme.background)
            .child(
                h_flex()
                    .w_full()
                    .h_full()
                    .when(!self.list_collapsed, |h| {
                        h.child(self.render_note_list(cx))
                    })
                    .when(self.list_collapsed, |h| {
                        // Collapsed: show a thin vertical strip with an expand button.
                        h.child(
                            v_flex()
                                .w(px(28.))
                                .h_full()
                                .flex_shrink_0()
                                .border_r_1()
                                .border_color(theme.border)
                                .bg(theme.muted.opacity(0.3))
                                .items_center()
                                .pt_2()
                                .child(
                                    Button::new("notes-expand-list")
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::PanelLeftOpen)
                                        .tooltip(rust_i18n::t!("notes.expand_list").to_string())
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.list_collapsed = false;
                                            cx.notify();
                                        })),
                                ),
                        )
                    })
                    .child(self.render_editor(window, cx)),
            );

        if self.slash_open {
            root = root.child(self.render_slash_palette(cx));
        }

        if let Some(id) = &self.pending_delete {
            root = root.child(self.render_delete_confirm(id, cx));
        }

        root
    }
}

/// Replace path-unsafe characters in a filename with underscores.
fn sanitize_filename(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' | '\0' => out.push('_'),
            c if c.is_control() => out.push('_'),
            c => out.push(c),
        }
    }
    out.trim().trim_matches('.').to_string()
}
