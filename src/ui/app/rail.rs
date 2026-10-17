//! The far-left activity rail (postman-style icon sidebar): rail icons,
//! drag-to-reorder, drop indicators, brand mark, settings + theme picker.
//! Also hosts `refresh_switchers` (rebuilds project/env switcher lists).

use super::widgets::vicon;
use super::{PendingDialog, RailDrag, SideView, VerveApp};
use crate::assets::{
    BRACES_JSON, DOCKER as DOCKER_ICON, DOCS, HELP, HISTORY, K8S as K8S_ICON, KANBAN, MARKDOWN,
    NOTEBOOK, SERVER,
};
use crate::state::AppEvent;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    ActiveTheme, Icon, IconName, Selectable as _, Sizable as _,
    Size::Medium,
    button::{Button, ButtonVariants as _},
    h_flex,
    popover::Popover,
    v_flex,
};

/// Shortcut hint for the i-th of `SideView::SHORTCUT_VIEWS`: ⌘N on macOS,
/// Ctrl+N elsewhere (N = i + 1). Also used by the help center's feature
/// cards.
pub(crate) fn rail_slot_hint(i: usize) -> String {
    let digit = char::from(b'1' + i.min(8) as u8);
    if cfg!(target_os = "macos") {
        format!("⌘{digit}")
    } else {
        format!("Ctrl+{digit}")
    }
}

impl VerveApp {
    pub(super) fn refresh_switchers(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let st = self.state.read(cx);
        let project_names: Vec<String> = st.data.projects.iter().map(|p| p.name.clone()).collect();
        let active_idx = st.active_project;
        let env_names: Vec<String> = match st.active_project() {
            Some(p) => {
                let mut v = vec!["No environment".to_string()];
                v.extend(p.environments.iter().map(|e| e.name.clone()));
                v
            }
            None => vec!["No environment".to_string()],
        };
        self.project_select.update(cx, |s, cx| {
            s.set_items(project_names, window, cx);
            s.set_selected_index(Some(gpui_component::IndexPath::new(active_idx)), window, cx);
        });
        self.env_select.update(cx, |s, cx| {
            s.set_items(env_names, window, cx);
        });
    }

    pub(crate) fn rail_icon_for(view: SideView) -> Icon {
        match view {
            SideView::Api => IconName::Network.into(),
            SideView::AutoTest => IconName::Play.into(),
            SideView::Stress => IconName::ChartPie.into(),
            SideView::Proxy => IconName::Globe.into(),
            SideView::Share => vicon(DOCS),
            SideView::K8s => vicon(K8S_ICON),
            SideView::Docker => vicon(DOCKER_ICON),
            SideView::Hosts => vicon(SERVER),
            SideView::Notes => vicon(NOTEBOOK),
            SideView::MarkdownEditor => vicon(MARKDOWN),
            SideView::ProjectManage => vicon(KANBAN),
            SideView::JsonFormat => vicon(BRACES_JSON),
            SideView::Mock => IconName::Cpu.into(),
            SideView::History => vicon(HISTORY),
            SideView::Ssh => IconName::SquareTerminal.into(),
        }
    }

    pub(super) fn apply_rail_drop(
        &mut self,
        dragged_name: &str,
        target_name: Option<&str>,
        cx: &mut Context<Self>,
    ) {
        let mut order = self.rail_order.clone();
        // Don't drop on top of yourself.
        order.retain(|n| n != dragged_name);
        match target_name {
            Some(target) => {
                if let Some(idx) = order.iter().position(|n| n == target) {
                    order.insert(idx, dragged_name.to_string());
                } else {
                    order.push(dragged_name.to_string());
                }
            }
            None => order.push(dragged_name.to_string()),
        }
        self.rail_order = order.clone();
        crate::state::persistence::save_rail_order(&order);
        self.state.update(cx, |s, cx| {
            s.dirty = true;
            cx.emit(AppEvent::Persisted);
        });
    }

