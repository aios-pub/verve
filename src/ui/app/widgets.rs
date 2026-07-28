//! Shared UI helper functions for the app shell: menu rows, dropdown
//! panels, keycaps, dividers, the share-result dialog, and resizable-
//! size persistence. These are used across several title-bar and
//! toolbar rendering methods.

use gpui::{img, *};
use gpui::prelude::FluentBuilder as _;
use gpui_component::{ActiveTheme, Icon, IconName, Sizable as _, WindowExt as _, button::{Button, ButtonVariants as _}, h_flex, v_flex};
use crate::assets::{EXPORT, SAVE, SAVE_AS};
use crate::share::models::ShareConfig;
use crate::state::persistence;
use gpui_component::resizable::ResizableState;
use super::SideView;
use super::{SaveWorkspace, NewRequest, OpenMarkdownFile, OpenMarkdownFileNewTab, NewFile, CloseFile};

/// Build an icon from a Verve-custom Lucide SVG path.
pub(super) fn vicon(path: &'static str) -> Icon {
    Icon::from(IconName::Redo).path(path)
}

/// Persist the main horizontal group sizes ([tree]).
pub(super) fn persist_main(state: &Entity<ResizableState>, cx: &mut gpui::App) {
    let sizes = state.read(cx).sizes().clone();
    if !sizes.is_empty() {
        let mut layout = crate::state::persistence::load_layout().unwrap_or_default();
        // Keep the array shape ([f32; 2]) for backward compat; only [0] (tree) is used.
        layout.main = Some([sizes[0].as_f32(), 0.0]);
        let _ = crate::state::persistence::save_layout(&layout);
    }
}

/// Persist the center vertical group sizes ([request, response, console?]).
pub(super) fn persist_center(state: &Entity<ResizableState>, cx: &mut gpui::App) {
    let sizes = state.read(cx).sizes().clone();
    if sizes.len() >= 2 {
        let mut layout = crate::state::persistence::load_layout().unwrap_or_default();
        let req = sizes[0].as_f32();
        let resp = sizes[1].as_f32();
        let con = sizes.get(2).map(|p| p.as_f32()).unwrap_or(200.0);
        layout.center = Some([req, resp, con]);
        let _ = crate::state::persistence::save_layout(&layout);
    }
}

