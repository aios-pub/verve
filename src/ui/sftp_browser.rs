//! SFTP file browser panel — lists remote files, supports upload, download,
//! mkdir, rename, delete.
//!
//! Architecture mirrors the SSH terminal pump pattern in `ssh_panel.rs`:
//! 1. The GPUI entity owns view state (path, entries, selection, transfers).
//! 2. A background OS thread runs a tokio runtime with an SFTP actor that
//!    owns the `SftpSessionWrapper`. Commands arrive through an mpsc channel.
//! 3. The actor sends results back through another mpsc channel.
//! 4. A GPUI `cx.spawn` loop drains the result channel and updates the entity.

use std::path::PathBuf;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::input::{Input, InputState};
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Sizable as _, WindowExt as _,
};
use tokio::sync::mpsc;

use crate::ssh::sftp::{FileEntry, TransferProgress, humanize_size};

/// Commands from the UI to the SFTP background actor.
#[derive(Debug)]
pub enum SftpCmd {
    ListDir { path: String },
    Mkdir { parent: String, name: String },
    Rename { from: String, to: String },
    Remove { path: String, is_dir: bool },
    Upload { local: PathBuf, remote: String, id: String },
    Download { remote: String, local: PathBuf, id: String },
}

/// One in-progress or completed transfer.
#[derive(Clone)]
pub struct TransferState {
    id: String,
    filename: String,
    total: u64,
    transferred: u64,
    uploading: bool,
    done: bool,
    error: Option<String>,
}

/// The GPUI entity that renders the SFTP file browser.
pub struct SftpBrowser {
    pub host_name: String,
    pub current_path: String,
    pub entries: Vec<FileEntry>,
    pub loading: bool,
    pub error: Option<String>,
    pub selected_index: Option<usize>,
    pub transfers: Vec<TransferState>,
    /// Uploads queued before the actor was ready.
    pub pending_uploads: Vec<PathBuf>,
    /// Actor command sender (set when the actor is spawned via start_actor).
    pub cmd_tx: Option<mpsc::UnboundedSender<SftpCmd>>,
}

impl SftpBrowser {
    /// Create a new browser. `initial_path` is a placeholder; the actor will
    /// canonicalize "~" once started.
    pub fn new(host_name: String, initial_path: String, _cx: &mut Context<Self>) -> Self {
        Self {
            host_name,
            current_path: initial_path,
            entries: Vec::new(),
            loading: true,
            error: None,
            selected_index: None,
            transfers: Vec::new(),
            pending_uploads: Vec::new(),
            cmd_tx: None,
        }
    }

