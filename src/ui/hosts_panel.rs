//! Hosts Quick Editor panel — manage hosts profiles with enable/disable,
//! environment binding, search, and apply to system or virtual override.

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::{
    ActiveTheme, Disableable as _, IconName, Selectable as _, Sizable as _, h_flex, v_flex,
};

use crate::hosts::{self, HostEntry};
use crate::hosts_priv;
use crate::hosts_profiles::{self, HostEntryEdit, HostsProfileStore};
use crate::state::{AppEvent, AppState};

pub struct HostsPanel {
    state: Entity<AppState>,
    store: HostsProfileStore,
    search_input: Entity<InputState>,
    entry_inputs: Vec<EntryInput>,
    apply_status: ApplyStatus,
    /// Cached system hosts entries (re-read on refresh / after apply).
    system_entries: Vec<HostEntry>,
    system_error: Option<String>,
    /// Resizable state for the left profile-list sidebar width.
    sidebar_resize: Entity<gpui_component::resizable::ResizableState>,
    _subs: Vec<Subscription>,
}

#[derive(Clone, Default)]
enum ApplyStatus {
    #[default]
    Idle,
    Appling,
    Success,
    Error(String),
}

struct EntryInput {
    entry_id: String,
    ip: Entity<InputState>,
    host: Entity<InputState>,
    comment: Entity<InputState>,
}

impl HostsPanel {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let store = crate::state::persistence::load_hosts_profiles();

        let search_input = cx.new(|cx| {
            let mut input = InputState::new(window, cx);
            input.set_value(String::new(), window, cx);
            input
        });

        let search_sub = cx.subscribe(&search_input, Self::on_search_event);

