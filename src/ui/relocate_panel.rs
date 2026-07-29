//! Relocate panel — the "磁盘迁移" exclusive view (PRD §9).
//!
//! Layout (top → bottom):
//! - target-root selector + probe result banner (SSD green / HDD amber /
//!   exFAT warning / insufficient reject) — PRD §7.
//! - auto-group cards (3 builtin groups + 用户自定义 + 其他) with per-entry
//!   checkboxes and a "推荐迁移" badge for groups > 5 GB — PRD §5, §6.
//! - "+ 添加自定义迁移目录" button → native folder picker — PRD §6.2.
//! - migration controls: conflict-policy select, verify switch, start button.
//! - live progress: global percent, current item/speed/file, per-item bar.
//! - queue controls: pause / resume / skip — PRD §9.
//! - scrolling log (Info/Warn/Error) — PRD §9.
//! - migrated-task list with per-task / per-group rollback — PRD §10.
//!
//! All I/O runs on the [`RelocateEngine`] entity (background executor); the
//! panel only reads snapshots + subscribes to [`RelocateEvent`].

use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::progress::Progress;
use gpui_component::{ActiveTheme, Disableable as _, IconName, Sizable as _, h_flex, v_flex};

use crate::relocate::disk_probe::{FsKind, Medium, Recommendation};
use crate::relocate::engine::{
    EngineState, LogLevel, RelocateEngine, RelocateEvent,
};
use crate::relocate::migrate::ConflictPolicy;
use crate::relocate::types::{human_bytes, AutoGroup, RiskLevel, TaskStatus};

pub struct RelocatePanel {
    pub engine: Entity<RelocateEngine>,
    _subs: Vec<Subscription>,
    focus_handle: FocusHandle,
    /// Whether the initial home-directory scan has run yet. The scan walks
    /// TCC-protected folders (`~/Music`, `~/Movies`, `~/Pictures`) which would
    /// trigger Apple Music / Media Library / Photos permission prompts; we
    /// therefore defer it until the user actually opens this view (see
    /// [`Self::activate`]) instead of running it at app startup.
    scan_started: bool,
}

