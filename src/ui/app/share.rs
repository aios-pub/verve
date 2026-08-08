//! Document-sharing actions: open the share-config dialog (local or cloud
//! mode), build share URLs, and delete a share config.

use super::VerveApp;
use super::widgets::show_share_result_dialog;
use crate::share::models::ShareScope;
use crate::ui::share_dialog;
use gpui::prelude::FluentBuilder as _;
use gpui::{img, *};
use gpui_component::{
    ActiveTheme, Sizable as _, WindowExt as _,
    button::{Button, ButtonVariants as _},
    h_flex, v_flex,
};

impl VerveApp {
    pub(super) fn open_share_dialog(
        &mut self,
        scope: ShareScope,
        target_id: Option<String>,
        target_name: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let state = self.state.clone();
        let configs_store = self.share_configs.clone();
        let share_host = self.share_host.clone();
        let share_port = self.share_port;
        let active_ws = self.active_workspace_id_cache.clone();
        let remote_cfg = crate::state::persistence::load_remote_config();
        let is_cloud = crate::share::remote::is_cloud_mode(&remote_cfg);
        // The confirm handler needs `this` to start/refresh the server + panel,
        // but `this` isn't movable into `open_dialog`. We close over the pieces
        // we need (the config store) and do the panel refresh via cx.notify.
        share_dialog::open_dialog(
            state.clone(),
            scope,
            target_id,
            target_name,
            move |cfg, window, cx: &mut App| {
                // Apply tenant namespace when multi-tenant mode is enabled.
                let cfg = if remote_cfg.multi_tenant {
                    if let Some(ws) = active_ws.as_deref() {
                        cfg.with_tenant(ws)
                    } else {
                        cfg
                    }
                } else {
                    cfg
                };
                log::info!("创建分享: id={} title={}", cfg.id, cfg.display_title());

                if is_cloud {
                    // ---- Cloud mode: push to remote server over HTTP ----
                    // Resolve the project to upload.
                    let project_id_str = cfg.project_id.clone();
                    let project = state
                        .read(cx)
                        .data
                        .projects
                        .iter()
                        .find(|p| p.id == project_id_str)
                        .cloned();
                    let Some(project) = project else {
                        let not_found_id = project_id_str.clone();
                        window.open_dialog(cx, move |dialog, _window, _cx| {
                            let msg = format!("找不到项目：{}", not_found_id);
                            dialog.title("分享失败").content(move |c, _, _| {
                                c.child(div().p_4().text_sm().child(msg.clone()))
                            })
                        });
                        return;
                    };
                    let cfg_for_push = cfg.clone();
                    let cfg_for_err = cfg.clone();
                    let client = cx.http_client();
                    let remote_cfg_clone = remote_cfg.clone();
                    let window_handle = window.window_handle();
                    cx.spawn(async move |cx| {
                        let cfg_for_result = cfg_for_push.clone();
                        let result = crate::share::remote::push_share(
                            client,
                            &remote_cfg_clone,
                            &project,
                            &cfg_for_push,
                        )
                        .await;
                        let _ = window_handle.update(cx, |_this, window, cx| match result {
                            Ok(remote_id) => {
                                let url =
                                    crate::share::remote::share_url(&remote_cfg_clone, &remote_id)
                                        .unwrap_or_default();
                                show_share_result_dialog(window, cx, cfg_for_result, url);
                            }
                            Err(e) => {
                                let err_msg = format!("推送到远程服务失败：{e}");
                                log::error!("{}", err_msg);
                                window.open_dialog(cx, move |dialog, _window, cx| {
                                    let msg = err_msg.clone();
                                    dialog
                                        .title("分享失败")
                                        .content(move |c, _, _| {
                                            c.child(
                                                div()
                                                    .p_4()
                                                    .w(px(400.))
                                                    .text_sm()
                                                    .child(msg.clone()),
                                            )
                                        })
                                        .footer(
                                            Button::new("share-error-close")
                                                .primary()
                                                .small()
                                                .label("关闭")
                                                .on_click(|_, w, cx| w.close_dialog(cx)),
                                        )
                                });
                            }
                        });
                    })
                    .detach();
                    return;
                }

                // ---- Local mode: persist + serve from embedded server ----
                let all = crate::share::persist::upsert_share(cfg.clone());
                if let Ok(mut guard) = configs_store.write() {
                    *guard = all;
                }

                let url = format!("http://{share_host}:{share_port}/s/{}", cfg.id);
                show_share_result_dialog(window, cx, cfg, url);
            },
            window,
            cx,
        );
    }

    pub(super) fn build_share_url(&self, id: &str) -> String {
        let remote_cfg = crate::state::persistence::load_remote_config();
        if let Some(cloud_url) = crate::share::remote::share_url(&remote_cfg, id) {
            return cloud_url;
        }
        format!("http://{}:{}/s/{}", self.share_host, self.share_port, id)
    }

    pub(super) fn delete_share(&mut self, id: String, cx: &mut Context<Self>) {
        let all = crate::share::persist::remove_share(&id);
        log::info!("删除分享 {id}，剩余 {} 个", all.len());
        if let Ok(mut guard) = self.share_configs.write() {
            *guard = all;
        }
        // Refresh the share panel so the row disappears.
        self.share.update(cx, |s, cx| s.reload(cx));
        cx.notify();
    }
}
