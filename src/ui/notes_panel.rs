//! Notepad panel — markdown notes with live split preview.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{h_flex, v_flex, ActiveTheme, IconName, Sizable as _, Selectable as _};

use crate::notes::{self, Note, NotesStore};
use crate::state::{AppEvent, AppState};
use crate::ui::markdown_view;

pub struct NotesPanel {
    state: Entity<AppState>,
    store: NotesStore,
    editor: Entity<InputState>,
    search: Entity<InputState>,
    title_input: Entity<InputState>,
    /// The note id currently being renamed inline (shown as an Input in the
    /// header). `None` when no title edit is in progress.
    editing_title_id: Option<String>,
    pending_delete: Option<String>,
    save_timer: Option<Task<()>>,
    _subs: Vec<Subscription>,
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

        Self {
            state,
            store,
            editor,
            search,
            title_input,
            editing_title_id: None,
            pending_delete: None,
            save_timer: None,
            _subs: vec![editor_sub, search_sub, title_sub],
        }
    }

    fn on_editor_event(
        &mut self,
        _: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            self.schedule_save(cx);
        }
    }

    fn on_search_event(
        &mut self,
        _: Entity<InputState>,
        _event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        cx.notify();
    }

    /// Title-input events: commit on Enter, cancel on Blur.
    fn on_title_event(
        &mut self,
        _: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        match event {
            InputEvent::PressEnter { .. } => {
                self.commit_title_edit(cx);
            }
            InputEvent::Blur => {
                self.commit_title_edit(cx);
            }
            _ => {}
        }
    }

    fn commit_title_edit(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editing_title_id.take() else {
            return;
        };
        let new_title = self.title_input.read(cx).value().to_string();
        notes::update_note_title(&mut self.store, &id, new_title);
        let _ = crate::state::persistence::save_notes(&self.store);
        self.state.update(cx, |s, cx| {
            s.dirty = true;
            cx.emit(AppEvent::Persisted);
        });
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
            let _ = crate::state::persistence::save_notes(&self.store);
            self.state.update(cx, |s, cx| {
                s.dirty = true;
                cx.emit(AppEvent::Persisted);
            });
        }
    }

    fn new_note(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let note = notes::create_note(&mut self.store, String::new());
        let new_id = note.id.clone();
        self.store.active_note = Some(new_id.clone());
        self.editor.update(cx, |e, cx| {
            e.set_value(String::new(), window, cx);
        });
        let _ = crate::state::persistence::save_notes(&self.store);
        // Immediately enter title-editing mode for the new note so the user
        // can type a name without an extra click.
        self.start_title_edit(&new_id, "", window, cx);
    }

    fn select_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Commit any in-progress title edit before switching notes.
        if self.editing_title_id.is_some() {
            self.commit_title_edit(cx);
        }
        self.save_current(cx);
        self.store.active_note = Some(id.to_string());
        if let Some(note) = self.store.notes.iter().find(|n| n.id == id) {
            self.editor.update(cx, |e, cx| {
                e.set_value(note.content.clone(), window, cx);
            });
        }
        cx.notify();
    }

    fn delete_note(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing_title_id.is_some() {
            self.commit_title_edit(cx);
        }
        notes::delete_note(&mut self.store, id);
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
        cx.notify();
    }

    fn set_view_mode(&mut self, mode: &str, cx: &mut Context<Self>) {
        self.store.view_mode = mode.to_string();
        let _ = crate::state::persistence::save_notes(&self.store);
        cx.notify();
    }

    fn render_note_list(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let search_val = self.search.read(cx).value().to_string().to_lowercase();

        let filtered: Vec<&Note> = self
            .store
            .notes
            .iter()
            .filter(|n| {
                search_val.is_empty()
                    || n.title.to_lowercase().contains(&search_val)
                    || n.content.to_lowercase().contains(&search_val)
            })
            .collect();

        let note_items: Vec<AnyElement> = if filtered.is_empty() {
            vec![
                div()
                    .p_3()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .child(rust_i18n::t!("notes.empty").to_string())
                    .into_any_element(),
            ]
        } else {
            filtered
                .iter()
                .map(|note| {
                    let id = note.id.clone();
                    let delete_id = note.id.clone();
                    let is_active = self.store.active_note.as_deref() == Some(&note.id);
                    let title = if note.title.is_empty() {
                        rust_i18n::t!("notes.untitled").to_string()
                    } else {
                        note.title.clone()
                    };
                    let updated = rust_i18n::t!(
                        "notes.last_updated",
                        time = note.updated_at.as_str()
                    )
                    .to_string();
                    h_flex()
                        .w_full()
                        .px_3()
                        .py_2()
                        .gap_2()
                        .items_center()
                        .cursor_pointer()
                        .when(is_active, |d| d.bg(theme.accent.opacity(0.3)))
                        .when(!is_active, |d| d.hover(|d| d.bg(theme.muted)))
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .on_mouse_down(
                                    MouseButton::Left,
                                    cx.listener(move |this, _, window, cx| {
                                        this.select_note(&id, window, cx);
                                    }),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .text_color(theme.foreground)
                                        .truncate()
                                        .child(title),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.muted_foreground)
                                        .truncate()
                                        .child(updated),
                                ),
                        )
                        .child(
                            Button::new(format!("notes-del-{}", note.id))
                                .ghost()
                                .small()
                                .icon(IconName::Delete)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.pending_delete = Some(delete_id.clone());
                                    cx.notify();
                                })),
                        )
                        .into_any_element()
                })
                .collect()
        };

        v_flex()
            .w(px(240.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.muted.opacity(0.3))
            .child(
                h_flex()
                    .w_full()
                    .p_2()
                    .gap_2()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        Button::new("notes-new")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .child(rust_i18n::t!("notes.new_note").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.new_note(window, cx);
                            })),
                    )
                    .child(Input::new(&self.search).w_full().flex_1().small()),
            )
            .child(
                v_flex()
                    .id("notes-list-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(note_items),
            )
            .into_any_element()
    }

    fn render_delete_confirm(&self, note_id: &str, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let note_title = self
            .store
            .notes
            .iter()
            .find(|n| n.id == note_id)
            .map(|n| n.title.clone())
            .unwrap_or_default();

        let msg = rust_i18n::t!(
            "notes.confirm_delete_msg",
            title = note_title.as_str()
        )
        .to_string();
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

    fn render_editor(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let mode = self.store.view_mode.clone();
        let editor_content = self.editor.read(cx).value().to_string();

        let active_id = self.store.active_note.clone();
        let active_note = active_id.as_deref().and_then(|id| {
            self.store.notes.iter().find(|n| n.id == id)
        });

        // If we're actively editing the title of the current note, render an
        // Input in the header; otherwise render the display title (clickable
        // to start editing).
        let editing_active = matches!(&self.editing_title_id, Some(eid) if Some(eid.as_str()) == active_id.as_deref());

        let mode_split = mode == "split";
        let mode_edit = mode == "edit";
        let mode_preview = mode == "preview";

        // Build the main content (editor + preview) conditionally.
        let mut content_children: Vec<AnyElement> = Vec::new();
        if !mode_preview {
            content_children.push(
                div()
                    .w_full()
                    .h_full()
                    .id("notes-editor-scroll")
                    .overflow_y_scroll()
                    .p_3()
                    .child(
                        Input::new(&self.editor)
                            .w_full()
                            .h_full()
                            .font_family(theme.mono_font_family.clone())
                            .text_size(px(13.)),
                    )
                    .into_any_element(),
            );
        }
        if mode_split {
            content_children.push(div().w(px(1.)).bg(theme.border).into_any_element());
        }
        if !mode_edit {
            content_children.push(
                div()
                    .w_full()
                    .h_full()
                    .id("notes-preview-scroll")
                    .overflow_y_scroll()
                    .p_3()
                    .child(markdown_view::render_markdown(&editor_content, cx))
                    .into_any_element(),
            );
        }

        v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .child({
                let header = h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .border_b_1()
                    .border_color(theme.border);
                if editing_active {
                    header.child(
                        Input::new(&self.title_input)
                            .w_full()
                            .small()
                            .appearance(false),
                    )
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
                    header
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
                        )
                }
            })
            .child(
                h_flex()
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .children(content_children),
            )
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .border_t_1()
                    .border_color(theme.border)
                    .justify_center()
                    .gap_1()
                    .child(
                        Button::new("mode-split")
                            .small()
                            .selected(mode_split)
                            .child(rust_i18n::t!("notes.split_view").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view_mode("split", cx);
                            })),
                    )
                    .child(
                        Button::new("mode-edit")
                            .small()
                            .selected(mode_edit)
                            .child(rust_i18n::t!("notes.edit_view").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view_mode("edit", cx);
                            })),
                    )
                    .child(
                        Button::new("mode-preview")
                            .small()
                            .selected(mode_preview)
                            .child(rust_i18n::t!("notes.preview_view").to_string())
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.set_view_mode("preview", cx);
                            })),
                    ),
            )
            .into_any_element()
    }
}

impl Render for NotesPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
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
                    .child(self.render_note_list(cx))
                    .child(self.render_editor(window, cx)),
            );

        if let Some(id) = &self.pending_delete {
            root = root.child(self.render_delete_confirm(id, cx));
        }

        root
    }
}
