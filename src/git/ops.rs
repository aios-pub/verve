//! Low-level gix operations. Each function is synchronous and cheap to call
//! from a background executor — none of them touch the UI thread directly.
//!
//! Repository model: the workspace lives at `<data_dir>` (i.e. `~/.verve`)
//! and the whole workspace is a single `workspace.json` tracked at the repo
//! root. Branches share that one file.
//!
//! Network notes: gix 0.69 supports fetch via its blocking network client
//! (used for remote-status reads); high-level push/merge is not yet
//! implemented in gitoxide, so push and pull shell out to a system `git`
//! binary. Credentials for both paths come from the user-configured
//! `GitAuth` (username + token):
//!   - **git CLI** (push/pull): a small `GIT_ASKPASS` helper script reads the
//!     token from an env var set on the child process — the token is never
//!     written to disk, `.git/config`, or the process args. `GIT_TERMINAL_PROMPT=0`
//!     + a 60s timeout guarantee the call can never hang on a missing credential.
//!   - **gix fetch**: a `with_credentials` closure hands the `(username, token)`
//!     directly to gix's HTTP transport as Basic auth.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, Result, anyhow, bail};

/// The GIT_ASKPASS helper script body. It reads credentials from env vars set
/// on the git child process — it contains NO secret itself, so it can live in
/// the data dir and be reused across calls.
const ASKPASS_SCRIPT: &str = "#!/bin/sh\ncase \"$1\" in\n  Username*) printf '%s\\n' \"$VERVE_GIT_USER\" ;;\n  Password*) printf '%s\\n' \"$VERVE_GIT_TOKEN\" ;;\nesac\n";

/// How long a single git CLI network call may run before we abort it. Stops a
/// missing/mistyped credential or an unreachable host from hanging the
/// background executor forever.
const GIT_TIMEOUT: Duration = Duration::from_secs(30);

/// Kill a process by PID. Cross-platform: uses `taskkill` on Windows and
/// `kill -TERM` on Unix. Avoids a hard dependency on the `libc` crate (which
/// lacks `kill` on Windows anyway).
fn kill_process(pid: u32) {
    #[cfg(target_os = "windows")]
    {
        // /T kills the whole process tree, /F forces termination.
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .output();
    }
    #[cfg(not(target_os = "windows"))]
    {
        // Use the kill command instead of libc::kill for portability.
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .output();
    }
}

/// Ensure the `GIT_ASKPASS` helper script exists at `<dir>/.verve-askpass.sh`,
/// is executable (0700), and matches [`ASKPASS_SCRIPT`]. Returns its path.
/// Idempotent: rewrites only when missing or stale.
pub fn ensure_askpass(dir: &Path) -> Result<PathBuf> {
    let path = dir.join(".verve-askpass.sh");
    let needs_write = match std::fs::read_to_string(&path) {
        Ok(existing) => existing != ASKPASS_SCRIPT,
        Err(_) => true,
    };
    if needs_write {
        log::info!("git: writing askpass helper to {:?}", path);
        std::fs::write(&path, ASKPASS_SCRIPT)
            .with_context(|| format!("write askpass helper {:?}", path))?;
        // chmod 0700 (u+rwx). Errors are non-fatal on some filesystems.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
        }
    }
    Ok(path)
}