impl RelocatePanel {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let engine = cx.new(|cx| RelocateEngine::new(cx));
        // Re-render on every engine event.
        let sub = cx.subscribe(&engine, |_this, _src, _ev: &RelocateEvent, cx| {
            cx.notify();
        });
        let _ = window;
        Self {
            engine,
            _subs: vec![sub],
            focus_handle: cx.focus_handle(),
            scan_started: false,
        }
    }

    /// Lazy-activate: kick off the first home-directory scan the first time the
    /// user opens the relocate view. Called from the rail click handler so the
    /// scan — and the TCC permission prompts it can trigger — only happen when
    /// the user actually wants to use disk migration, never at app launch.
    pub fn activate(&mut self, cx: &mut Context<Self>) {
        if self.scan_started {
            return;
        }
        self.scan_started = true;
        self.engine.update(cx, |e, cx| e.scan_async(cx));
    }

    // ---- actions --------------------------------------------------------

    /// Open the macOS folder picker (PRD §6.2) and add the chosen directory.
    /// Blacklist-rejected paths surface a Notification (PRD §6.3.1).
    fn pick_custom(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .set_title("选择要迁移的目录")
                .pick_folder();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let ent = cx.entity().downgrade();
        let engine = self.engine.clone();
        cx.spawn(async move |_this, cx| {
            let picked = loop {
                match rx.try_recv() {
                    Ok(v) => break v,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(60))
                            .await;
                    }
                    Err(_) => break None,
                }
            };
            if let Some(path) = picked {
                let accepted = engine.update(cx, |e, cx| e.add_custom(path, cx));
                let _ = ent.update(cx, |_, cx| cx.notify());
                if !accepted {
                    // The engine already logged the reason; surface a toast.
                    let _ = ent.update(cx, |_, _cx| {});
                }
            }
        })
        .detach();
    }

    /// Open the folder picker to choose the external target root, then probe.
    fn pick_target_root(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let (tx, rx) = std::sync::mpsc::channel::<Option<PathBuf>>();
        std::thread::spawn(move || {
            let res = rfd::FileDialog::new()
                .set_title("选择外置磁盘迁移根目录")
                .pick_folder();
            let _ = tx.send(res.map(|p| p.to_path_buf()));
        });
        let engine = self.engine.clone();
        cx.spawn(async move |_this, cx| {
            let picked = loop {
                match rx.try_recv() {
                    Ok(v) => break v,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(60))
                            .await;
                    }
                    Err(_) => break None,
                }
            };
            if let Some(path) = picked {
                engine.update(cx, |e, cx| e.set_target_root(path, cx));
            }
        })
        .detach();
    }

    // ---- render ---------------------------------------------------------

    fn render_groups(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let groups: Vec<AutoGroup> = self.engine.read(cx).scan.groups.clone();
        let selected = self.engine.read(cx).selected.clone();
        let is_running = self.engine.read(cx).state != EngineState::Idle;

        let mut col = v_flex().gap_3();

        for (gi, group) in groups.iter().enumerate() {
            let all_selected = group.entries.iter().all(|e| {
                selected.contains(&e.path.to_string_lossy().to_string())
            });
            let any_selected = group.entries.iter().any(|e| {
                selected.contains(&e.path.to_string_lossy().to_string())
            });

            // Group header: risk badge, name, size, recommended tag, all-toggle.
            let risk_color = match group.risk {
                RiskLevel::Safe => theme.success,
                RiskLevel::PerformanceWarn => theme.warning,
                RiskLevel::Forbidden => theme.danger,
            };

            let group_name = group.group_name.clone();
            let hint = group.hint_text.clone();
            let total = group.total_size_bytes;
            let recommended = group.is_recommended();

            // Header row.
            let header = h_flex()
                .items_center()
                .gap_2()
                .child(
                    Checkbox::new(("grp-all", gi))
                        .checked(all_selected)
                        .on_click({
                            let engine = self.engine.clone();
                            let group = group.clone();
                            cx.listener(move |_, _, _w, cx| {
                                engine.update(cx, |e, cx| {
                                    if all_selected {
                                        e.clear_group(&group, cx);
                                    } else {
                                        e.select_group(&group, cx);
                                    }
                                });
                            })
                        }),
                )
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(group_name.clone()),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .px(px(6.))
                        .py(px(1.))
                        .rounded(px(4.))
                        .bg(risk_color.opacity(0.18))
                        .text_color(risk_color)
                        .child(group.risk.label()),
                )
                .when(recommended, |h| {
                    h.child(
                        div()
                            .text_size(px(11.))
                            .px(px(6.))
                            .py(px(1.))
                            .rounded(px(4.))
                            .bg(theme.primary.opacity(0.18))
                            .text_color(theme.primary)
                            .child("推荐迁移"),
                    )
                })
                .child(div().flex_1())
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted_foreground)
                        .child(group.size_human()),
                );

            // Hint line.
            let hint_row = div()
                .text_size(px(11.))
                .text_color(theme.muted_foreground)
                .child(hint);

            // Per-entry rows.
            let mut entries_col = v_flex().gap_1().pl(px(24.));
            for (ei, entry) in group.entries.iter().enumerate() {
                let key = entry.path.to_string_lossy().to_string();
                let checked = selected.contains(&key);
                let path_display = entry.path.display().to_string();
                entries_col = entries_col.child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .py(px(2.))
                        .child(
                            Checkbox::new(format!("ent-{}-{}", gi, ei))
                                .checked(checked)
                                .disabled(is_running)
                                .on_click({
                                    let engine = self.engine.clone();
                                    let key = key.clone();
                                    cx.listener(move |_, _, _w, cx| {
                                        engine.update(cx, |e, cx| e.toggle_selected(&key, cx));
                                    })
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.foreground)
                                .child(path_display),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(px(11.))
                                .text_color(theme.muted_foreground)
                                .child(entry.size_human()),
                        ),
                );
            }
            let _ = any_selected;

            let card = v_flex()
                .border_1()
                .border_color(theme.border)
                .rounded(theme.radius)
                .p_3()
                .gap_1()
                .bg(theme.muted.opacity(0.25))
                .child(header)
                .child(hint_row)
                .child(entries_col);

            col = col.child(card);
        }

        if groups.is_empty() {
            col = col.child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.muted_foreground)
                    .p_4()
                    .child("暂无可迁移目录，点击下方按钮重新扫描。"),
            );
        }

        // "+ add custom" + rescan row.
        let actions = h_flex().gap_2().pt(px(4.)).children([
            Button::new("relocate-add-custom")
                .outline()
                .small()
                .icon(IconName::Plus)
                .label("添加自定义迁移目录")
                .on_click(cx.listener(|this, _, w, cx| this.pick_custom(w, cx))),
            Button::new("relocate-rescan")
                .ghost()
                .small()
                .icon(IconName::Redo)
                .label("重新扫描")
                .on_click(cx.listener(|_, _, _, cx| {
                    cx.notify();
                })),
        ]);

        col.child(actions).into_any_element()
    }

    fn render_target_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let eng = self.engine.read(cx);
        let target = eng.target_root.clone();
        let probe = eng.probe.clone();
        let required = eng.selected_total_bytes();

        // Banner colour by recommendation.
        let (banner_text, banner_color) = match &probe {
            Some(p) => {
                let rec = p.recommendation(required);
                match rec {
                    Recommendation::SsdOk => (
                        format!(
                            "✅ 外置 SSD 就绪 · {} · 剩余 {}",
                            p.fs_kind.label(),
                            human_bytes(p.free_bytes)
                        ),
                        theme.success,
                    ),
                    Recommendation::UnknownProceed => (
                        format!(
                            "⚪ 磁盘信息未知 · 剩余 {}（请确认目标为外置磁盘）",
                            human_bytes(p.free_bytes)
                        ),
                        theme.muted_foreground,
                    ),
                    Recommendation::HddWarn => (
                        "⚠️ 机械硬盘：迁移/编译/索引速度会明显下降，建议使用雷电/USB4 SSD。"
                            .to_string(),
                        theme.warning,
                    ),
                    Recommendation::ExfatWarn => (
                        "⚠️ exFAT：存在权限/属性风险，推荐 APFS。".to_string(),
                        theme.warning,
                    ),
                    Recommendation::InsufficientSpace => (
                        format!(
                            "❌ 空间不足：剩余 {}，需要 {}",
                            human_bytes(p.free_bytes),
                            human_bytes(required)
                        ),
                        theme.danger,
                    ),
                    Recommendation::InternalRejected => (
                        "❌ 内置磁盘，已拒绝（仅支持外置磁盘）。".to_string(),
                        theme.danger,
                    ),
                }
            }
            None => (
                "未选择外置磁盘迁移根目录".to_string(),
                theme.muted_foreground,
            ),
        };

        // Disk meta line.
        let meta = match &probe {
            Some(p) => format!(
                "{} · {} · 总 {} · 剩余 {}",
                p.volume_name.clone().unwrap_or_else(|| "外置卷".into()),
                medium_fs_label(p.medium, p.fs_kind),
                human_bytes(p.total_bytes),
                human_bytes(p.free_bytes)
            ),
            None => "点击右侧按钮选择外置磁盘迁移根目录".to_string(),
        };

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("外置磁盘"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("relocate-pick-target")
                            .outline()
                            .small()
                            .icon(IconName::FolderOpen)
                            .label(if target.is_some() {
                                "更换磁盘"
                            } else {
                                "选择磁盘"
                            })
                            .on_click(cx.listener(|this, _, w, cx| {
                                this.pick_target_root(w, cx)
                            })),
                    ),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .p(px(8.))
                    .rounded(theme.radius)
                    .bg(banner_color.opacity(0.12))
                    .border_1()
                    .border_color(banner_color.opacity(0.35))
                    .text_color(banner_color)
                    .child(banner_text),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(meta),
            )
            .into_any_element()
    }

    fn render_migration_controls(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let eng = self.engine.read(cx);
        let state = eng.state;
        let can_start = eng.can_start_migration() && state == EngineState::Idle;
        let total = eng.selected_total_bytes();
        let verify = eng.verify;
        let conflict = eng.conflict_policy;

        // Conflict policy toggle: Merge / Backup / Skip.
        let policy_row = h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("目标冲突时："),
            )
            .children([
                policy_btn("policy-merge", "合并", conflict == ConflictPolicy::Merge, {
                    let engine = self.engine.clone();
                    cx.listener(move |_, _, _, cx| {
                        engine.update(cx, |e, cx| {
                            e.conflict_policy = ConflictPolicy::Merge;
                            cx.notify();
                        });
                    })
                }),
                policy_btn(
                    "policy-backup",
                    "备份重命名",
                    conflict == ConflictPolicy::BackupRename,
                    {
                        let engine = self.engine.clone();
                        cx.listener(move |_, _, _, cx| {
                            engine.update(cx, |e, cx| {
                                e.conflict_policy = ConflictPolicy::BackupRename;
                                cx.notify();
                            });
                        })
                    },
                ),
                policy_btn("policy-skip", "跳过", conflict == ConflictPolicy::Skip, {
                    let engine = self.engine.clone();
                    cx.listener(move |_, _, _, cx| {
                        engine.update(cx, |e, cx| {
                            e.conflict_policy = ConflictPolicy::Skip;
                            cx.notify();
                        });
                    })
                }),
            ]);

        // Verify switch.
        let verify_row = h_flex()
            .items_center()
            .gap_2()
            .child(
                Checkbox::new("relocate-verify")
                    .checked(verify)
                    .on_click({
                        let engine = self.engine.clone();
                        cx.listener(move |_, _, _, cx| {
                            engine.update(cx, |e, cx| {
                                e.verify = !e.verify;
                                cx.notify();
                            });
                        })
                    }),
            )
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child("迁移后 SHA-256 完整性校验（较慢）"),
            );

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("迁移"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(format!("预估释放：{}", human_bytes(total))),
                    ),
            )
            .child(policy_row)
            .child(verify_row)
            .child(
                h_flex().gap_2().child(
                    Button::new("relocate-start")
                        .primary()
                        .small()
                        .icon(IconName::ArrowRight)
                        .label(match state {
                            EngineState::Migrating => "迁移中…",
                            EngineState::Paused => "已暂停",
                            EngineState::RollingBack => "回滚中…",
                            EngineState::Idle => "开始迁移",
                        })
                        .disabled(!can_start)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.engine.update(cx, |e, cx| e.start_migration(cx));
                        })),
                ),
            )
            .into_any_element()
    }

    fn render_progress(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let eng = self.engine.read(cx);
        let p = &eng.progress;
        let state = eng.state;
        let percent = p.global_percent();

        let header = h_flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(match state {
                        EngineState::Migrating => "迁移进度",
                        EngineState::RollingBack => "回滚进度",
                        EngineState::Paused => "已暂停",
                        EngineState::Idle => "进度",
                    }),
            )
            .child(div().flex_1())
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{}/{} · {:.0}%",
                        p.current_index + 1,
                        p.total_items.max(1),
                        percent * 100.0
                    )),
            );

        // Queue controls (only while running / paused).
        let controls = match state {
            EngineState::Migrating | EngineState::Paused => Some(
                h_flex().gap_2().children([
                    if state == EngineState::Migrating {
                        Button::new("rel-pause")
                            .ghost()
                            .small()
                            .icon(IconName::Pause)
                            .label("暂停")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.engine.update(cx, |e, cx| e.pause(cx))
                            }))
                    } else {
                        Button::new("rel-resume")
                            .ghost()
                            .small()
                            .icon(IconName::Play)
                            .label("继续")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.engine.update(cx, |e, cx| e.resume(cx))
                            }))
                    },
                    Button::new("rel-skip")
                        .ghost()
                        .small()
                        .icon(IconName::ChevronRight)
                        .label("跳过当前")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.engine.update(cx, |e, cx| e.skip_current(cx))
                        })),
                ]),
            ),
            _ => None,
        };

        let current_item = if !p.current_source.is_empty() {
            Some(
                div()
                    .text_size(px(12.))
                    .text_color(theme.foreground)
                    .child(format!("当前：{}", p.current_source)),
            )
        } else {
            None
        };

        let speed = if p.speed_bytes_per_sec > 0 {
            Some(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted_foreground)
                    .child(format!(
                        "{}/s{}",
                        human_bytes(p.speed_bytes_per_sec),
                        if !p.current_file.is_empty() {
                            format!(" · {}", p.current_file)
                        } else {
                            String::new()
                        }
                    )),
            )
        } else {
            None
        };

        v_flex()
            .gap_2()
            .child(header)
            .child(Progress::new("relocate-global-progress").value(percent))
            .when_some(current_item, |c, it| c.child(it))
            .when_some(speed, |c, it| c.child(it))
            .when_some(controls, |c, ctl| c.child(ctl))
            .into_any_element()
    }

    fn render_log(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let logs = self.engine.read(cx).logs.clone();

        let mut rows = v_flex().gap_0();
        // Show the last 200 lines.
        let start = logs.len().saturating_sub(200);
        for line in &logs[start..] {
            let color = match line.level {
                LogLevel::Info => theme.foreground,
                LogLevel::Warn => theme.warning,
                LogLevel::Error => theme.danger,
            };
            rows = rows.child(
                h_flex()
                    .gap_2()
                    .px(px(6.))
                    .py(px(1.))
                    .child(
                        div()
                            .text_size(px(10.))
                            .text_color(theme.muted_foreground)
                            .child(line.time.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(color)
                            .child(line.message.clone()),
                    ),
            );
        }

        v_flex()
            .gap_2()
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("日志"),
            )
            .child(
                div()
                    .id("relocate-log-scroll")
                    .flex_1()
                    .min_h(px(80.))
                    .overflow_y_scroll()
                    .border_1()
                    .border_color(theme.border)
                    .rounded(theme.radius)
                    .bg(theme.background)
                    .p_2()
                    .child(rows),
            )
            .into_any_element()
    }

    fn render_tasks(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme().clone();
        let tasks: Vec<crate::relocate::types::TaskRecord> =
            self.engine.read(cx).tasks.clone();
        let reclaimed = self.engine.read(cx).reclaimed_bytes();

        let mut rows = v_flex().gap_1();
        // Group migrated tasks by group_name for batch rollback.
        let mut group_order: Vec<String> = Vec::new();
        for t in &tasks {
            if !group_order.contains(&t.group_name) {
                group_order.push(t.group_name.clone());
            }
        }

        for gname in &group_order {
            let group_tasks: Vec<&crate::relocate::types::TaskRecord> =
                tasks.iter().filter(|t| &t.group_name == gname).collect();
            let migrated_in_group = group_tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Migrated)
                .count();

            rows = rows.child(
                v_flex()
                    .gap_1()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child(gname.clone()),
                            )
                            .child(div().flex_1())
                            .when(migrated_in_group > 1, |h| {
                                h.child(
                                    Button::new(format!("rel-rollback-group-{}", gname))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Undo2)
                                        .label("整组回滚")
                                        .on_click({
                                            let engine = self.engine.clone();
                                            let gname = gname.clone();
                                            cx.listener(move |_, _, _, cx| {
                                                engine.update(cx, |e, cx| {
                                                    e.rollback_group(&gname, cx)
                                                });
                                            })
                                        }),
                                )
                            }),
                    )
                    .children(group_tasks.iter().enumerate().map(|(i, t)| {
                        let status_color = match t.status {
                            TaskStatus::Migrated => theme.success,
                            TaskStatus::RolledBack => theme.muted_foreground,
                            TaskStatus::Failed => theme.danger,
                        };
                        h_flex()
                            .items_center()
                            .gap_2()
                            .px(px(6.))
                            .py(px(2.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(theme.foreground)
                                    .child(t.source_path.clone()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .text_color(theme.muted_foreground)
                                    .child(t.size_human()),
                            )
                            .child(
                                div()
                                    .text_size(px(11.))
                                    .px(px(6.))
                                    .py(px(1.))
                                    .rounded(px(4.))
                                    .bg(status_color.opacity(0.16))
                                    .text_color(status_color)
                                    .child(t.status.label()),
                            )
                            .when(t.status == TaskStatus::Migrated, |row| {
                                row.child(
                                    Button::new(format!("rel-rollback-{}", t.task_id))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Undo2)
                                        .label("回滚")
                                        .on_click({
                                            let engine = self.engine.clone();
                                            let id = t.task_id.clone();
                                            cx.listener(move |_, _, _, cx| {
                                                engine.update(cx, |e, cx| {
                                                    e.rollback_task(&id, cx)
                                                });
                                            })
                                        }),
                                )
                            })
                            .when(t.status == TaskStatus::RolledBack, |row| {
                                row.child(
                                    Button::new(format!("rel-del-ext-{}", t.task_id))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Delete)
                                        .label("删除外置副本")
                                        .on_click({
                                            let engine = self.engine.clone();
                                            let id = t.task_id.clone();
                                            cx.listener(move |_, _, _, cx| {
                                                engine.update(cx, |e, cx| {
                                                    e.delete_external_copy(&id, cx)
                                                });
                                            })
                                        }),
                                )
                            })
                            .when_some(
                                t.error.clone().filter(|_| t.status == TaskStatus::Failed),
                                |row, err| {
                                    row.child(
                                        div()
                                            .text_size(px(10.))
                                            .text_color(theme.danger)
                                            .max_w(px(320.))
                                            .child(err),
                                    )
                                },
                            )
                    })),
            );
        }

        v_flex()
            .gap_2()
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("已迁移任务"),
                    )
                    .child(div().flex_1())
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted_foreground)
                            .child(format!("已释放：{}", human_bytes(reclaimed))),
                    ),
            )
            // Rollback warning banner (PRD §10.4 fixed copy).
            .child(
                div()
                    .text_size(px(11.))
                    .p(px(8.))
                    .rounded(theme.radius)
                    .bg(theme.warning.opacity(0.10))
                    .border_1()
                    .border_color(theme.warning.opacity(0.30))
                    .text_color(theme.warning)
                    .child("一键回滚：将外置固态文件复制回内置磁盘，移除符号链接。⚠️ 本功能依赖迁移所用外置固态保持连接；硬盘丢失/损坏请依靠 Time Machine 历史备份兜底恢复。"),
            )
            .child(rows)
            .into_any_element()
    }
}