        let mut this = Self {
            state,
            store,
            search_input,
            entry_inputs: Vec::new(),
            apply_status: ApplyStatus::Idle,
            system_entries: Vec::new(),
            system_error: None,
            sidebar_resize: cx.new(|_| gpui_component::resizable::ResizableState::default()),
            _subs: vec![search_sub],
        };
        this.refresh_system_hosts();
        this.rebuild_entry_inputs(window, cx);
        this
    }

    fn refresh_system_hosts(&mut self) {
        match hosts::read_hosts() {
            Ok(entries) => {
                self.system_entries = entries;
                self.system_error = None;
            }
            Err(msg) => {
                self.system_entries.clear();
                self.system_error = Some(msg);
            }
        }
    }

    fn open_system_in_editor(&self) {
        match hosts::open_in_editor() {
            Ok(()) => {}
            Err(e) => log::error!("open hosts editor failed: {e}"),
        }
    }

    fn rebuild_entry_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.entry_inputs.clear();
        let active_entries: Vec<HostEntryEdit> = self
            .store
            .profiles
            .iter()
            .filter(|p| Some(&p.id) == self.store.active_profile.as_ref())
            .flat_map(|p| p.entries.clone())
            .collect();

        for entry in active_entries {
            let ip = cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_value(entry.ip.clone(), window, cx);
                s
            });
            let host = cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_value(entry.host.clone(), window, cx);
                s
            });
            let comment = cx.new(|cx| {
                let mut s = InputState::new(window, cx);
                s.set_value(entry.comment.clone().unwrap_or_default(), window, cx);
                s
            });
            let ip_clone = ip.clone();
            let host_clone = host.clone();
            let comment_clone = comment.clone();
            let eid = entry.id.clone();
            let ip_eid = eid.clone();
            let _ip_sub = cx.subscribe(&ip, move |this: &mut Self, _src, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change | InputEvent::Blur) {
                    let val = ip_clone.read(cx).value().to_string();
                    if let Some(p) = this
                        .store
                        .profiles
                        .iter_mut()
                        .find(|p| Some(&p.id) == this.store.active_profile.as_ref())
                    {
                        if let Some(e) = p.entries.iter_mut().find(|e| e.id == ip_eid) {
                            e.ip = val;
                            let _ = crate::state::persistence::save_hosts_profiles(&this.store);
                        }
                    }
                }
            });
            let host_eid = eid.clone();
            let host_clone2 = host.clone();
            let _host_sub = cx.subscribe(&host, move |this, _src, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change | InputEvent::Blur) {
                    let val = host_clone2.read(cx).value().to_string();
                    if let Some(p) = this
                        .store
                        .profiles
                        .iter_mut()
                        .find(|p| Some(&p.id) == this.store.active_profile.as_ref())
                    {
                        if let Some(e) = p.entries.iter_mut().find(|e| e.id == host_eid) {
                            e.host = val;
                            let _ = crate::state::persistence::save_hosts_profiles(&this.store);
                        }
                    }
                }
            });
            let comment_eid = eid.clone();
            let comment_clone2 = comment.clone();
            let _comment_sub = cx.subscribe(&comment, move |this, _src, ev: &InputEvent, cx| {
                if matches!(ev, InputEvent::Change | InputEvent::Blur) {
                    let val = comment_clone2.read(cx).value().to_string();
                    if let Some(p) = this
                        .store
                        .profiles
                        .iter_mut()
                        .find(|p| Some(&p.id) == this.store.active_profile.as_ref())
                    {
                        if let Some(e) = p.entries.iter_mut().find(|e| e.id == comment_eid) {
                            e.comment = if val.is_empty() { None } else { Some(val) };
                            let _ = crate::state::persistence::save_hosts_profiles(&this.store);
                        }
                    }
                }
            });
            self.entry_inputs.push(EntryInput {
                entry_id: entry.id,
                ip,
                host,
                comment,
            });
            let _ = ip_clone;
            let _ = host_clone;
            let _ = comment_clone;
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

    fn save(&self) {
        let _ = crate::state::persistence::save_hosts_profiles(&self.store);
    }

    fn add_profile(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        hosts_profiles::create_profile(&mut self.store, String::new());
        self.save();
        self.rebuild_entry_inputs(window, cx);
        cx.notify();
    }

    fn delete_profile(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        hosts_profiles::delete_profile(&mut self.store, id);
        self.save();
        self.rebuild_entry_inputs(window, cx);
        cx.notify();
    }

    fn select_profile(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.store.active_profile = Some(id.to_string());
        self.rebuild_entry_inputs(window, cx);
        cx.notify();
    }

    fn toggle_profile_enabled(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(p) = self.store.profiles.iter_mut().find(|p| p.id == id) {
            p.enabled = !p.enabled;
            self.save();
            self.state.update(cx, |s, cx| {
                s.dirty = true;
                cx.emit(AppEvent::Persisted);
            });
        }
        cx.notify();
    }

    fn add_entry(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let profile_id = self.store.active_profile.clone().unwrap_or_default();
        hosts_profiles::add_entry(&mut self.store, &profile_id);
        self.save();
        self.rebuild_entry_inputs(window, cx);
        cx.notify();
    }

    fn remove_entry(&mut self, entry_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(profile_id) = self.store.active_profile.clone() {
            hosts_profiles::remove_entry(&mut self.store, &profile_id, entry_id);
            self.save();
            self.rebuild_entry_inputs(window, cx);
        }
        cx.notify();
    }

    fn toggle_entry_enabled(&mut self, entry_id: &str, cx: &mut Context<Self>) {
        if let Some(p) = self
            .store
            .profiles
            .iter_mut()
            .find(|p| Some(&p.id) == self.store.active_profile.as_ref())
        {
            if let Some(e) = p.entries.iter_mut().find(|e| e.id == entry_id) {
                e.enabled = !e.enabled;
                self.save();
                self.state.update(cx, |s, cx| {
                    s.dirty = true;
                    cx.emit(AppEvent::Persisted);
                });
            }
        }
        cx.notify();
    }

    fn toggle_virtual(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self
            .store
            .profiles
            .iter_mut()
            .find(|p| Some(&p.id) == self.store.active_profile.as_ref())
        {
            p.apply_virtual = !p.apply_virtual;
            self.save();
        }
        cx.notify();
    }

    fn toggle_system(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self
            .store
            .profiles
            .iter_mut()
            .find(|p| Some(&p.id) == self.store.active_profile.as_ref())
        {
            p.apply_to_system = !p.apply_to_system;
            self.save();
        }
        cx.notify();
    }

    fn apply_to_system(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let active_env = self
            .state
            .read(cx)
            .active_project()
            .and_then(|p| p.active_environment.clone());

        let verve_block =
            hosts_profiles::render_enabled_entries(&self.store, active_env.as_deref());
        let existing = crate::hosts::read_hosts_string();
        let merged = hosts_profiles::merge_into_existing(&existing, &verve_block);

        self.apply_status = ApplyStatus::Appling;
        cx.notify();

        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { hosts_priv::write_system_hosts(&merged) })
                .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => {
                        this.apply_status = ApplyStatus::Success;
                        this.refresh_system_hosts();
                    }
                    Err(e) => {
                        this.apply_status = ApplyStatus::Error(e.to_string());
                    }
                }
                cx.notify();
                let weak = cx.weak_entity();
                cx.spawn(async move |_this, cx| {
                    cx.background_executor()
                        .timer(std::time::Duration::from_secs(3))
                        .await;
                    let _ = weak.update(cx, |this, cx| {
                        this.apply_status = ApplyStatus::Idle;
                        cx.notify();
                    });
                })
                .detach();
            });
        })
        .detach();
    }

    fn render_profile_list(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();

        v_flex()
            .w_full()
            .h_full()
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
                    .items_center()
                    .child(
                        Button::new("hosts-add-profile")
                            .small()
                            .primary()
                            .icon(IconName::Plus)
                            .child(rust_i18n::t!("hosts.add_profile").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_profile(window, cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .id("hosts-profiles-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(self.store.profiles.iter().map(|p| {
                        let id = p.id.clone();
                        let id2 = p.id.clone();
                        let id3 = p.id.clone();
                        let is_active = self.store.active_profile.as_deref() == Some(&p.id);
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
                                Checkbox::new(format!("profile-enabled-{}", p.id))
                                    .checked(p.enabled)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_profile_enabled(&id, cx);
                                    })),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(px(13.))
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(move |this, _, window, cx| {
                                            this.select_profile(&id2, window, cx);
                                        }),
                                    )
                                    .child(p.name.clone()),
                            )
                            .child(
                                Button::new(format!("profile-del-{}", p.id))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Delete)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.delete_profile(&id3, window, cx);
                                    })),
                            )
                    })),
            )
            .into_any_element()
    }

    fn render_editor(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();

        let active_profile = self
            .store
            .profiles
            .iter()
            .find(|p| Some(&p.id) == self.store.active_profile.as_ref());

        let profiles_section = if let Some(profile) = active_profile {
            self.render_profile_editor(cx, profile)
        } else {
            div()
                .w_full()
                .flex_1()
                .min_h_0()
                .flex()
                .items_center()
                .justify_center()
                .text_color(theme.muted_foreground)
                .child(rust_i18n::t!("hosts.no_entries").to_string())
                .into_any_element()
        };

        v_flex()
            .w_full()
            .h_full()
            .min_w_0()
            .child(self.render_system_hosts(cx))
            .child(profiles_section)
            .into_any_element()
    }

    fn render_profile_editor(
        &self,
        cx: &Context<Self>,
        p: &hosts_profiles::HostsProfile,
    ) -> AnyElement {
        let theme = cx.theme();
        let search_val = self
            .search_input
            .read(cx)
            .value()
            .to_string()
            .to_lowercase();

        let filtered_inputs: Vec<&EntryInput> = self
            .entry_inputs
            .iter()
            .filter(|ei| {
                let ip_val = ei.ip.read(cx).value().to_string().to_lowercase();
                let host_val = ei.host.read(cx).value().to_string().to_lowercase();
                let comment_val = ei.comment.read(cx).value().to_string().to_lowercase();
                search_val.is_empty()
                    || ip_val.contains(&search_val)
                    || host_val.contains(&search_val)
                    || comment_val.contains(&search_val)
            })
            .collect();

        let enabled_map: std::collections::HashMap<String, bool> = p
            .entries
            .iter()
            .map(|e| (e.id.clone(), e.enabled))
            .collect();

        let apply_virtual = p.apply_virtual;
        let apply_system = p.apply_to_system;

        let mut status_child: AnyElement = div().into_any_element();
        match &self.apply_status {
            ApplyStatus::Success => {
                status_child = div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .bg(theme.success.opacity(0.1))
                    .text_color(theme.success)
                    .text_size(px(12.))
                    .child(rust_i18n::t!("hosts.system_write_success").to_string())
                    .into_any_element();
            }
            ApplyStatus::Error(e) => {
                status_child = div()
                    .w_full()
                    .p_2()
                    .rounded_md()
                    .bg(theme.danger.opacity(0.1))
                    .text_color(theme.danger)
                    .text_size(px(12.))
                    .child(
                        rust_i18n::t!("hosts.system_write_failed", error = e.as_str()).to_string(),
                    )
                    .into_any_element();
            }
            _ => {}
        }

        v_flex()
            .w_full()
            .h_full()
            .flex_1()
            .min_w_0()
            .gap(px(8.))
            .child(
                v_flex()
                    .w_full()
                    .p_3()
                    .gap(px(8.))
                    .border_b_1()
                    .border_color(theme.border)
                    .child(
                        h_flex()
                            .w_full()
                            .gap_2()
                            .items_center()
                            .child(
                                div()
                                    .text_size(px(15.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .flex_1()
                                    .child(p.name.clone()),
                            )
                            .child(
                                Checkbox::new("toggle-virtual")
                                    .checked(apply_virtual)
                                    .label(rust_i18n::t!("hosts.apply_virtual").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_virtual(window, cx);
                                    })),
                            )
                            .child(
                                Checkbox::new("toggle-system")
                                    .checked(apply_system)
                                    .label(rust_i18n::t!("hosts.apply_system").to_string())
                                    .on_click(cx.listener(|this, _, window, cx| {
                                        this.toggle_system(window, cx);
                                    })),
                            ),
                    )
                    .child(Input::new(&self.search_input).w_full().small()),
            )
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .gap(px(8.))
                    .child(div().w(px(24.)).flex_shrink_0())
                    .child(
                        div()
                            .w(px(140.))
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(rust_i18n::t!("hosts.ip_ph").to_string()),
                    )
                    .child(
                        div()
                            .flex_1()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(rust_i18n::t!("hosts.host_ph").to_string()),
                    )
                    .child(
                        div()
                            .w(px(200.))
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(rust_i18n::t!("hosts.comment_ph").to_string()),
                    )
                    .child(div().w(px(28.)).flex_shrink_0()),
            )
            .child(
                v_flex()
                    .id("hosts-entries-scroll")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(filtered_inputs.iter().map(|ei| {
                        let entry_id = ei.entry_id.clone();
                        let entry_id_toggle = entry_id.clone();
                        let entry_id_remove = entry_id.clone();
                        let enabled = enabled_map.get(&ei.entry_id).copied().unwrap_or(true);
                        h_flex()
                            .w_full()
                            .px_3()
                            .py(px(6.))
                            .gap(px(8.))
                            .items_center()
                            .border_b_1()
                            .border_color(theme.border)
                            .hover(|d| d.bg(theme.muted.opacity(0.3)))
                            .child(
                                Checkbox::new(format!("entry-enabled-{}", ei.entry_id))
                                    .checked(enabled)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.toggle_entry_enabled(&entry_id_toggle, cx);
                                    })),
                            )
                            .child(Input::new(&ei.ip).w(px(140.)).small())
                            .child(Input::new(&ei.host).w_full().flex_1().small())
                            .child(Input::new(&ei.comment).w(px(200.)).small())
                            .child(
                                Button::new(format!("entry-remove-{}", ei.entry_id))
                                    .ghost()
                                    .small()
                                    .icon(IconName::Close)
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.remove_entry(&entry_id_remove, window, cx);
                                    })),
                            )
                    }))
                    .child(
                        Button::new("hosts-add-entry")
                            .w_full()
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .child(rust_i18n::t!("hosts.add_entry").to_string())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_entry(window, cx);
                            })),
                    ),
            )
            .child(
                v_flex()
                    .w_full()
                    .p_3()
                    .gap(px(8.))
                    .border_t_1()
                    .border_color(theme.border)
                    .child(status_child)
                    .child({
                        let is_appling = matches!(self.apply_status, ApplyStatus::Appling);
                        let btn = Button::new("hosts-apply-system")
                            .w_full()
                            .child(match &self.apply_status {
                                ApplyStatus::Appling => {
                                    rust_i18n::t!("hosts.need_admin").to_string() + "…"
                                }
                                _ => rust_i18n::t!("hosts.apply_system").to_string(),
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.apply_to_system(window, cx);
                            }));
                        if is_appling { btn.disabled(true) } else { btn }
                    }),
            )
            .into_any_element()
    }

    /// The system hosts preview shown pinned at the top of the right pane, always visible.
    fn render_system_hosts(&self, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let path = hosts::hosts_path();
        let path_display = path.display().to_string();

        v_flex()
            .w_full()
            .border_b_1()
            .border_color(theme.border)
            .child(
                h_flex()
                    .w_full()
                    .px_3()
                    .py_2()
                    .gap_2()
                    .items_center()
                    .child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .flex_1()
                            .child(format!("系统 Hosts · {}", path_display)),
                    )
                    .child(
                        Button::new("hosts-system-edit-ext")
                            .ghost()
                            .xsmall()
                            .icon(IconName::ExternalLink)
                            .tooltip("用外部编辑器打开 (需 sudo)")
                            .on_click(cx.listener(|this, _, _, _cx| {
                                this.open_system_in_editor();
                            })),
                    )
                    .child(
                        Button::new("hosts-system-refresh")
                            .ghost()
                            .xsmall()
                            .icon(IconName::Loader)
                            .tooltip("刷新")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh_system_hosts();
                                cx.notify();
                            })),
                    ),
            )
            .when_some(self.system_error.as_ref(), |c, err| {
                c.child(
                    div()
                        .px_3()
                        .py_1()
                        .text_size(px(12.))
                        .text_color(theme.danger)
                        .child(err.clone()),
                )
            })
            .child({
                let mut rows = v_flex()
                    .id("hosts-system-preview-scroll")
                    .w_full()
                    .max_h(px(200.))
                    .overflow_y_scroll()
                    .px_3()
                    .pb_2();
                for e in &self.system_entries {
                    rows = rows.child(
                        h_flex()
                            .w_full()
                            .gap_3()
                            .py(px(1.))
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .w(px(130.))
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground)
                                    .child(e.ip.clone()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .flex_1()
                                    .font_family(theme.mono_font_family.clone())
                                    .text_color(theme.muted_foreground)
                                    .child(e.host.clone()),
                            )
                            .when(e.comment.is_some(), |c| {
                                c.child(
                                    div()
                                        .text_size(px(10.))
                                        .text_color(theme.muted_foreground)
                                        .child(e.comment.clone().unwrap_or_default()),
                                )
                            }),
                    );
                }
                rows
            })
            .into_any_element()
    }
}