/// Run `git` in `workdir` with the given args, injecting non-interactive
/// credential handling when `auth` carries a token:
///   - `GIT_TERMINAL_PROMPT=0` — fail fast instead of blocking on a tty prompt.
///   - `GIT_ASKPASS=<helper>` + `VERVE_GIT_USER`/`VERVE_GIT_TOKEN` — supply the
///     username/token without persisting them or exposing them in argv.
///
/// Enforced via [`GIT_TIMEOUT`]: the child is killed if it runs longer, so a
/// hung network call surfaces as an error instead of freezing the UI.
///
/// Returns the trimmed combined stdout+stderr on success, or an error whose
/// message includes git's stderr on failure.
fn run_git(workdir: &Path, auth: &GitAuth, args: &[&str]) -> Result<String> {
    let askpass = ensure_askpass(workdir)?;
    let mut cmd = std::process::Command::new("git");
    cmd.current_dir(workdir);
    cmd.args(["-c", "core.editor=true"]);
    cmd.args(args);
    // Never block on an interactive prompt — fail immediately instead.
    cmd.env("GIT_TERMINAL_PROMPT", "0");
    let has_token = !auth.token.is_empty();
    if has_token {
        cmd.env("GIT_ASKPASS", &askpass);
        // The askpass helper is also consulted for username; pass ours through.
        cmd.env("VERVE_GIT_USER", &auth.username);
        cmd.env("VERVE_GIT_TOKEN", &auth.token);
    }

    log::info!(
        "git {} (cwd={:?}, askpass={}, token={})",
        args.join(" "),
        workdir,
        if has_token { "on" } else { "off" },
        if has_token {
            "set"
        } else {
            "EMPTY — git will have no credentials"
        }
    );

    // Spawn + wait under a watchdog timeout. `wait_with_output` would otherwise
    // block indefinitely on a credential/hang.
    let child = cmd
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("git failed to start — is `git` installed?")?;

    let (tx, rx) = std::sync::mpsc::channel();
    let child_id = child.id();
    let timeout = GIT_TIMEOUT;
    let args_owned: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    std::thread::spawn(move || {
        // `child` is moved into this thread so we can wait on it.
        let out = child.wait_with_output();
        let _ = tx.send(out);
    });
    let output = match rx.recv_timeout(timeout) {
        Ok(out) => {
            let out = out.context("git exited without reporting status")?;
            log::info!(
                "git {} → exit {:?}",
                args_owned.join(" "),
                out.status.code()
            );
            out
        }
        Err(_) => {
            // Timed out — kill the process if it's still around.
            log::warn!(
                "git {} 超时（{}s），终止子进程",
                args_owned.join(" "),
                timeout.as_secs()
            );
            kill_process(child_id);
            bail!(
                "git 操作超时（{}s）— 检查远程地址或认证配置",
                timeout.as_secs()
            )
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if !stdout.is_empty() {
        log::info!("git {} stdout: {}", args_owned.join(" "), stdout);
    }
    if !stderr.is_empty() {
        // Always log stderr — push/pull diagnostics (auth failures, ref rejects)
        // arrive here and are the most common "stuck" cause.
        log::warn!("git {} stderr: {}", args_owned.join(" "), stderr);
    }
    if !output.status.success() {
        let msg = if stderr.is_empty() {
            stdout.clone()
        } else {
            stderr
        };
        bail!("git {} 失败：{msg}", args_owned.join(" "));
    }
    Ok(if stdout.is_empty() { stderr } else { stdout })
}

/// User-supplied credentials for HTTPS remotes. The token is sent as the
/// password with `username` (defaults to `git` for token auth on most hosts).
#[derive(Debug, Clone, Default)]
pub struct GitAuth {
    pub username: String,
    pub token: String,
}

/// A one-line summary of a commit, for the history list.
#[derive(Debug, Clone)]
pub struct GitCommit {
    pub short_id: String,
    pub message: String,
    pub author: String,
    /// Unix seconds.
    pub time: i64,
}

/// Repository status snapshot.
#[derive(Debug, Clone, Default)]
pub struct GitStatus {
    pub branch: Option<String>,
    pub dirty: usize,
    /// Commits on the local branch not on its upstream. Best-effort.
    pub ahead: usize,
}

// ---------------------------------------------------------------------------
// Repository open / init
// ---------------------------------------------------------------------------

/// Return true if `dir` is the worktree of a git repository.
pub fn is_repo(dir: &Path) -> bool {
    dir.join(".git").exists()
}

/// Open the repository at `dir`, returning an error if it isn't a repo.
pub fn open(dir: &Path) -> Result<gix::Repository> {
    let repo = gix::open(dir).with_context(|| format!("open git repo at {:?}", dir))?;
    Ok(repo)
}

/// Ensure a repository exists at `dir`, initialising one if needed, and that
/// the default identity is configured. Returns the opened repo.
pub fn ensure_repo(dir: &Path) -> Result<gix::Repository> {
    if !is_repo(dir) {
        log::info!("git: initialising new repository at {:?}", dir);
        let _ = gix::init(dir)?;
    }
    let mut repo = open(dir)?;
    ensure_identity(&mut repo, dir)?;
    Ok(repo)
}

/// Write a default `user.name` / `user.email` into `.git/config` if missing.
/// Done via plain-text append to avoid the typed config-key API surface.
fn ensure_identity(repo: &mut gix::Repository, dir: &Path) -> Result<()> {
    let cfg_path = dir.join(".git").join("config");
    let existing = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    let snapshot = repo.config_snapshot();
    let has_name = snapshot.string("user.name").is_some();
    let has_email = snapshot.string("user.email").is_some();
    let _ = snapshot;
    if has_name && has_email {
        return Ok(());
    }
    let mut content = existing;
    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    if !has_name || !has_email {
        content.push_str("\n[user]\n");
        if !has_name {
            content.push_str("\tname = Verve\n");
        }
        if !has_email {
            content.push_str("\temail = verve@local\n");
        }
    }
    std::fs::write(&cfg_path, content)
        .with_context(|| format!("write git config {:?}", cfg_path))?;
    *repo = open(dir)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Snapshot the current branch + the number of uncommitted changes.
///
/// Dirty count: we stage a fresh tree from the worktree contents and diff it
/// against `HEAD^{tree}`. For the single-file workspace this is fast and exact.
pub fn status(repo: &gix::Repository) -> Result<GitStatus> {
    let branch = current_branch(repo).ok();

    let worktree_tree_id = stage_worktree_tree(repo)?;
    let head_tree_id = repo
        .head_tree_id()
        .ok()
        .map(|id| id.detach())
        .unwrap_or_else(|| gix::hash::ObjectId::empty_tree(repo.object_hash()));

    let dirty = if worktree_tree_id == head_tree_id {
        0
    } else {
        let old = repo.find_tree(head_tree_id).ok();
        let new = repo.find_tree(worktree_tree_id).ok();
        match repo.diff_tree_to_tree(old.as_ref(), new.as_ref(), None) {
            Ok(changes) => changes.len(),
            Err(e) => {
                log::warn!("git: diff_tree_to_tree failed: {e:?}");
                1
            }
        }
    };

    // ahead = commits on local HEAD not on origin/<branch>. Best-effort: 0
    // before the first push (no upstream yet) or on any error.
    let ahead = count_ahead(repo, branch.as_deref()).unwrap_or(0);

    Ok(GitStatus {
        branch,
        dirty,
        ahead,
    })
}

/// Count local commits ahead of `origin/<branch>`. Returns 0 if the remote
/// tracking ref doesn't exist (e.g. before the first push).
fn count_ahead(repo: &gix::Repository, branch: Option<&str>) -> Result<usize> {
    let branch = match branch {
        Some(b) => b,
        None => return Ok(0),
    };
    // Resolve local branch HEAD via its ref, not rev-parse (that needs a feature).
    let local_id = match repo.try_find_reference(format!("refs/heads/{branch}").as_str())? {
        Some(r) => r.id().detach(),
        None => return Ok(0),
    };
    // origin/<branch> tracking ref may not exist yet (before first push).
    let remote_ref = format!("refs/remotes/origin/{branch}");
    let remote_id = match repo.try_find_reference(remote_ref.as_str())? {
        Some(r) => Some(r.id().detach()),
        None => None,
    };
    match remote_id {
        None => Ok(0),
        Some(remote_id) => {
            // Walk first-parent from local, stop when we reach remote.
            let mut count = 0;
            let mut cursor = local_id;
            while cursor != remote_id {
                let commit = match repo.find_commit(cursor) {
                    Ok(c) => c,
                    Err(_) => break,
                };
                let next = match commit.parent_ids().next() {
                    Some(p) => p.detach(),
                    None => break,
                };
                count += 1;
                cursor = next;
                if count > 100_000 {
                    break;
                }
            }
            Ok(count)
        }
    }
}

/// Current branch short name (e.g. `main`). Returns `Err` on detached HEAD or
/// an unborn branch.
pub fn current_branch(repo: &gix::Repository) -> Result<String> {
    let name = repo.head_name()?.ok_or_else(|| anyhow!("detached HEAD"))?;
    let bytes = name.as_bstr();
    let short = bytes
        .strip_prefix(b"refs/heads/")
        .map(|s| s.to_vec())
        .unwrap_or_else(|| bytes.to_vec());
    Ok(String::from_utf8_lossy(&short).into_owned())
}

/// Write every file under the repo workdir into the object db and produce a
/// tree id representing the current worktree state. Untracked files (like
/// `workspace.json` on first commit) are included.
fn stage_worktree_tree(repo: &gix::Repository) -> Result<gix::hash::ObjectId> {
    let workdir: PathBuf = repo
        .work_dir()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow!("bare repo has no workdir"))?;
    let head_tree_id = repo
        .head_tree_id()
        .ok()
        .map(|id| id.detach())
        .unwrap_or_else(|| gix::hash::ObjectId::empty_tree(repo.object_hash()));
    log::debug!("stage_worktree_tree: head_tree={}", head_tree_id);
    let mut editor = repo.edit_tree(head_tree_id)?;

    // Collect relative paths first so we can mutate the editor while iterating.
    let mut paths: Vec<PathBuf> = Vec::new();
    collect_files(&workdir, &workdir, &mut paths)?;
    log::debug!("stage_worktree_tree: 收集到 {} 个文件", paths.len());
    for rel_path in &paths {
        let abs = workdir.join(rel_path);
        let bytes = match std::fs::read(&abs) {
            Ok(b) => b,
            Err(e) => {
                log::warn!("git: skip {:?}: {e}", rel_path);
                continue;
            }
        };
        if let Ok(blob_id) = repo.write_blob(&bytes) {
            let mode = gix::object::tree::EntryKind::Blob;
            // Tree paths must use '/' separators. On Windows `to_string_lossy`
            // yields '\', which gix_validate rejects as an invalid filename
            // when the tree is written (commit then fails on every sync).
            let rel_str = rel_path.to_string_lossy().replace('\\', "/");
            if let Err(e) = editor.upsert(rel_str.as_str(), mode, blob_id.detach()) {
                log::warn!("git: upsert {:?} failed: {e}", rel_path);
            }
        }
    }

    // Remove global/machine config files from the tree if they were previously
    // committed (so future checkouts don't restore stale versions).
    for untrack in &[
        "workspaces.json",
        "layout.local.json",
        "ssh_hosts.json",
        "shares.json",
        "docker_hosts.json",
        "docker_active.json",
        "kube_config.yaml",
        "kube_clusters.json",
        "hosts.staging",
        ".bootstrap_done",
    ] {
        let _ = editor.remove(*untrack);
    }

    // editor.write() needs nothing — it writes back through the repo's object db.
    let tree_id = editor.write()?.detach();
    log::debug!("stage_worktree_tree: 完成 → {}", tree_id);
    Ok(tree_id)
}

/// Recursively collect file paths (relative to `root`) under `dir`, skipping
/// `.git` and cross-branch / non-data / machine-local files. These must NOT
/// enter per-workspace commits because they are shared across branches or
/// machine-specific.
/// layout.json IS now tracked (shared settings); layout.local.json is machine-specific and skipped.
fn collect_files(root: &Path, dir: &Path, out: &mut Vec<PathBuf>) -> Result<()> {
    /// Files that are cross-branch infrastructure or machine-local, not per-workspace data.
    /// These are global/machine config files that must NOT be overwritten when
    /// switching workspace branches (git checkout -f).
    const SKIP_FILES: &[&str] = &[
        "workspaces.json",
        "layout.local.json",
        "ssh_hosts.json",
        "shares.json",
        "docker_hosts.json",
        "docker_active.json",
        "kube_config.yaml",
        "kube_clusters.json",
        ".verve-askpass.sh",
        ".gitignore",
        "hosts.staging",
        ".bootstrap_done",
        // Tantivy search-index runtime lock — held by the app while running,
        // so reading it fails (os error 33) and it must never be committed.
        "tantivy-writer.lock",
    ];
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == ".git" || name == "exports" {
            continue;
        }
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_files(root, &path, out)?;
        } else if ft.is_file() {
            if SKIP_FILES.contains(&name.as_ref()) {
                continue;
            }
            let rel = path
                .strip_prefix(root)
                .map(|p| p.to_path_buf())
                .unwrap_or(path);
            out.push(rel);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Commit
// ---------------------------------------------------------------------------

/// Commit the current worktree state to the active branch. Returns the new
/// commit's short id, or `None` if there were no changes to commit.
pub fn commit(repo: &gix::Repository, message: &str) -> Result<Option<String>> {
    log::debug!("commit: stage_worktree_tree...");
    let tree_id = stage_worktree_tree(repo)?;
    let head_tree_id = repo
        .head_tree_id()
        .ok()
        .map(|id| id.detach())
        .unwrap_or_else(|| gix::hash::ObjectId::empty_tree(repo.object_hash()));
    if tree_id == head_tree_id {
        log::debug!("commit: 无变更（tree 相同），返回 None");
        return Ok(None);
    }

    // Parent = current HEAD (if any). On first commit there is no HEAD yet.
    let parents: Vec<gix::hash::ObjectId> = match repo.head_id() {
        Ok(id) => vec![id.detach()],
        Err(_) => vec![],
    };
    log::debug!(
        "commit: tree={} head={} parents={}",
        tree_id,
        head_tree_id,
        parents.len()
    );

    let head_name = repo
        .head_name()?
        .map(|n| {
            let bytes = n.as_bstr();
            String::from_utf8_lossy(bytes).into_owned()
        })
        .unwrap_or_else(|| "refs/heads/main".to_string());

    let commit_id = repo.commit(
        head_name.as_str(),
        message,
        tree_id,
        parents.iter().copied(),
    )?;
    let detached = commit_id.detach();
    log::debug!("commit: 完成 → {}", detached);
    Ok(Some(detached.to_string()))
}

// ---------------------------------------------------------------------------
// Log
// ---------------------------------------------------------------------------

/// Return the last `limit` commits reachable from HEAD, newest first.
pub fn log(repo: &gix::Repository, limit: usize) -> Result<Vec<GitCommit>> {
    let head_id = match repo.head_id() {
        Ok(id) => id.detach(),
        Err(_) => return Ok(Vec::new()),
    };
    let walk = repo.rev_walk(Some(head_id)).first_parent_only().all()?;
    let mut out = Vec::new();
    for info in walk {
        let info = info?;
        if out.len() >= limit {
            break;
        }
        // info.id is already a detached ObjectId.
        let commit = match repo.find_commit(info.id) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let message = commit
            .message()
            .ok()
            .map(|m| String::from_utf8_lossy(m.title.as_ref()).into_owned())
            .unwrap_or_default();
        let author = commit
            .author()
            .ok()
            .map(|a| String::from_utf8_lossy(a.name.as_ref()).into_owned())
            .unwrap_or_default();
        let time = commit.time().map(|t| t.seconds).unwrap_or(0);
        out.push(GitCommit {
            short_id: short_sha(&commit.id().detach().to_string()),
            message,
            author,
            time,
        });
    }
    Ok(out)
}

/// Shorten a 40/64-char hex sha to the conventional 7 chars.
fn short_sha(full: &str) -> String {
    let len = full.len().min(7);
    full[..len].to_string()
}

// ---------------------------------------------------------------------------
// Branches
// ---------------------------------------------------------------------------

/// List local branch names (short form, without the `refs/heads/` prefix).
pub fn branches(repo: &gix::Repository) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let refs = repo.references()?;
    let iter = refs.local_branches()?;
    for r in iter {
        // The iterator yields Result<Reference, Box<dyn Error>>.
        let r = r.map_err(|e| anyhow!(e.to_string()))?;
        let bytes = r.name().as_bstr();
        let short = bytes
            .strip_prefix(b"refs/heads/")
            .map(|s| s.to_vec())
            .unwrap_or_else(|| bytes.to_vec());
        out.push(String::from_utf8_lossy(&short).into_owned());
    }
    out.sort();
    Ok(out)
}

/// Create a new branch pointing at HEAD and switch to it.
pub fn create_branch(repo: &gix::Repository, name: &str) -> Result<()> {
    let head_id = repo
        .head_id()
        .map_err(|_| anyhow!("no commit to branch from — commit first"))?;
    let full = format!("refs/heads/{name}");
    repo.reference(
        full.as_str(),
        head_id.detach(),
        gix::refs::transaction::PreviousValue::Any,
        format!("branch: created {name}"),
    )?;
    set_head(repo, name)?;
    Ok(())
}

/// Switch to the local branch `name`, rewriting the worktree files to match.
/// Uses the git CLI because gix's `set_head` only rewrites `.git/HEAD` — it
/// does NOT update `workspace.json` on disk, which breaks the per-workspace
/// branch model. `git checkout -f` correctly swaps the worktree content and
/// forces overwrite of untracked files (safe here because callers commit first).
pub fn checkout(repo: &gix::Repository, name: &str, auth: &GitAuth) -> Result<()> {
    let full = format!("refs/heads/{name}");
    if repo.try_find_reference(full.as_str())?.is_none() {
        bail!("分支 {name} 不存在");
    }
    let workdir = repo
        .work_dir()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow!("bare repo has no workdir"))?;
    run_git(&workdir, auth, &["checkout", "-f", name])?;
    Ok(())
}

/// Create the branch if it doesn't exist, then check it out. Returns `true`
/// when the branch was newly created, `false` when it already existed (and was
/// just checked out). Used by workspace switching.
pub fn create_or_checkout(repo: &gix::Repository, name: &str, auth: &GitAuth) -> Result<bool> {
    let full = format!("refs/heads/{name}");
    let exists = repo.try_find_reference(full.as_str())?.is_some();
    if exists {
        checkout(repo, name, auth)?;
        Ok(false)
    } else {
        create_branch(repo, name)?;
        // create_branch uses set_head (symbolic ref), but we must also rewrite
        // the worktree — a fresh branch from HEAD has identical content so the
        // checkout is a no-op, but run it for consistency.
        let workdir = repo
            .work_dir()
            .map(|p| p.to_path_buf())
            .ok_or_else(|| anyhow!("bare repo has no workdir"))?;
        run_git(&workdir, auth, &["checkout", "-f", name])?;
        Ok(true)
    }
}

fn set_head(repo: &gix::Repository, name: &str) -> Result<()> {
    let full = format!("refs/heads/{name}");
    // Point HEAD at the branch symbolically by editing `.git/HEAD` directly —
    // simpler than going through gix's reference-transaction API for this.
    let git_dir = repo.git_dir();
    std::fs::write(git_dir.join("HEAD"), format!("ref: {full}\n"))
        .with_context(|| format!("write HEAD -> {full}"))?;
    Ok(())
}

/// Delete a local branch via the git CLI (`git branch -D`). The caller MUST
/// move off the branch first (git refuses to delete the checked-out branch).
/// Refuses to delete the last remaining branch.
pub fn delete_branch(repo: &gix::Repository, name: &str, auth: &GitAuth) -> Result<()> {
    let full = format!("refs/heads/{name}");
    if repo.try_find_reference(full.as_str())?.is_none() {
        bail!("分支 {name} 不存在");
    }
    let all = branches(repo)?;
    if all.len() <= 1 {
        bail!("不能删除唯一分支");
    }
    // Don't allow deleting the checked-out branch — caller must switch first.
    let cur = current_branch(repo).ok();
    if cur.as_deref() == Some(name) {
        bail!("不能删除当前所在分支，请先切换到其他分支");
    }
    let workdir = repo
        .work_dir()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow!("bare repo has no workdir"))?;
    // -D = force delete (equivalent to -d -f); safe because we checked above.
    run_git(&workdir, auth, &["branch", "-D", name])?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Remote
// ---------------------------------------------------------------------------

/// Return the configured `origin` push/fetch URL, if any.
pub fn get_remote(repo: &gix::Repository) -> Option<String> {
    let snap = repo.config_snapshot();
    snap.string("remote.origin.url").map(|s| s.to_string())
}

/// Set (or replace) the `origin` remote URL in `.git/config`.
pub fn set_remote(_repo: &gix::Repository, dir: &Path, url: &str) -> Result<()> {
    let cfg_path = dir.join(".git").join("config");
    let mut content = std::fs::read_to_string(&cfg_path).unwrap_or_default();
    // Remove any existing [remote "origin"] section.
    let mut out = String::new();
    let mut in_section = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            in_section = trimmed == "[remote \"origin\"]";
        }
        if !in_section {
            out.push_str(line);
            out.push('\n');
        }
    }
    content = out;
    if !content.ends_with('\n') && !content.is_empty() {
        content.push('\n');
    }
    content.push_str(&format!(
        "[remote \"origin\"]\n\turl = {url}\n\tfetch = +refs/heads/*:refs/remotes/origin/*\n"
    ));
    std::fs::write(&cfg_path, content)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Network: fetch (pure gix, authenticated) — updates refs/remotes/origin/*
// only; never moves the local branch or the worktree. Used to read remote
// status. Pull/push go through the git CLI below.
// ---------------------------------------------------------------------------

/// Fetch from `origin` using the user's token for HTTPS Basic auth. Updates
/// `refs/remotes/origin/*` only — does NOT fast-forward the local branch or
/// touch the worktree (gix has no high-level pull). Returns a human summary.
pub fn fetch(repo: &gix::Repository, auth: &GitAuth) -> Result<String> {
    let url = get_remote(repo).ok_or_else(|| anyhow!("未配置 origin 远程仓库"))?;
    log::info!("git fetch (gix): {}", mask_url(&url));
    let remote = repo.remote_at(url)?;
    let username = auth.username.clone();
    let token = auth.token.clone();
    if token.is_empty() {
        log::warn!("git fetch: token 为空，gix 可能因无凭据而挂起");
    }
    // Attach a credential provider so gix's HTTP transport can authenticate.
    let connection = remote
        .connect(gix::remote::Direction::Fetch)?
        .with_credentials(move |action| {
            use gix::credentials::helper::Action;
            match action {
                Action::Get(ctx) => {
                    log::info!("git fetch: 凭据请求 → 提供已配置的 token");
                    let user = if username.is_empty() {
                        ctx.username.clone().unwrap_or_default()
                    } else {
                        username.clone()
                    };
                    Ok(Some(gix::credentials::protocol::Outcome {
                        identity: gix::sec::identity::Account {
                            username: user,
                            password: token.clone(),
                        },
                        next: ctx.into(),
                    }))
                }
                // We never persist/erase credentials — this provider is stateless.
                _ => Ok(None),
            }
        });
    log::info!("git fetch: prepare_fetch...");
    let prepare = connection.prepare_fetch(gix::progress::Discard, Default::default())?;
    log::info!("git fetch: receive...");
    let outcome = prepare.receive(
        gix::progress::Discard,
        &std::sync::atomic::AtomicBool::new(false),
    )?;
    let changed = match &outcome.status {
        gix::remote::fetch::Status::NoPackReceived { .. } => "无新内容".to_string(),
        gix::remote::fetch::Status::Change { .. } => "有新提交".to_string(),
    };
    log::info!("git fetch: 完成 — {changed}");
    Ok(format!("拉取远程：{changed}"))
}

/// Mask the userinfo in a URL for logging: `https://user:token@host/...` →
/// `https://***@host/...`. Falls back to the scheme+host if parsing fails.
fn mask_url(url: &str) -> String {
    match url.split_once("://") {
        Some((scheme, rest)) => {
            let host_part = match rest.split_once('@') {
                Some((_, host)) => host,
                None => rest,
            };
            format!("{scheme}://***@{host_part}")
        }
        None => url.to_string(),
    }
}

// ---------------------------------------------------------------------------
// Network: pull (system git, authenticated) — fetch + fast-forward the local
// branch and rewrite the worktree. Requires upstream (set by the first push).
// ---------------------------------------------------------------------------

/// Pull from origin with a fast-forward-only merge. Updates the local branch
/// and rewrites `workspace.json` on disk. Always commit before calling this
/// (the worktree must be clean); `--ff-only` never produces a merge commit and
/// aborts cleanly on real divergence.
pub fn pull(repo: &gix::Repository, auth: &GitAuth) -> Result<String> {
    let workdir = repo
        .work_dir()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow!("bare repo has no workdir"))?;
    // --ff-only: fast-forward if possible, otherwise fail (no merge commit).
    // If upstream isn't set yet, this errors with a clear message — callers
    // should push first to establish tracking.
    run_git(&workdir, auth, &["pull", "--ff-only", "--no-edit"])
}

// ---------------------------------------------------------------------------
// Network: push (system git, authenticated) — sends local commits to origin.
// `git push -u origin HEAD` both creates the remote branch on first push AND
// sets upstream tracking; subsequent pushes just update it.
// ---------------------------------------------------------------------------

/// Push the current branch (`HEAD`) to `origin`, setting upstream tracking on
/// the first push. Works for both "first push to an empty remote" and
/// subsequent updates — one command handles both.
pub fn push(repo: &gix::Repository, auth: &GitAuth) -> Result<String> {
    let workdir = repo
        .work_dir()
        .map(|p| p.to_path_buf())
        .ok_or_else(|| anyhow!("bare repo has no workdir"))?;
    let out = run_git(&workdir, auth, &["push", "-u", "origin", "HEAD"])?;
    Ok(if out.is_empty() {
        "已推送到 origin".to_string()
    } else {
        out
    })
}

// ---------------------------------------------------------------------------
// Sync — the single high-level entry point used by the sync button.
// commit (if dirty) → pull (if upstream) → push. Idempotent and safe to retry.
// ---------------------------------------------------------------------------

/// Full sync: commit any dirty worktree, then exchange with the remote.
///
/// Order, for the "首次建立仓库点同步" scenario and steady-state:
/// 1. `ensure_repo` (init if needed).
/// 2. If dirty, commit with `message` (skipped when nothing changed).
/// 3. If a remote is configured:
///    - If upstream tracking already exists → `pull --ff-only` (merge remote
///      changes into the local branch + worktree), then push.
///    - If no upstream yet (first sync) → `push -u` (creates the remote branch
///      and sets tracking); pull is skipped since there's nothing to merge.
/// 4. Returns the step-by-step log (joined with " · ") for the UI banner.
pub fn sync(dir: &Path, auth: &GitAuth, message: &str) -> Result<Vec<String>> {
    log::info!(
        "git sync 开始 (dir={:?}, user={}, token={})",
        dir,
        auth.username,
        if auth.token.is_empty() {
            "EMPTY"
        } else {
            "set"
        }
    );
    let mut logs = Vec::new();
    let repo = ensure_repo(dir)?;
    log::info!("git sync: ensure_repo 完成");
    let st = status(&repo)?;
    log::info!(
        "git sync: status = branch={:?} dirty={} ahead={}",
        st.branch,
        st.dirty,
        st.ahead
    );
    if st.dirty > 0 {
        log::info!("git sync: 有改动，提交中...");
        match commit(&repo, message)? {
            Some(id) => {
                log::info!("git sync: 已提交 {id}");
                logs.push(format!("已提交 {id}"));
            }
            None => {
                log::info!("git sync: commit 返回 None（无变更）");
                logs.push("无变更可提交".to_string());
            }
        }
    } else {
        log::info!("git sync: 无改动，跳过提交");
    }
    let remote = get_remote(&repo);
    if remote.is_none() {
        log::warn!("git sync: 未配置远程，跳过推送");
        logs.push("未配置远程，跳过推送".to_string());
        return Ok(logs);
    }
    log::info!(
        "git sync: 远程 = {}",
        mask_url(remote.as_deref().unwrap_or(""))
    );
    if has_upstream(&repo)? {
        log::info!("git sync: 有 upstream → 先 pull --ff-only");
        // Steady-state: merge remote first, then push our (possibly new) commits.
        match pull(&repo, auth) {
            Ok(msg) => {
                log::info!("git sync: pull 成功 — {msg}");
                logs.push(if msg.is_empty() {
                    "已是最新".to_string()
                } else {
                    msg
                });
            }
            Err(e) => {
                log::warn!("git sync: pull 失败 — {e}");
                logs.push(format!("拉取跳过：{e}"));
            }
        }
    } else {
        log::info!("git sync: 无 upstream（首次推送）→ 跳过 pull，直接 push -u");
    }
    log::info!("git sync: 推送中...");
    match push(&repo, auth) {
        Ok(msg) => {
            log::info!("git sync: push 成功 — {msg}");
            logs.push(if msg.is_empty() {
                "已推送".to_string()
            } else {
                msg
            });
        }
        Err(e) => {
            log::error!("git sync: push 失败 — {e}");
            logs.push(format!("推送失败：{e}"));
        }
    }
    log::info!("git sync 完成: {:?}", logs);
    Ok(logs)
}

/// Whether the remote tracking ref `refs/remotes/origin/<branch>` exists —
/// i.e. we've pushed at least once and `pull` will have something to merge.
/// Checking only the config (`branch.<name>.remote`) is insufficient: that
/// gets set by `git push -u` even when the fetch of origin/main hasn't
/// happened yet, causing `pull` to fail with "no such ref was fetched".
fn has_upstream(repo: &gix::Repository) -> Result<bool> {
    let branch = match current_branch(repo).ok() {
        Some(b) => b,
        None => return Ok(false),
    };
    let ref_name = format!("refs/remotes/origin/{branch}");
    Ok(repo.try_find_reference(ref_name.as_str())?.is_some())
}

// ---------------------------------------------------------------------------
// Clone (first-run bootstrap)
// ---------------------------------------------------------------------------

/// Clone a remote repository into `dir` using the system git CLI.
/// `dir` must NOT already exist. Uses GIT_ASKPASS for credential handling.
pub fn clone(dir: &Path, url: &str, auth: &GitAuth) -> Result<()> {
    if dir.exists() {
        bail!("target directory {:?} already exists", dir);
    }
    let parent = dir.parent().context("clone target has no parent")?;
    let name = dir.file_name().context("invalid clone target")?;
    let out = run_git(parent, auth, &["clone", url, &name.to_string_lossy()])?;
    log::info!("git clone: {}", out);
    Ok(())
}

// ---------------------------------------------------------------------------
// Conflict detection and resolution
// ---------------------------------------------------------------------------

/// Check if the working tree at `dir` has unresolved merge conflicts.
pub fn has_conflict(dir: &Path) -> Result<bool> {
    // git diff --name-only --diff-filter=U lists unmerged files.
    let output = std::process::Command::new("git")
        .current_dir(dir)
        .args(["diff", "--name-only", "--diff-filter=U"])
        .output()
        .context("git diff failed")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(!stdout.trim().is_empty())
}

/// Abort an in-progress merge, returning to the pre-merge state.
pub fn abort_merge(dir: &Path, auth: &GitAuth) -> Result<()> {
    run_git(dir, auth, &["merge", "--abort"])?;
    Ok(())
}

/// Resolve conflicts by keeping our (local) version of all conflicted files,
/// then stage them.
pub fn resolve_keep_ours(dir: &Path, auth: &GitAuth) -> Result<()> {
    run_git(dir, auth, &["checkout", "--ours", "."])?;
    run_git(dir, auth, &["add", "-A"])?;
    Ok(())
}

/// Resolve conflicts by keeping their (remote) version of all conflicted files,
/// then stage them.
pub fn resolve_keep_theirs(dir: &Path, auth: &GitAuth) -> Result<()> {
    run_git(dir, auth, &["checkout", "--theirs", "."])?;
    run_git(dir, auth, &["add", "-A"])?;
    Ok(())
}
