//! Integration test for the pure-gix git ops wrapper.
//! Runs against a temp directory; no network.

use std::fs;
use verve::git::ops;

#[test]
fn init_commit_log_branch_roundtrip() {
    let dir = std::env::temp_dir().join(format!("verve-gix-test-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();

    // ensure_repo initialises + sets identity.
    let repo = ops::ensure_repo(&dir).expect("ensure_repo");
    assert!(ops::is_repo(&dir));

    // Empty repo: status dirty=0 (nothing tracked yet, HEAD unborn).
    let st = ops::status(&repo).expect("status");
    assert_eq!(st.dirty, 0, "empty repo should report 0 dirty");

    // Write the workspace file.
    fs::write(dir.join("workspace.json"), r#"{"version":1}"#).unwrap();

    // Now dirty=1 (untracked file).
    let st = ops::status(&repo).expect("status");
    assert_eq!(st.dirty, 1, "untracked file should be dirty");

    // Commit it.
    let id = ops::commit(&repo, "initial commit").expect("commit");
    assert!(id.is_some(), "first commit should produce an id");

    // After commit, dirty=0.
    let st = ops::status(&repo).expect("status");
    assert_eq!(st.dirty, 0, "after commit dirty should be 0");

    // Log has one entry.
    let log = ops::log(&repo, 10).expect("log");
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].message, "initial commit");

    // Modify and re-commit.
    fs::write(dir.join("workspace.json"), r#"{"version":2}"#).unwrap();
    let id2 = ops::commit(&repo, "second").expect("commit");
    assert!(id2.is_some());
    let log = ops::log(&repo, 10).expect("log");
    assert_eq!(log.len(), 2);
    assert_eq!(log[0].message, "second");

    // Branch list + create + checkout.
    let initial_branch = ops::current_branch(&repo).unwrap();
    let branches = ops::branches(&repo).unwrap();
    assert!(branches.contains(&initial_branch));

    ops::create_branch(&repo, "feature").expect("create_branch");
    assert_eq!(ops::current_branch(&repo).unwrap(), "feature");

    // checkout now requires auth (it shells out to git CLI); use empty auth
    // since these tests never touch a remote.
    let no_auth = ops::GitAuth::default();
    ops::checkout(&repo, &initial_branch, &no_auth).expect("checkout back");
    assert_eq!(ops::current_branch(&repo).unwrap(), initial_branch);

    // Cleanup.
    drop(repo);
    let _ = fs::remove_dir_all(&dir);
}

#[test]
fn sync_workflow_commits_dirty_changes() {
    // Mirrors GitState::sync_async's ops sequence (minus network).
    let dir = std::env::temp_dir().join(format!("verve-gix-sync-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let repo = ops::ensure_repo(&dir).unwrap();

    // Initially clean (nothing tracked).
    let st = ops::status(&repo).unwrap();
    assert_eq!(st.dirty, 0);

    // Simulate a workspace write (what AppState::persist does).
    std::fs::write(dir.join("workspace.json"), r#"{"v":1}"#).unwrap();

    // sync_async's logic: dirty > 0 → commit.
    let st = ops::status(&repo).unwrap();
    assert_eq!(st.dirty, 1);
    let id = ops::commit(&repo, "auto-save").unwrap();
    assert!(id.is_some(), "sync should commit dirty changes");

    // After commit, clean again.
    let st = ops::status(&repo).unwrap();
    assert_eq!(st.dirty, 0);

    // A no-op sync (clean) commits nothing.
    let id = ops::commit(&repo, "nothing").unwrap();
    assert!(id.is_none(), "clean sync should not commit");

    // Log reflects the single real commit.
    let log = ops::log(&repo, 10).unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].message, "auto-save");

    drop(repo);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ensure_askpass_writes_executable_helper() {
    let dir = std::env::temp_dir().join(format!("verve-askpass-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // First call creates the helper.
    let path = ops::ensure_askpass(&dir).expect("ensure_askpass");
    assert!(
        path.exists(),
        "askpass helper should exist after first call"
    );
    let body = std::fs::read_to_string(&path).unwrap();
    assert!(
        body.contains("VERVE_GIT_USER") && body.contains("VERVE_GIT_TOKEN"),
        "helper must read credentials from env, not hardcode them"
    );
    // No secret in the file.
    assert!(
        !body.contains("ghp_") && !body.contains("token"),
        "helper must not contain any literal token"
    );

    // Second call is a no-op when the content matches.
    let path2 = ops::ensure_askpass(&dir).expect("ensure_askpass second");
    assert_eq!(path, path2, "idempotent: same path");

    // Unix exec bit set.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert!(mode & 0o100 != 0, "helper should be executable");
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn delete_branch_works_after_switching_off() {
    let dir = std::env::temp_dir().join(format!("verve-delbr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let repo = ops::ensure_repo(&dir).unwrap();
    std::fs::write(dir.join("workspace.json"), r#"{"v":1}"#).unwrap();
    ops::commit(&repo, "init").unwrap();
    let no_auth = ops::GitAuth::default();

    // main exists; create a feature branch and switch to it.
    ops::create_branch(&repo, "feature").unwrap();
    assert_eq!(ops::current_branch(&repo).unwrap(), "feature");

    // delete_branch refuses to delete the checked-out branch — must switch off
    // first (mirrors how the workspace-delete flow switches to default first).
    let err = ops::delete_branch(&repo, "feature", &no_auth);
    assert!(err.is_err(), "should refuse to delete checked-out branch");

    // Switch back to the initial branch, then delete feature.
    // NOTE: the initial branch name is platform-dependent — gix::init uses
    // "main" on some systems but "master" on others (e.g. older Windows git).
    let initial_branch = ops::current_branch(&repo).unwrap_or_else(|_| "main".to_string());
    // We're on "feature" now; read the initial branch from the branch list
    // (it's the one that isn't "feature").
    let branches_before = ops::branches(&repo).unwrap();
    let default_branch = branches_before
        .iter()
        .find(|b| b.as_str() != "feature")
        .cloned()
        .unwrap_or_else(|| "main".to_string());
    ops::checkout(&repo, &default_branch, &no_auth).unwrap();
    ops::delete_branch(&repo, "feature", &no_auth).expect("delete after switching off");
    let branches = ops::branches(&repo).unwrap();
    assert!(
        !branches.contains(&"feature".to_string()),
        "feature should be gone"
    );

    // Refuses to delete the last branch.
    let err = ops::delete_branch(&repo, &default_branch, &no_auth);
    assert!(err.is_err(), "cannot delete the only branch");

    drop(repo);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn count_ahead_is_zero_before_first_push() {
    // `ahead` must be 0 when there's no origin tracking ref yet (first-run),
    // so the branch-manage header doesn't show a misleading "N 待推送".
    let dir = std::env::temp_dir().join(format!("verve-ahead-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let repo = ops::ensure_repo(&dir).unwrap();
    std::fs::write(dir.join("workspace.json"), r#"{"v":1}"#).unwrap();
    ops::commit(&repo, "init").unwrap();

    let st = ops::status(&repo).unwrap();
    assert_eq!(st.ahead, 0, "no remote yet → ahead must be 0");

    drop(repo);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sync_without_remote_commits_and_skips_push() {
    // ops::sync with no remote configured must still commit dirty changes
    // and log that the push was skipped — never hang or error.
    let dir = std::env::temp_dir().join(format!("verve-sync-noremote-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let auth = ops::GitAuth::default();
    std::fs::write(dir.join("workspace.json"), r#"{"v":1}"#).unwrap();

    let logs = ops::sync(&dir, &auth, "auto-save").expect("sync without remote");
    assert!(
        logs.iter().any(|l| l.contains("已提交")),
        "should have committed the dirty file: {logs:?}"
    );
    assert!(
        logs.iter().any(|l| l.contains("未配置远程")),
        "should note the missing remote: {logs:?}"
    );

    // Second sync: clean, no remote → just notes the skip.
    let logs2 = ops::sync(&dir, &auth, "auto-save").expect("sync clean");
    assert!(logs2.iter().any(|l| l.contains("未配置远程")));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn status_on_unborn_branch_does_not_hang() {
    let dir = std::env::temp_dir().join(format!("verve-unborn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    // gix init via ensure_repo → leaves HEAD pointing at an unborn main.
    let repo = ops::ensure_repo(&dir).expect("ensure_repo");

    // Add the workspace file (untracked) — mirrors the real ~/.verve state.
    std::fs::write(dir.join("workspace.json"), r#"{"v":1}"#).unwrap();

    // status() must complete (not hang) and report dirty > 0 for the untracked file.
    let st = ops::status(&repo).expect("status on unborn branch must not hang");
    assert!(
        st.dirty > 0,
        "untracked file on unborn branch should be dirty"
    );

    // And commit() must create the first commit (parents = []).
    let id = ops::commit(&repo, "initial").expect("first commit on unborn branch");
    assert!(id.is_some(), "first commit should produce an id");

    // After commit, the branch is born — status is clean.
    let st2 = ops::status(&repo).expect("status after first commit");
    assert_eq!(st2.dirty, 0);
    // The branch name is platform-dependent ("main" on most systems, but
    // "master" on some Windows git installs). Just assert it's non-empty.
    assert!(
        st2.branch.is_some(),
        "branch should be set after first commit"
    );

    drop(repo);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Test against the REAL ~/.verve directory (the one the user actually hit).
/// Run with: `cargo test real_verve_status_does_not_hang -- --nocapture`
/// This is what reproduces the actual "stuck on 同步中" — a repo that was
/// `git init`-ed by the real git CLI (not gix) and has an unborn main + a
/// configured remote, exactly like the user's setup.
///
/// Marked `#[ignore]` so it doesn't run in CI / mutate the user's real repo
/// during a normal `cargo test`. Run on-demand with `--ignored`.
#[test]
#[ignore]
fn real_verve_status_does_not_hang() {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/Users/lijing".to_string());
    let dir = std::path::PathBuf::from(home).join(".verve");
    if !dir.join(".git").exists() {
        eprintln!("跳过：{:?} 不是 git 仓库", dir);
        return;
    }
    eprintln!("测试真实 ~/.verve 的 status()...");
    let repo = match ops::open(&dir) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("open 失败: {e}");
            return;
        }
    };
    eprintln!("open 成功，调用 status()...");
    match ops::status(&repo) {
        Ok(st) => eprintln!(
            "status 成功: branch={:?} dirty={} ahead={}",
            st.branch, st.dirty, st.ahead
        ),
        Err(e) => eprintln!("status 失败: {e}"),
    }
    eprintln!("remote = {:?}", ops::get_remote(&repo));
}
