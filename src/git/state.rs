//! `GitState` — a GPUI entity that wraps the gix operations, runs them on a
//! background executor, and emits [`GitEvent::Updated`] whenever the cached
//! snapshot changes.
//!
//! All gix calls are blocking; they are dispatched on the background executor
//! so the UI thread never stalls. Results are applied on the main thread via
//! `weak_entity().update(...)`.

use std::path::PathBuf;

use gpui::{AppContext as _, Context, Entity, EventEmitter};

use super::ops::{self, GitAuth, GitCommit, GitStatus};

/// Outcome of a raced sync+watchdog — distinguishes "completed (ok/err)" from
/// "timed out so we force-cleared busy".
enum SyncOutcome {
    Done(anyhow::Result<Vec<String>>),
    Timeout,
}

/// Event emitted whenever the cached git snapshot is refreshed or changes.
#[derive(Clone, Debug)]
pub enum GitEvent {
    Updated,
}

/// The cached git snapshot + user configuration. Owned by the main thread.
pub struct GitState {
    /// `~/.verve` — the repository workdir.
    pub dir: PathBuf,
    /// Whether a repository has been initialised at `dir`.
    pub initialized: bool,
    /// Last known status.
    pub status: GitStatus,
    /// Last known commit list (newest first).
    pub commits: Vec<GitCommit>,
    /// Local branch names.
    pub branches: Vec<String>,
    /// Configured origin URL, if any.
    pub remote: Option<String>,

    // --- user config (persisted via PanelLayout) ---
    pub auto_commit: bool,
    pub auto_push: bool,
    pub auth: GitAuth,