    /// Consume an SftpEvent and update state. Called from the pump loop.
    fn handle_event(&mut self, ev: SftpEvent, cx: &mut Context<Self>) {
        match ev {
            SftpEvent::Ready { home } => {
                self.current_path = home;
                self.loading = true; // listing is sent right after Ready
                self.error = None;
            }
            SftpEvent::DirListed { path, entries } => {
                self.current_path = path;
                self.entries = entries;
                self.loading = false;
                self.error = None;
                self.selected_index = None;
            }
            SftpEvent::MkdirDone { parent } => {
                let _ = self.send_cmd(SftpCmd::ListDir { path: parent });
            }
            SftpEvent::RenameDone | SftpEvent::RemoveDone => {
                self.refresh(cx);
            }
            SftpEvent::Transfer(tp) => match tp {
                TransferProgress::Started {
                    id,
                    filename,
                    total,
                    uploading,
                } => {
                    self.transfers.retain(|t| t.id != id);
                    self.transfers.push(TransferState {
                        id,
                        filename,
                        total,
                        transferred: 0,
                        uploading,
                        done: false,
                        error: None,
                    });
                }
                TransferProgress::Progress {
                    id,
                    transferred,
                    total,
                } => {
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        t.transferred = transferred;
                        t.total = total;
                    }
                }
                TransferProgress::Completed { id } => {
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        t.done = true;
                    }
                    self.refresh(cx);
                }
                TransferProgress::Error { id, message } => {
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) {
                        t.error = Some(message);
                    }
                }
            },
            SftpEvent::Error { message } => {
                self.loading = false;
                self.error = Some(message);
            }
        }
        if self.transfers.len() > 5 {
            self.transfers.retain(|t| !t.done);
        }
        cx.notify();
    }

    fn send_cmd(&self, cmd: SftpCmd) -> bool {
        if let Some(tx) = &self.cmd_tx {
            tx.send(cmd).is_ok()
        } else {
            false
        }
    }

    /// Refresh the current directory listing.
    pub fn refresh(&mut self, _cx: &mut Context<Self>) {
        let path = self.current_path.clone();
        self.loading = true;
        self.error = None;
        let _ = self.send_cmd(SftpCmd::ListDir { path });
    }

    /// Navigate to an absolute path.
    pub fn cd(&mut self, path: String, _cx: &mut Context<Self>) {
        self.loading = true;
        self.error = None;
        let _ = self.send_cmd(SftpCmd::ListDir { path });
    }

    /// Navigate into a child entry.
    pub fn cd_into(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(e) = self.entries.get(idx) {
            if e.is_dir {
                self.cd(e.path.clone(), cx);
            }
        }
    }

    /// Enqueue a single upload.
    pub fn enqueue_upload(&mut self, local: PathBuf, _cx: &mut Context<Self>) {
        let filename = local
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file")
            .to_string();
        let remote = format!("{}/{}", self.current_path.trim_end_matches('/'), filename);
        let id = remote.clone();
        if self.cmd_tx.is_none() {
            self.pending_uploads.push(local);
        } else {
            let _ = self.send_cmd(SftpCmd::Upload {
                local,
                remote,
                id,
            });
        }
    }

    /// Enqueue multiple uploads.
    pub fn enqueue_uploads(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        for p in paths {
            self.enqueue_upload(p, cx);
        }
    }

    /// Show native open-file dialog and upload chosen files.
    pub fn pick_and_upload(&mut self, cx: &mut Context<Self>) {
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("选择要上传的文件".into()),
        });
        let entity = cx.entity();
        cx.spawn(async move |_this, cx| {
            if let Ok(Ok(Some(paths))) = prompt.await {
                let paths: Vec<PathBuf> = paths.into_iter().map(|p| p.to_path_buf()).collect();
                let _ = entity.update(cx, move |this, cx| {
                    this.enqueue_uploads(paths, cx);
                });
            }
        })
        .detach();
    }

    /// Show native save dialog and download the remote entry to the chosen path.
    pub fn download_entry(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.entries.get(idx).cloned() else {
            return;
        };
        if entry.is_dir {
            return;
        }
        let entity = cx.entity();
        let remote_path = entry.path.clone();
        let suggested = entry.name.clone();

        // Use a oneshot channel to bring the chosen PathBuf back from an OS thread.
        let (tx, rx) = smol::channel::bounded::<Option<PathBuf>>(1);
        std::thread::spawn(move || {
            let local_path = rfd::FileDialog::new()
                .set_file_name(&suggested)
                .save_file();
            let _ = smol::block_on(tx.send(local_path));
        });

        cx.spawn(async move |_this, cx| {
            if let Ok(Some(local_path)) = rx.recv().await {
                let id = remote_path.clone();
                let _ = entity.update(cx, move |this, _cx| {
                    let _ = this.send_cmd(SftpCmd::Download {
                        remote: remote_path,
                        local: local_path,
                        id,
                    });
                });
            }
        })
        .detach();
    }

    /// Open a dialog with a text input for the new folder name, then send
    /// the Mkdir command on confirm.
    pub fn prompt_mkdir(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parent = self.current_path.clone();
        let input = cx.new(|cx| {
            let mut s = InputState::new(window, cx).placeholder("文件夹名称");
            s.set_value("new_folder", window, cx);
            s
        });
        let entity = cx.entity();
        window.open_dialog(cx, move |dialog, _window, cx| {
            let input_c = input.clone();
            let input_f = input.clone();
            let parent_for_content = parent.clone();
            let parent_for_footer = parent.clone();
            let entity_for_footer = entity.clone();
            dialog
                .title("新建文件夹")
                .content(move |content, _window, _cx| {
                    content.child(
                        v_flex()
                            .p_4()
                            .w(px(340.))
                            .gap_2()
                            .child(
                                div()
                                    .text_sm()
                                    .text_color(gpui::hsla(0.0, 0.0, 0.5, 1.0))
                                    .child(format!("在 {} 下创建：", parent_for_content)),
                            )
                            .child(Input::new(&input_c).small()),
                    )
                })
                .footer(
                    Button::new("sftp-mkdir-confirm")
                        .primary()
                        .small()
                        .label("创建")
                        .on_click(move |_ev, window, cx| {
                            let name = input_f.read(cx).value().trim().to_string();
                            window.close_dialog(cx);
                            if name.is_empty() || name.contains('/') || name.contains('\\') {
                                return;
                            }
                            let _ = entity_for_footer.update(cx, |this, _cx| {
                                let _ = this.send_cmd(SftpCmd::Mkdir {
                                    parent: parent_for_footer.clone(),
                                    name,
                                });
                            });
                        }),
                )
        });
    }
}