/// Build a popover menu row (see original doc comment).
pub(super) fn menu_item(
    id: String,
    label: String,
    is_active: bool,
    icon: Option<IconName>,
    on_delete: Option<Box<dyn Fn(&mut gpui::App) + 'static>>,
    on_settings: Option<Box<dyn Fn(&mut gpui::App) + 'static>>,
    on_click: impl Fn(&gpui::ClickEvent, &mut gpui::App) + 'static,
    theme: &gpui_component::Theme,
) -> impl IntoElement {
    let theme = theme.clone();
    let has_action = on_settings.is_some() || on_delete.is_some();
    let group = format!("menu-row-{}", id);
    div()
        .id(id.clone())
        .w_full()
        .px(px(8.))
        .py(px(5.))
        .gap(px(6.))
        .flex()
        .items_center()
        .group(group.clone())
        .rounded(px(4.))
        .text_size(px(12.))
        .text_color(if is_active {
            theme.foreground
        } else {
            theme.muted_foreground
        })
        .when(is_active, |d| {
            d.bg(theme.primary.opacity(0.2))
                .font_weight(FontWeight::SEMIBOLD)
        })
        .hover(|d| d.bg(theme.accent.opacity(0.4)))
        .when_some(icon, |d, ic| {
            d.child(
                div()
                    .w(px(16.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(theme.muted_foreground)
                    .child(ic),
            )
        })
        // The label: nowrap + ellipsis so icon and text stay on one line.
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .text_ellipsis()
                .whitespace_nowrap()
                .child(label),
        )
        .when(is_active, |d| {
            // When active AND no action buttons, show the check mark.
            d.when(!has_action, |d| {
                d.child(div().text_color(theme.primary).child(IconName::Check))
            })
        })
        // The hover-revealed delete button (left of settings).
        .when_some(on_delete, |d, on_del| {
            let theme_g = theme.clone();
            let theme_hover = theme.clone();
            d.child(
                div()
                    .id(format!("{}-del", id))
                    .w(px(20.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .text_color(theme_g.muted_foreground)
                    .opacity(0.0)
                    .group_hover(group.clone(), move |b| {
                        b.opacity(1.0).text_color(theme_g.danger)
                    })
                    .hover(move |b| b.bg(theme_hover.danger.opacity(0.2)))
                    .child(IconName::Delete)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        move |_, window, cx: &mut gpui::App| {
                            cx.stop_propagation();
                            let _ = window;
                            (on_del)(cx);
                        },
                    ),
            )
        })
        // The hover-revealed settings button (rightmost).
        .when_some(on_settings, |d, on_set| {
            let theme_g = theme.clone();
            let theme_hover = theme.clone();
            d.child(
                div()
                    .id(format!("{}-set", id))
                    .w(px(20.))
                    .h(px(20.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(4.))
                    .text_color(theme_g.muted_foreground)
                    // Hidden by default; revealed on row hover.
                    .opacity(0.0)
                    .group_hover(group.clone(), move |b| {
                        b.opacity(1.0).text_color(theme_g.foreground)
                    })
                    .hover(move |b| b.bg(theme_hover.border))
                    .child(IconName::Settings)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        move |_, window, cx: &mut gpui::App| {
                            cx.stop_propagation();
                            let _ = window;
                            (on_set)(cx);
                        },
                    ),
            )
        })
        .on_click(move |ev, _window, cx: &mut gpui::App| {
            (on_click)(ev, cx);
        })
}


pub(super) fn menu_separator(theme: &gpui_component::Theme) -> impl IntoElement {
    div().w_full().h(px(1.)).my(px(2.)).bg(theme.border)
}


pub(super) fn command_keycap(
    command: crate::wysiwyg::components::actions::ShortcutCommand,
) -> Option<AnyElement> {
    use crate::wysiwyg::components::actions::resolved_shortcut_keys;
    use gpui_component::kbd::Kbd;
    let cfg = crate::state::persistence::load_shortcuts();
    resolved_shortcut_keys(&cfg, command)
        .first()
        .and_then(|k| Keystroke::parse(k).ok())
        .map(|ks| Kbd::new(ks).into_any_element())
}


pub(super) fn toolbar_divider(theme: &gpui_component::Theme) -> impl IntoElement {
    div().mx(px(4.)).w(px(1.)).h(px(16.)).bg(theme.border)
}


pub(super) fn dropdown_separator(theme: &gpui_component::Theme) -> AnyElement {
    div()
        .mx(px(6.))
        .my(px(3.))
        .h(px(1.))
        .bg(theme.border)
        .into_any_element()
}


pub(super) fn dropdown_panel(
    id: &'static str,
    items: Vec<AnyElement>,
    theme: &gpui_component::Theme,
) -> AnyElement {
    div()
        .id(id)
        .w(px(240.))
        .max_h(px(420.))
        .overflow_y_scroll()
        .p(px(4.))
        .flex()
        .flex_col()
        .gap(px(2.))
        .bg(theme.background)
        .border(px(1.))
        .border_color(theme.border)
        .rounded(px(6.))
        .shadow_lg()
        // Stop the outside-click dismiss when clicking inside the panel.
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .children(items)
        .into_any_element()
}

/// Show the "share created" result dialog with the given URL. Used by both

pub(super) fn show_share_result_dialog(
    window: &mut Window,
    cx: &mut App,
    cfg: crate::share::ShareConfig,
    url: String,
) {
    // Build a QR code if the user selected that method.
    let wants_qr = cfg
        .share_methods
        .iter()
        .any(|m| matches!(m, crate::share::models::ShareMethod::QrCode));
    let qr_data_url = if wants_qr {
        crate::share::qrcode::to_svg_data_url(&url)
    } else {
        None
    };

    // If the user chose "导出 HTML", write a copy to the exports folder.
    // (In cloud mode this still happens locally — it's an offline artifact.)
    let cfg_id_for_html = cfg.id.clone();
    let cfg_project_id = cfg.project_id.clone();
    if cfg
        .share_methods
        .iter()
        .any(|m| matches!(m, crate::share::models::ShareMethod::ExportHtml))
    {
        if let Ok(export_dir) = crate::state::persistence::data_dir().map(|d| d.join("exports")) {
            let _ = std::fs::create_dir_all(&export_dir);
            let project = crate::state::persistence::load_or_default()
                .projects
                .iter()
                .find(|p| p.id == cfg_project_id)
                .cloned();
            if let Some(project) = project {
                let html = crate::share::html::render_doc_html(&cfg, &project);
                let fname = format!("{}.html", cfg_id_for_html);
                let _ = std::fs::write(export_dir.join(&fname), html);
            }
        }
    }

    cx.open_url(&url);

    let title = cfg.display_title();
    let expire_label = cfg.expire.label().to_string();
    let access_label = cfg.access_label().to_string();
    let url_arc = std::sync::Arc::new(url);
    let qr_arc = std::sync::Arc::new(qr_data_url);

    window.open_dialog(cx, move |dialog, _window, cx| {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;
        let theme_muted = theme.muted;
        let qr = (*qr_arc).clone();
        let title_text = title.clone();
        let expire_label = expire_label.clone();
        let access_label = access_label.clone();
        let url_for_content = url_arc.clone();
        dialog
            .title("分享已创建")
            .content(move |content, _window, _cx| {
                let url_content = (*url_for_content).clone();
                let url_copy = (*url_for_content).clone();
                content.child(
                    v_flex()
                        .p_4()
                        .w(px(440.))
                        .gap_3()
                        .child(
                            div()
                                .text_sm()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(format!("文档「{}」分享成功", title_text)),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(muted)
                                .child("访问以下链接查看文档："),
                        )
                        .child(
                            h_flex()
                                .gap_2()
                                .child(
                                    div()
                                        .flex_1()
                                        .px_2()
                                        .py_1()
                                        .rounded_md()
                                        .bg(theme_muted)
                                        .text_xs()
                                        .child(url_content),
                                )
                                .child(
                                    Button::new("share-copy-link")
                                        .ghost()
                                        .small()
                                        .label("复制")
                                        .on_click(move |_, _, cx| {
                                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                                url_copy.clone(),
                                            ));
                                        }),
                                ),
                        )
                        .when_some(qr.as_ref(), |col, qr_url| {
                            col.child(
                                div()
                                    .mt_2()
                                    .items_center()
                                    .justify_center()
                                    .flex()
                                    .child(img(qr_url.clone()).w(px(200.)).h(px(200.))),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted)
                                    .text_center()
                                    .child("扫描二维码访问"),
                            )
                        })
                        .child(div().h(px(1.)).w_full().bg(border).text_color(muted))
                        .child(div().text_xs().text_color(muted).child(format!(
                            "有效期：{}  ·  访问限制：{}",
                            expire_label, access_label
                        ))),
                )
            })
            .footer(
                Button::new("share-result-close")
                    .primary()
                    .small()
                    .label("完成")
                    .on_click(|_, window, cx| {
                        window.close_dialog(cx);
                    }),
            )
    });
}