    // --- transient UI state ---
    /// Human label of the in-flight background op, when set.
    pub busy: Option<String>,
    /// Last operation result message + success flag, for a transient banner.
    pub last_result: Option<(String, bool)>,
    /// Cancellation flag for the running auto-sync loop; flipped when the
    /// interval is changed so the old loop exits and a new one starts.
    auto_sync_cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl GitState {
    /// Create + initialise the entity: ensure the repo exists, then refresh.
    /// `cx` may be any context — we build the entity off the App.
    pub fn init(dir: PathBuf, cx: &mut gpui::App) -> Entity<Self> {
        cx.new(|cx| {
            let initialized = ops::is_repo(&dir);
            let mut s = Self {
                dir,
                initialized,
                status: GitStatus::default(),
                commits: Vec::new(),
                branches: Vec::new(),
                remote: None,
                auto_commit: true,
                auto_push: false,
                auth: GitAuth::default(),
                busy: None,
                last_result: None,
                auto_sync_cancel: None,
            };
            if initialized {
                s.refresh_async(cx);
            }
            s
        })
    }

    /// Load persisted git config into this entity (called once at startup,
    /// before any refresh).
    pub fn load_config(
        &mut self,
        auto_commit: bool,
        auto_push: bool,
        remote: Option<String>,
        username: String,
        token: String,
    ) {
        self.auto_commit = auto_commit;
        self.auto_push = auto_push;
        self.remote = remote;
        self.auth = GitAuth { username, token };
    }

    /// Whether git operations are currently in flight.
    pub fn is_busy(&self) -> bool {
        self.busy.is_some()
    }

    /// Start a recurring background timer that auto-commits + syncs every
    /// `interval`. Only fires when the repo is initialized and not already
    /// busy (a manual sync in progress takes priority). Detached; runs for
    /// the lifetime of the entity.
    ///
    /// Calling this again cancels any previously-running loop (the old
    /// interval is replaced immediately), so settings changes take effect
    /// live without an app restart.
    pub fn start_auto_sync(&mut self, cx: &mut Context<Self>, interval: std::time::Duration) {
        // Cancel any prior loop before starting a new one.
        if let Some(cancel) = self.auto_sync_cancel.take() {
            cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        }
        let cancel = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.auto_sync_cancel = Some(cancel.clone());
        log::info!(
            "启动自动同步定时器：每 {} 分钟自动提交+同步",
            interval.as_secs() / 60
        );
        let weak = cx.weak_entity();
        cx.spawn(async move |_, cx| {
            loop {
                cx.background_executor().timer(interval).await;
                // Check for cancellation between ticks.
                if cancel.load(std::sync::atomic::Ordering::SeqCst) {
                    break;
                }
                let triggered = weak.update(cx, |this, cx| {
                    if this.initialized && !this.is_busy() {
                        log::info!("自动同步定时器触发 → sync_async");
                        this.sync_async(None, cx);
                        true
                    } else {
                        false
                    }
                });
                if triggered.is_err() {
                    // Entity dropped — stop the timer.
                    break;
                }
            }
        })
        .detach();
    }

    // -----------------------------------------------------------------
    // Operations — each spawns a background task and refreshes on success.
    // -----------------------------------------------------------------

    /// (Re)read status / log / branches / remote from disk. Runs without
    /// touching the `busy` flag (it's a background re-read, not a user op) so
    /// it doesn't clobber a "同步中" indicator or pollute the result banner.
    pub fn refresh_async(&mut self, cx: &mut Context<Self>) {
        let dir = self.dir.clone();
        let weak = cx.weak_entity();
        // Run blocking gix ops on a DEDICATED OS thread (8 MB stack on macOS) instead of the
        // GPUI background executor (GCD worker, ~512 KB stack). gix's loose-object write path
        // uses miniz_oxide::deflate which recurses deeply and overflows the small worker stack
        // (SIGBUS / "Thread stack size exceeded" observed after autosave).
        let handle = std::thread::spawn(move || {
            let repo = ops::ensure_repo(&dir)?;
            let status = ops::status(&repo)?;
            let commits = ops::log(&repo, 50).unwrap_or_default();
            let branches = ops::branches(&repo).unwrap_or_default();
            let remote = ops::get_remote(&repo);
            Ok::<_, anyhow::Error>((status, commits, branches, remote))
        });
        cx.spawn(async move |_, cx| {
            let res = handle
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("refresh 线程崩溃")));
            let _ = weak.update(cx, |this, cx| {
                if let Ok((status, commits, branches, remote)) = res {
                    this.status = status;
                    this.commits = commits;
                    this.branches = branches;
                    this.remote = remote;
                }
                this.initialized = ops::is_repo(&this.dir);
                cx.emit(GitEvent::Updated);
                cx.notify();
            });
        })
        .detach();
    }

    /// Initialise the repository if it doesn't exist yet.
    pub fn init_repo_async(&mut self, cx: &mut Context<Self>) {
        self.spawn_read(cx, "初始化仓库", move |_repo| {
            // ensure_repo already ran before this closure; nothing more to do.
            Ok(Box::new(|this: &mut GitState| {
                this.initialized = true;
                None
            })
                as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>)
        });
    }

    /// Commit all changes with `message`.
    pub fn commit_async(&mut self, message: String, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        self.spawn_read(cx, "提交中", move |repo| {
            match ops::commit(&repo, &message)? {
                Some(id) => Ok(
                    Box::new(move |_this: &mut GitState| Some(format!("已提交 {id}")))
                        as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>,
                ),
                None => anyhow::bail!("没有可提交的更改"),
            }
        });
    }

    /// Create a branch and switch to it.
    pub fn create_branch_async(&mut self, name: String, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let label = format!("已创建并切换到 {name}");
        self.spawn_read(cx, "创建分支", move |repo| {
            ops::create_branch(&repo, &name)?;
            Ok(Box::new(move |_this: &mut GitState| Some(label))
                as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>)
        });
    }

    /// Switch to an existing local branch (rewrites worktree files via git CLI).
    pub fn checkout_async(&mut self, name: String, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let auth = self.auth.clone();
        let label = format!("已切换到 {name}");
        self.spawn_read(cx, "切换分支", move |repo| {
            ops::checkout(&repo, &name, &auth)?;
            Ok(Box::new(move |_this: &mut GitState| Some(label))
                as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>)
        });
    }

    /// Set the origin URL.
    pub fn set_remote_async(&mut self, url: String, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let dir = self.dir.clone();
        self.spawn_read(cx, "配置远程", move |repo| {
            ops::set_remote(&repo, &dir, &url)?;
            Ok(Box::new(move |this: &mut GitState| {
                this.remote = Some(url);
                Some("已配置远程仓库".to_string())
            })
                as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>)
        });
    }

    /// Pull (fetch + fast-forward) from origin. Uses the git CLI so the local
    /// branch and worktree actually move forward (gix fetch can't).
    pub fn pull_async(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let auth = self.auth.clone();
        self.spawn_read(cx, "拉取中", move |repo| {
            let msg = ops::pull(&repo, &auth)?;
            Ok(Box::new(move |_this: &mut GitState| {
                Some(if msg.is_empty() {
                    "已是最新".to_string()
                } else {
                    msg
                })
            })
                as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>)
        });
    }

    /// Push the current branch to origin (git CLI, authenticated).
    pub fn push_async(&mut self, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let auth = self.auth.clone();
        self.spawn_read(cx, "推送中", move |repo| {
            let msg = ops::push(&repo, &auth)?;
            Ok(Box::new(move |_this: &mut GitState| {
                Some(if msg.is_empty() {
                    "已推送到 origin".to_string()
                } else {
                    msg
                })
            })
                as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>)
        });
    }

    /// Delete a local branch.
    pub fn delete_branch_async(&mut self, name: String, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        let auth = self.auth.clone();
        let label = format!("已删除分支 {name}");
        self.spawn_read(cx, "删除分支", move |repo| {
            ops::delete_branch(&repo, &name, &auth)?;
            Ok(Box::new(move |_this: &mut GitState| Some(label))
                as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>)
        });
    }

    /// Create-or-checkout a branch (for workspace switching). Returns nothing
    /// here — the result banner shows which happened. Only acts when the repo
    /// is initialised; a no-op otherwise (degraded mode without git).
    pub fn switch_branch_async(&mut self, branch: String, cx: &mut Context<Self>) {
        if self.is_busy() {
            return;
        }
        if !self.initialized {
            log::info!("switch_branch_async: git 未初始化，跳过分支切换");
            return;
        }
        let auth = self.auth.clone();
        let br = branch.clone();
        self.spawn_read(cx, "切换工作空间", move |repo| {
            let created = ops::create_or_checkout(&repo, &br, &auth)?;
            let msg = if created {
                format!("已创建并切换到 {br}")
            } else {
                format!("已切换到 {br}")
            };
            Ok(Box::new(move |_this: &mut GitState| Some(msg))
                as Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>)
        });
    }

    /// Sync = commit (if dirty) → pull (if upstream) → push. The single entry
    /// point used by the sync button. `ops::sync` returns a step-by-step log.
    pub fn sync_async(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        if self.is_busy() {
            log::warn!("sync_async: 已有操作进行中（busy），跳过");
            return;
        }
        log::info!("sync_async: 启动后台同步任务（专用线程）");
        let dir = self.dir.clone();
        let auth = self.auth.clone();
        let msg = message.unwrap_or_else(default_message);
        self.busy = Some("同步中".to_string());
        cx.emit(GitEvent::Updated);
        cx.notify();
        let weak = cx.weak_entity();
        // Run the blocking sync on a DEDICATED OS thread (not the cooperative
        // background-executor pool). A blocking git network call can't starve
        // the executor this way, and we can race join() against a timer.
        let handle = std::thread::spawn(move || ops::sync(&dir, &auth, &msg));
        cx.spawn(async move |_, cx| {
            // Watchdog loop: try to join the thread, yielding to the executor
            // between attempts; give up after 90s so the UI is never stuck.
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(90);
            let outcome = loop {
                // Poll the dedicated thread every 200ms (yielding to the
                // executor between checks) so the timer watchdog can fire.
                match handle.is_finished() {
                    true => {
                        let res = handle
                            .join()
                            .unwrap_or_else(|_| Err(anyhow::anyhow!("同步线程崩溃")));
                        break SyncOutcome::Done(res);
                    }
                    false => {
                        if std::time::Instant::now() >= deadline {
                            break SyncOutcome::Timeout;
                        }
                        cx.background_executor()
                            .timer(std::time::Duration::from_millis(200))
                            .await;
                    }
                }
            };
            let _ = weak.update(cx, |this, cx| {
                this.busy = None;
                match outcome {
                    SyncOutcome::Done(Ok(logs)) => {
                        log::info!("sync_async: 后台任务完成 — {:?}", logs);
                        let msg = summarize_sync(&logs);
                        let failed = logs.iter().any(|l| l.contains("失败"));
                        this.last_result = Some((msg, !failed));
                    }
                    SyncOutcome::Done(Err(e)) => {
                        log::error!("sync_async: 后台任务异常 — {e}");
                        this.last_result = Some((format!("同步失败：{e}"), false));
                    }
                    SyncOutcome::Timeout => {
                        log::error!("sync_async: 后台任务 90s 未返回，强制清除 busy");
                        this.last_result =
                            Some(("同步超时（90s）— 远程地址或认证可能有误".to_string(), false));
                    }
                }
                cx.emit(GitEvent::Updated);
                this.refresh_async(cx);
            });
        })
        .detach();
    }

    // -----------------------------------------------------------------
    // Internal: the shared background-read runner.
    // -----------------------------------------------------------------

    /// Open the repo (initialising if needed), run `op` on the background
    /// executor, then apply the returned closure on the main thread and
    /// refresh + emit. `label` shows up in the busy indicator.
    ///
    /// The apply closure may return `Some(message)` to override the default
    /// "<label>完成" banner with a more specific result (e.g. the git CLI's
    /// output); returning `None` falls back to the generic message.
    fn spawn_read<F>(&mut self, cx: &mut Context<Self>, label: &str, op: F)
    where
        F: FnOnce(
                gix::Repository,
            )
                -> anyhow::Result<Box<dyn FnOnce(&mut GitState) -> Option<String> + Send>>
            + Send
            + 'static,
    {
        let dir = self.dir.clone();
        self.busy = Some(label.to_string());
        // Notify + emit so title-bar observers (VerveApp) re-render the busy
        // indicator immediately — without this emit the click looks unresponsive.
        cx.emit(GitEvent::Updated);
        cx.notify();
        let weak = cx.weak_entity();
        let label_owned = label.to_string();
        // Run blocking gix ops on a DEDICATED OS thread (8 MB stack) instead of the
        // GPUI background executor (GCD worker, ~512 KB stack). gix's write path
        // (commit/checkout) uses miniz_oxide::deflate which recurses deeply and can
        // overflow the small worker stack.
        let handle = std::thread::spawn(move || {
            let repo = ops::ensure_repo(&dir)?;
            op(repo)
        });
        cx.spawn(async move |_, cx| {
            let res = handle
                .join()
                .unwrap_or_else(|_| Err(anyhow::anyhow!("{label_owned} 后台线程崩溃")));
            let _ = weak.update(cx, |this, cx| {
                this.busy = None;
                match res {
                    Ok(apply) => {
                        let custom = apply(this);
                        let msg = custom.unwrap_or_else(|| format!("{label_owned}完成"));
                        let failed = msg.contains("失败");
                        this.last_result = Some((msg, !failed));
                    }
                    Err(e) => {
                        this.last_result = Some((format!("{label_owned}失败：{e}"), false));
                    }
                }
                // Always ensure the initialized flag is truthful.
                this.initialized = ops::is_repo(&this.dir);
                // Emit so observers see the cleared busy + the result banner.
                cx.emit(GitEvent::Updated);
                this.refresh_async(cx);
            });
        })
        .detach();
    }
}