impl Render for SftpBrowser {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let bg = theme.background;
        let border = theme.border;
        let muted = theme.muted_foreground;
        let fg = theme.foreground;
        let accent = theme.accent;

        let (_root, breadcrumbs) = crate::ssh::sftp::split_path(&self.current_path);
        let entries = self.entries.clone();
        let transfers = self.transfers.clone();
        let loading = self.loading;
        let error = self.error.clone();

        v_flex()
            .size_full()
            .min_h_0()
            .overflow_hidden()
            .bg(bg)
            .text_color(fg)
            // Toolbar
            .child(
                h_flex()
                    .h(px(38.))
                    .px_3()
                    .gap_2()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .bg(theme.muted)
                    .child(
                        Button::new("sftp-refresh")
                            .ghost()
                            .xsmall()
                            .label("↻")
                            .tooltip("刷新")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.refresh(cx);
                            })),
                    )
                    .child(
                        Button::new("sftp-upload")
                            .ghost()
                            .xsmall()
                            .label("↑ 上传")
                            .tooltip("上传文件…")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.pick_and_upload(cx);
                            })),
                    )
                    .child(
                        Button::new("sftp-mkdir")
                            .ghost()
                            .xsmall()
                            .label("新建文件夹")
                            .tooltip("在当前目录新建文件夹")
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.prompt_mkdir(window, cx);
                            })),
                    )
                    .child(div().w(px(8.)))
                    // Breadcrumbs
                    .child(
                        h_flex()
                            .gap_1()
                            .items_center()
                            .flex_1()
                            .children(breadcrumbs.iter().enumerate().map(|(i, (path, label))| {
                                let is_last = i + 1 == breadcrumbs.len();
                                let path_c = path.clone();
                                h_flex()
                                    .gap_1()
                                    .items_center()
                                    .cursor_pointer()
                                    .when(!is_last, |d| d.text_color(muted))
                                    .when(is_last, |d| {
                                        d.text_color(fg).font_weight(FontWeight::SEMIBOLD)
                                    })
                                    .child(
                                        div()
                                            .text_sm()
                                            .hover(|s| s.text_color(accent))
                                            .child(label.clone())
                                            .on_mouse_down(
                                                MouseButton::Left,
                                                cx.listener(move |this, _, _, cx| {
                                                    if !is_last {
                                                        this.cd(path_c.clone(), cx);
                                                    }
                                                }),
                                            ),
                                    )
                                    .when(!is_last, |d| {
                                        d.child(div().text_sm().text_color(muted).child("/"))
                                    })
                            })),
                    ),
            )
            // Column header
            .child(
                h_flex()
                    .h(px(28.))
                    .px_3()
                    .items_center()
                    .border_b_1()
                    .border_color(border)
                    .text_xs()
                    .text_color(muted)
                    .child(div().flex_1().child("名称"))
                    .child(div().w(px(100.)).child("大小"))
                    .child(div().w(px(160.)).child("修改时间")),
            )
            // File list
            .child(
                div()
                    .id("sftp-file-list")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children({
                        let mut rows: Vec<AnyElement> = Vec::new();
                        if loading {
                            rows.push(
                                div()
                                    .p_3()
                                    .text_sm()
                                    .text_color(muted)
                                    .child("加载中…")
                                    .into_any_element(),
                            );
                        } else if let Some(err) = error {
                            rows.push(
                                div()
                                    .p_3()
                                    .text_sm()
                                    .text_color(theme.danger)
                                    .child(format!("错误: {err}"))
                                    .into_any_element(),
                            );
                        } else if entries.is_empty() {
                            rows.push(
                                div()
                                    .p_3()
                                    .text_sm()
                                    .text_color(muted)
                                    .child("（空目录）")
                                    .into_any_element(),
                            );
                        } else {
                            for (i, entry) in entries.iter().enumerate() {
                                let is_sel = self.selected_index == Some(i);
                                let icon = if entry.is_dir {
                                    "📁"
                                } else if entry.is_symlink {
                                    "🔗"
                                } else {
                                    "📄"
                                };
                                let size_str = if entry.is_dir {
                                    "—".into()
                                } else {
                                    humanize_size(entry.size)
                                };
                                let mtime_str = entry
                                    .modified
                                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                                    .unwrap_or_else(|| "—".into());
                                let path_for_dl = entry.path.clone();
                                let path_for_cd = entry.path.clone();
                                let e_is_dir = entry.is_dir;
                                let name = entry.name.clone();
                                rows.push(
                                    h_flex()
                                        .h(px(28.))
                                        .px_3()
                                        .items_center()
                                        .cursor_pointer()
                                        .when(is_sel, |d| d.bg(accent.opacity(0.2)))
                                        .hover(|d| d.bg(theme.muted))
                                        .on_mouse_down(
                                            MouseButton::Left,
                                            cx.listener(move |this, _, _, cx| {
                                                this.selected_index = Some(i);
                                                if e_is_dir {
                                                    this.cd(path_for_cd.clone(), cx);
                                                }
                                                cx.notify();
                                            }),
                                        )
                                        .child(
                                            h_flex()
                                                .flex_1()
                                                .gap_2()
                                                .items_center()
                                                .child(div().text_sm().child(icon))
                                                .child(div().text_sm().truncate().child(name)),
                                        )
                                        .child(
                                            div().w(px(100.)).text_xs().text_color(muted).child(size_str),
                                        )
                                        .child(
                                            div().w(px(160.)).text_xs().text_color(muted).child(mtime_str),
                                        )
                                        .child(
                                            h_flex()
                                                .gap_1()
                                                .child(
                                                    Button::new(("sftp-dl", i))
                                                        .ghost()
                                                        .xsmall()
                                                        .label("↓")
                                                        .tooltip("下载")
                                                        .on_click(cx.listener(
                                                            move |this, _, _, cx| {
                                                                this.download_entry(i, cx);
                                                            },
                                                        )),
                                                )
                                                .child(
                                                    Button::new(("sftp-del", i))
                                                        .ghost()
                                                        .xsmall()
                                                        .label("🗑")
                                                        .text_color(theme.danger)
                                                        .tooltip("删除")
                                                        .on_click(cx.listener(move |this, _, _, cx| {
                                                            let path = path_for_dl.clone();
                                                            let is_dir = e_is_dir;
                                                            let _ = this.send_cmd(SftpCmd::Remove {
                                                                path,
                                                                is_dir,
                                                            });
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                        .into_any_element(),
                                );
                            }
                        }
                        rows
                    }),
            )
            // Transfer bar
            .child({
                let visible: Vec<&TransferState> = transfers
                    .iter()
                    .filter(|t| !t.done || t.error.is_some())
                    .take(3)
                    .collect();
                v_flex().when(!visible.is_empty(), |c| {
                    c.border_t_1()
                        .border_color(border)
                        .bg(theme.muted)
                        .children(visible.into_iter().map(|t| {
                            let pct = if t.total == 0 {
                                0.0
                            } else {
                                (t.transferred as f64 / t.total as f64).clamp(0.0, 1.0)
                            };
                            let bar_w = 180.0 * pct;
                            let arrow = if t.uploading { "↑" } else { "↓" };
                            let label = if let Some(err) = &t.error {
                                format!("{} {} 错误: {}", arrow, t.filename, err)
                            } else if t.done {
                                format!("{} {} 完成", arrow, t.filename)
                            } else {
                                format!(
                                    "{} {}  {} / {}",
                                    arrow,
                                    t.filename,
                                    humanize_size(t.transferred),
                                    humanize_size(t.total)
                                )
                            };
                            h_flex()
                                .h(px(22.))
                                .px_3()
                                .gap_2()
                                .items_center()
                                .child(div().text_xs().child(label))
                                .child(
                                    div()
                                        .w(px(180.))
                                        .h(px(6.))
                                        .rounded(px(3.))
                                        .bg(border)
                                        .child(
                                            div().w(px(bar_w as f32)).h_full().rounded(px(3.)).bg(accent),
                                        ),
                                )
                        }))
                })
            })
    }
}

/// Open the SFTP subsystem on `conn` (running inside a dedicated OS thread
/// with its own Tokio runtime, since russh-sftp requires a Tokio reactor),
/// canonicalize the home path, then run the SFTP actor command loop on that
/// same runtime. Returns a cmd sender and an event receiver immediately;
/// the actor sends a `Ready { home }`/`Error` event once SFTP is open.
///
/// The caller should spawn a GPUI pump task that drains `event_rx` and calls
/// `SftpBrowser::handle_event`.
fn spawn_sftp_actor(
    conn: crate::ssh::client::SshConnection,
) -> (
    mpsc::UnboundedSender<SftpCmd>,
    mpsc::UnboundedReceiver<SftpEvent>,
) {
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<SftpCmd>();
    let (event_tx, event_rx) = mpsc::unbounded_channel::<SftpEvent>();

    std::thread::Builder::new()
        .name("verve-sftp-actor".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::error!("SFTP actor tokio runtime init failed: {e}");
                    let _ = event_tx.send(SftpEvent::Error {
                        message: format!("Tokio runtime 初始化失败: {e}"),
                    });
                    return;
                }
            };
            rt.block_on(async move {
                // Open the SFTP subsystem. This calls russh channels +
                // russh_sftp::SftpSession::new, both of which require a Tokio
                // reactor, so it must run here (not on GPUI's smol executor).
                let sftp = match conn.open_sftp().await {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = event_tx.send(SftpEvent::Error {
                            message: format!("无法打开 SFTP: {e}"),
                        });
                        return;
                    }
                };

                let home = sftp
                    .canonicalize(".")
                    .await
                    .unwrap_or_else(|_| "/".to_string());
                // Send a synthetic DirListed for the home path to both seed
                // the current_path and serve as the "ready" signal. The UI
                // will use the `path` field to update current_path before
                // rendering entries.
                let initial_entries = match sftp.list_dir(&home).await {
                    Ok(entries) => entries,
                    Err(e) => {
                        let _ = event_tx.send(SftpEvent::Error {
                            message: format!("读取主目录失败: {e}"),
                        });
                        return;
                    }
                };
                let _ = event_tx.send(SftpEvent::Ready { home: home.clone() });
                let _ = event_tx.send(SftpEvent::DirListed {
                    path: home,
                    entries: initial_entries,
                });

                // Command loop.
                while let Some(cmd) = cmd_rx.recv().await {
                    match cmd {
                        SftpCmd::ListDir { path } => {
                            let ev = match sftp.list_dir(&path).await {
                                Ok(entries) => SftpEvent::DirListed { path, entries },
                                Err(e) => SftpEvent::Error { message: e },
                            };
                            let _ = event_tx.send(ev);
                        }
                        SftpCmd::Mkdir { parent, name } => {
                            let full =
                                format!("{}/{}", parent.trim_end_matches('/'), name);
                            let ev = match sftp.mkdir(&full).await {
                                Ok(()) => SftpEvent::MkdirDone { parent },
                                Err(e) => SftpEvent::Error { message: e },
                            };
                            let _ = event_tx.send(ev);
                        }
                        SftpCmd::Rename { from, to } => {
                            let ev = match sftp.rename(&from, &to).await {
                                Ok(()) => SftpEvent::RenameDone,
                                Err(e) => SftpEvent::Error { message: e },
                            };
                            let _ = event_tx.send(ev);
                        }
                        SftpCmd::Remove { path, is_dir } => {
                            let res = if is_dir {
                                sftp.remove_dir(&path).await
                            } else {
                                sftp.remove_file(&path).await
                            };
                            let ev = match res {
                                Ok(()) => SftpEvent::RemoveDone,
                                Err(e) => SftpEvent::Error { message: e },
                            };
                            let _ = event_tx.send(ev);
                        }
                        SftpCmd::Upload { local, remote, id } => {
                            let (ptx, mut prx) =
                                mpsc::unbounded_channel::<TransferProgress>();
                            let fwd = event_tx.clone();
                            let id_fwd = id.clone();
                            tokio::spawn(async move {
                                while let Some(tp) = prx.recv().await {
                                    let _ = fwd.send(SftpEvent::Transfer(tp));
                                }
                            });
                            let res = sftp.upload_file(&local, &remote, ptx).await;
                            let ev = match res {
                                Ok(()) => SftpEvent::Transfer(
                                    TransferProgress::Completed { id },
                                ),
                                Err(e) => SftpEvent::Transfer(TransferProgress::Error {
                                    id: id_fwd,
                                    message: e,
                                }),
                            };
                            let _ = event_tx.send(ev);
                        }
                        SftpCmd::Download { remote, local, id } => {
                            let (ptx, mut prx) =
                                mpsc::unbounded_channel::<TransferProgress>();
                            let fwd = event_tx.clone();
                            let id_fwd = id.clone();
                            tokio::spawn(async move {
                                while let Some(tp) = prx.recv().await {
                                    let _ = fwd.send(SftpEvent::Transfer(tp));
                                }
                            });
                            let res = sftp.download_file(&remote, &local, ptx).await;
                            let ev = match res {
                                Ok(()) => SftpEvent::Transfer(
                                    TransferProgress::Completed { id },
                                ),
                                Err(e) => SftpEvent::Transfer(TransferProgress::Error {
                                    id: id_fwd,
                                    message: e,
                                }),
                            };
                            let _ = event_tx.send(ev);
                        }
                    }
                }
            });
        })
        .expect("spawn sftp actor thread");

    (cmd_tx, event_rx)
}