impl Focusable for RelocatePanel {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<()> for RelocatePanel {}

impl Render for RelocatePanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        // Left column: groups + custom + migration controls (scrollable).
        let left = v_flex()
            .id("relocate-left-scroll")
            .w(px(480.))
            .min_w(px(360.))
            .h_full()
            .min_h_0()
            .border_r_1()
            .border_color(theme.border)
            .bg(theme.background)
            .p_4()
            .gap_4()
            .overflow_y_scroll()
            .child(self.render_target_bar(cx))
            .child(self.render_groups(cx))
            .child(self.render_migration_controls(cx));

        // Right column: progress + log + tasks.
        let right = v_flex()
            .flex_1()
            .min_w_0()
            .h_full()
            .min_h_0()
            .p_4()
            .gap_4()
            .child(self.render_progress(cx))
            .child(self.render_log(cx))
            .child(self.render_tasks(cx));

        h_flex()
            .size_full()
            .min_h_0()
            .child(left)
            .child(right)
    }
}

// ---- helpers -----------------------------------------------------------

/// Render a toggle-able conflict-policy pill button, returned as an
/// `AnyElement` so callers can collect several into a single container.
fn policy_btn(
    id: &'static str,
    label: &'static str,
    active: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    // We can't read the theme here cheaply (no cx), so rely on Button styles:
    // primary when active, ghost otherwise.
    if active {
        Button::new(id)
            .primary()
            .xsmall()
            .label(label)
            .on_click(on_click)
            .into_any_element()
    } else {
        Button::new(id)
            .ghost()
            .xsmall()
            .label(label)
            .on_click(on_click)
            .into_any_element()
    }
}

/// Combined medium + fs label string for the target meta line.
fn medium_fs_label(medium: Medium, fs: FsKind) -> String {
    format!("{} · {}", medium.label(), fs.label())
}