impl EventEmitter<GitEvent> for GitState {}

/// Generate the default commit message ("Verve auto-save · <timestamp>").
fn default_message() -> String {
    let now = chrono::Local::now();
    format!("Verve 自动保存 · {}", now.format("%Y-%m-%d %H:%M:%S"))
}

/// Turn the step-by-step `ops::sync` log into a concise title-bar message.
/// Instead of joining all raw lines (which can be long and multi-line), this
/// picks the meaningful outcome:
///   - "同步完成 · 已提交 <id>" if we committed + pushed
///   - "已是最新" if nothing changed
///   - "同步完成" as a generic success
fn summarize_sync(logs: &[String]) -> String {
    if logs.is_empty() {
        return "已是最新".to_string();
    }
    // Look for the key signals in priority order.
    let committed = logs.iter().find(|l| l.starts_with("已提交"));
    let pushed = logs
        .iter()
        .any(|l| l.contains("已推送") || l.contains("track 'origin"));
    if let Some(c) = committed {
        // Shorten the commit id for display.
        let short = c
            .split_whitespace()
            .nth(1)
            .map(|id| &id[..7.min(id.len())])
            .unwrap_or("");
        if pushed {
            return format!("同步完成 · 已提交 {short}");
        }
        return format!("已提交 {short}");
    }
    if logs.iter().any(|l| l.contains("未配置远程")) {
        return "已保存到本地".to_string();
    }
    if pushed {
        return "同步完成".to_string();
    }
    "同步完成".to_string()
}