    pub(super) fn set_rail_drop_target(&mut self, target: Option<String>, cx: &mut Context<Self>) {
        if self.rail_drop_target != target {
            self.rail_drop_target = target;
            cx.notify();
        }
    }

    /// The rail-selectable views in the user's custom order (`rail_order`,
    /// minus the home view and hidden rails) — exactly what the activity rail
    /// renders.
    pub(super) fn visible_rail_views(&self) -> Vec<SideView> {
        self.rail_order
            .iter()
            .filter_map(|name| {
                let v = SideView::parse(name);
                // Filter home view and hidden views.
                if v != self.home_view && !self.hidden_rails.contains(v.name()) {
                    Some(v)
                } else {
                    None
                }
            })
            .collect()
    }

    /// Switch the workbench to `view`, lazily activating panels that shouldn't
    /// connect at app startup. Shared by the rail buttons and the rail-switch
    /// keyboard shortcuts.
    pub(super) fn activate_view(
        &mut self,
        view: SideView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_view = view;
        match view {
            SideView::Docker => {
                self.docker.update(cx, |d, cx| d.activate(cx));
            }
            SideView::K8s => {
                self.k8s.update(cx, |k, cx| k.activate(window, cx));
            }
            // Focus the panel root so its "JsonPanel" key context is active —
            // Cmd/Ctrl+F opens the search bar immediately after switching,
            // without requiring a click inside the view first.
            SideView::JsonFormat => {
                let handle = self.json.read(cx).focus_handle(cx);
                window.focus(&handle, cx);
            }
            _ => {}
        }
        cx.notify();
    }