impl Render for HostsPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();

        // Resizable left sidebar: drag handle on the right edge.
        let saved = crate::state::persistence::load_layout()
            .and_then(|l| l.side_widths)
            .and_then(|w| w.get(3).copied())
            .filter(|&w| w > 0.0)
            .map(gpui::px)
            .unwrap_or_else(|| gpui::px(220.));
        let state = self.sidebar_resize.clone();

        gpui_component::resizable::h_resizable("hosts-sidebar")
            .with_state(&state)
            .on_resize(|state, _, cx| {
                if let Some(w) = state.read(cx).sizes().first() {
                    let mut layout = crate::state::persistence::load_layout().unwrap_or_default();
                    let mut arr = layout.side_widths.unwrap_or([260., 260., 260., 220.]);
                    arr[3] = w.as_f32();
                    layout.side_widths = Some(arr);
                    let _ = crate::state::persistence::save_layout(&layout);
                }
            })
            .child(
                gpui_component::resizable::resizable_panel()
                    .size(saved)
                    .size_range(gpui::px(160.)..gpui::px(420.))
                    .overflow_hidden()
                    .child(self.render_profile_list(cx)),
            )
            .child(
                gpui_component::resizable::resizable_panel()
                    .overflow_hidden()
                    .bg(theme.background)
                    .child(self.render_editor(cx)),
            )
    }
}