/// Events from the SFTP background actor back to the UI pump.
#[derive(Debug, Clone)]
pub enum SftpEvent {
    /// Emitted once when the SFTP subsystem is open and the home path is known.
    Ready { home: String },
    DirListed {
        path: String,
        entries: Vec<FileEntry>,
    },
    MkdirDone {
        parent: String,
    },
    RenameDone,
    RemoveDone,
    Transfer(TransferProgress),
    Error {
        message: String,
    },
}

/// Open the SFTP session on `conn` (in a background tokio runtime), and wire
/// the browser's pump loop to drain events. Public entry point used by
/// `SshPanel`.
pub fn bind_sftp_browser(
    browser: &mut SftpBrowser,
    conn: crate::ssh::client::SshConnection,
    cx: &mut Context<SftpBrowser>,
) {
    let entity = cx.entity();

    // Spawn the actor immediately (it opens SFTP on its own tokio runtime on
    // an OS thread, then sends events). The returned channels are Send, so we
    // can move them into the GPUI pump.
    let (cmd_tx, mut event_rx) = spawn_sftp_actor(conn);

    // Mark the browser as ready to accept commands once the actor signals
    // Ready. Until then cmd_tx is held here (the actor is still initializing)
    // and any pending uploads will be queued via pending_uploads.
    let mut cmd_tx_for_browser = Some(cmd_tx);

    cx.spawn(async move |_this, cx| {
        while let Some(ev) = event_rx.recv().await {
            let is_ready = matches!(ev, SftpEvent::Ready { .. });
            let is_fatal_err = matches!(ev, SftpEvent::Error { .. });
            let _ = entity.update(cx, |b, cx| {
                // On Ready, bind the cmd_tx so subsequent uploads/refresh go
                // through immediately, and drain any pending uploads.
                if is_ready {
                    if let Some(tx) = cmd_tx_for_browser.take() {
                        b.cmd_tx = Some(tx);
                        let pending = std::mem::take(&mut b.pending_uploads);
                        for local in pending {
                            b.enqueue_upload(local, cx);
                        }
                    }
                    b.loading = false;
                }
                if is_fatal_err {
                    b.loading = false;
                }
                b.handle_event(ev, cx);
            });
            if is_fatal_err {
                break;
            }
        }
    })
    .detach();
}