    pub(super) fn render_activity_rail(&mut self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let active = self.active_view;
        let entity = cx.entity();

        let ordered_views = self.visible_rail_views();

        let entity_cancel = entity.clone();
        v_flex()
            .w(px(44.))
            .flex_shrink_0()
            .h_full()
            .py_2()
            .gap_1()
            .items_center()
            .bg(theme.muted)
            // Catch-all drop: if the user releases over a non-target area
            // (e.g. the brand mark, settings button, or empty gaps), just
            // cancel the drag cleanly instead of leaving stale state.
            .on_drop(move |_drag: &RailDrag, _window, cx: &mut App| {
                let _ = entity_cancel.update(cx, |this, cx| {
                    this.dragging_rail = None;
                    this.rail_drop_target = None;
                    cx.notify();
                });
            })
            // Brand mark "V"
            .child({
                let p = theme.primary;
                let hover_bg = gpui::hsla(p.h, p.s, (p.l + 0.12).min(1.0), p.a);
                div()
                    .id("brand-mark")
                    .size_8()
                    .mb_2()
                    .rounded(theme.radius)
                    .bg(theme.primary)
                    .text_color(theme.primary_foreground)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .text_size(px(16.))
                    .hover(move |d| d.bg(hover_bg))
                    .child(div().font_weight(FontWeight::BOLD).child("V"))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.active_view = this.home_view;
                        cx.notify();
                    }))
            })
            // Rail buttons in custom order with drag-and-drop support.
            .children(ordered_views.iter().map(|&view| {
                let view_name = view.name().to_string();
                let is_active = active == view;
                let is_dragging = self.dragging_rail.as_deref() == Some(view_name.as_str());
                let is_drop_target = self.rail_drop_target.as_deref() == Some(view_name.as_str());
                // Fixed shortcut (⌘1..⌘5), if this view has one.
                let shortcut_hint = SideView::SHORTCUT_VIEWS
                    .iter()
                    .position(|&v| v == view)
                    .map(rail_slot_hint);

                let btn = Button::new(format!("rail-btn-{}", view.name()))
                    .ghost()
                    .with_size(Medium)
                    .icon(Self::rail_icon_for(view))
                    .selected(is_active)
                    .tooltip(match shortcut_hint {
                        Some(hint) => format!("{} ({})", view.label(), hint),
                        None => view.label(),
                    })
                    .when(is_active, |btn| {
                        btn.bg(theme.accent.opacity(0.5))
                            .text_color(theme.foreground)
                    })
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.activate_view(view, window, cx);
                    }));

                let drop_name = view_name.clone();
                let drop_name_hover = view_name.clone();

                let drag_payload = RailDrag(view_name.clone());
                let entity_drag = entity.clone();
                let entity_drop = entity.clone();
                let entity_hover = entity.clone();

                div()
                    .id(format!("rail-{}", view.name()))
                    .relative()
                    .when(is_dragging, |d| d.opacity(0.4))
                    // Draw a thick, vivid indicator line above the hovered item
                    // (insert-before marker).
                    .when(is_drop_target, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_1()
                                .right_1()
                                .h(px(3.))
                                .rounded(px(2.))
                                .bg(theme.primary),
                        )
                    })
                    .on_drag(drag_payload, move |drag: &RailDrag, _pos, _window, cx| {
                        let _ = entity_drag.update(cx, |this, cx| {
                            this.dragging_rail = Some(drag.0.clone());
                            cx.notify();
                        });
                        cx.new(|_| drag.clone())
                    })
                    // Live hover tracking — update drop target as the cursor
                    // moves so the indicator re-renders in real time.
                    .on_hover(move |hovering: &bool, _window, cx: &mut App| {
                        let target = drop_name_hover.clone();
                        let _ = entity_hover.update(cx, |this, cx| {
                            if *hovering && this.dragging_rail.is_some() {
                                this.set_rail_drop_target(Some(target), cx);
                            }
                        });
                    })
                    .on_drop(move |drag: &RailDrag, _window, cx: &mut App| {
                        let source = drag.0.clone();
                        let target = drop_name.clone();
                        let _ = entity_drop.update(cx, |this, cx| {
                            // Only reorder when dropped onto a *different* item.
                            if source != target {
                                this.apply_rail_drop(&source, Some(target.as_str()), cx);
                            }
                            this.dragging_rail = None;
                            this.rail_drop_target = None;
                            cx.notify();
                        });
                    })
                    .child(btn)
            }))
            // End-of-list drop zone: fills the remaining space below the icons
            // so dragging anywhere past the last item places the dragged item
            // at the very end (fixes the "can't drag to last" bug).
            .child({
                let is_end_target = self.rail_drop_target.as_deref() == Some("__rail_end__");
                let entity_end = entity.clone();
                let entity_end_drop = entity.clone();
                div()
                    .id("rail-end-zone")
                    .w_full()
                    .flex_1()
                    .min_h(px(20.))
                    .relative()
                    .when(is_end_target, |d| {
                        d.child(
                            div()
                                .absolute()
                                .top_0()
                                .left_1()
                                .right_1()
                                .h(px(3.))
                                .rounded(px(2.))
                                .bg(theme.primary),
                        )
                    })
                    .on_hover(move |hovering: &bool, _window, cx: &mut App| {
                        let _ = entity_end.update(cx, |this, cx| {
                            if *hovering && this.dragging_rail.is_some() {
                                this.set_rail_drop_target(Some("__rail_end__".to_string()), cx);
                            }
                        });
                    })
                    .on_drop(move |drag: &RailDrag, _window, cx: &mut App| {
                        let source = drag.0.clone();
                        let _ = entity_end_drop.update(cx, |this, cx| {
                            this.apply_rail_drop(&source, None, cx);
                            this.dragging_rail = None;
                            this.rail_drop_target = None;
                            cx.notify();
                        });
                    })
            })
            // Separator + Help center + Settings + theme picker.
            .child(
                div()
                    .w(px(24.))
                    .h(px(1.))
                    .my_1()
                    .bg(theme.border.opacity(0.5)),
            )
            .child({
                let hint = if cfg!(target_os = "macos") {
                    "⌘/"
                } else {
                    "Ctrl+/"
                };
                Button::new("rail-help")
                    .ghost()
                    .with_size(Medium)
                    .icon(vicon(HELP))
                    .tooltip(format!(
                        "{} ({hint})",
                        rust_i18n::t!("help.open_hint")
                    ))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.open_help_center(cx);
                    }))
            })
            .child(
                Button::new("rail-settings")
                    .ghost()
                    .with_size(Medium)
                    .icon(IconName::Settings)
                    .tooltip(rust_i18n::t!("settings.title").to_string())
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.pending_dialog = Some(PendingDialog::Settings);
                        cx.notify();
                    })),
            )
            .child({
                let open = self.theme_popover_open;
                let theme_name = crate::ui::themes::current_theme_name(cx);
                Popover::new("theme-picker")
                    .anchor(gpui::Anchor::TopCenter)
                    .open(open)
                    .on_open_change(cx.listener(|this, open, _, cx| {
                        this.theme_popover_open = *open;
                        cx.notify();
                    }))
                    .trigger(
                        Button::new("rail-theme")
                            .ghost()
                            .with_size(Medium)
                            .icon(IconName::Palette)
                            .tooltip(format!("主题：{}", theme_name)),
                    )
                    .p(px(4.))
                    .child({
                        let names = crate::ui::themes::theme_names(cx);
                        let active = theme_name.clone();
                        let entity = cx.entity();
                        let theme_fg = cx.theme().muted_foreground;
                        let theme_accent = cx.theme().accent;
                        let cols = 3usize;
                        let per = names.len().div_ceil(cols);
                        let build_items =
                            |slice: &[String],
                             offset: usize,
                             active: &str,
                             ent: &gpui::Entity<VerveApp>| {
                                v_flex().w(px(140.)).gap(px(1.)).children(
                                    slice.iter().enumerate().map(move |(i, name)| {
                                        let n = name.clone();
                                        let is_active = n == active;
                                        let ent = ent.clone();
                                        div()
                                            .id(format!("theme-opt-{}", offset + i))
                                            .w_full()
                                            .px(px(8.))
                                            .py(px(4.))
                                            .rounded(px(4.))
                                            .text_size(px(12.))
                                            .when(is_active, |d| {
                                                d.bg(gpui::black().opacity(0.3))
                                                    .text_color(gpui::white())
                                            })
                                            .when(!is_active, |d| d.text_color(theme_fg))
                                            .hover(|d| d.bg(theme_accent.opacity(0.3)))
                                            .child(n.clone())
                                            .on_click(move |_, _window, cx: &mut App| {
                                                crate::ui::themes::apply_theme(&n, cx);
                                                let _ = ent.update(cx, |this, cx| {
                                                    this.theme_popover_open = false;
                                                    cx.notify();
                                                });
                                            })
                                    }),
                                )
                            };
                        h_flex()
                            .gap(px(4.))
                            .max_h(px(420.))
                            .child(build_items(&names[..per], 0, &active, &entity))
                            .child(build_items(&names[per..per * 2], per, &active, &entity))
                            .child(build_items(&names[per * 2..], per * 2, &active, &entity))
                    })
            })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{SideView, rail_slot_keybindings};
    // Import only what's under test — a `use super::*` glob would pull in
    // gpui's own `test` attribute macro (via the parent's `use gpui::*`) and
    // shadow the builtin `#[test]`.
    use super::rail_slot_hint;

    #[test]
    fn rail_slot_hint_digits() {
        for (i, _) in SideView::SHORTCUT_VIEWS.iter().enumerate() {
            let digit = char::from(b'1' + i as u8);
            let expected = if cfg!(target_os = "macos") {
                format!("⌘{digit}")
            } else {
                format!("Ctrl+{digit}")
            };
            assert_eq!(rail_slot_hint(i), expected);
        }
    }

    /// `KeyBinding::new` panics on an invalid keystroke string at app
    /// startup, so make sure every rail-switch binding constructs.
    #[test]
    fn rail_slot_keybindings_parse() {
        let n = SideView::SHORTCUT_VIEWS.len();
        assert_eq!(rail_slot_keybindings("cmd").len(), n);
        assert_eq!(rail_slot_keybindings("ctrl").len(), n);
    }
}
