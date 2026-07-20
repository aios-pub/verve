//! Driver for the [`oha`](https://github.com/hatoo/oha) HTTP load generator.
//!
//! oha is invoked as a child process with flags derived from a
//! [`crate::stress::StressConfig`] + [`crate::state::models::ApiRequest`].
//! We capture stdout (JSON via `--no-tui --output-format json`) and parse it
//! into an [`OhaJsonReport`].
//!
//! The oha binary is resolved by [`find_or_download`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

use crate::http;
use crate::state::models::ApiRequest;
use crate::state::models::Project;
use crate::state::persistence::data_dir;

use super::models::OhaJsonReport;

/// The oha release version we target (documented for users installing manually).
pub const OHA_VERSION: &str = "1.4.5";

/// Locate the oha binary. Returns the path if it exists, otherwise an Err with
/// the expected install path. Auto-download is deferred (requires network).
pub fn find_or_download() -> Result<PathBuf, String> {
    if let Ok(dir) = data_dir() {
        let candidate = dir.join("bin").join(oha_exe_name());
        if candidate.exists() {
            return Ok(candidate);
        }
    }
    if let Some(p) = which_in_path(&oha_exe_name()) {
        return Ok(p);
    }
    let dir = data_dir().unwrap_or_else(|_| PathBuf::from("."));
    Err(format!(
        "oha 未找到。请安装 v{OHA_VERSION} (cargo install oha) 并放到 {:?}",
        dir.join("bin")
    ))
}

fn oha_exe_name() -> String {
    if cfg!(target_os = "windows") {
        "oha.exe".into()
    } else {
        "oha".into()
    }
}

/// Run oha against a single resolved request (variable-substituted via
/// `http::prepare`), returning the parsed JSON report.
pub async fn run_oha(
    oha_path: &Path,
    req: &ApiRequest,
    vars: &BTreeMap<String, String>,
    concurrency: u32,
    duration_secs: u32,
    qps: Option<u32>,
) -> Result<OhaJsonReport, String> {
    let prepared = http::client::prepare(
        req.method,
        &req.url,
        &req.params,
        &req.headers,
        &req.path,
        &req.cookies,
        &req.auth,
        &req.body,
        vars,
        60,
    )
    .map_err(|e| format!("prepare: {e}"))?;

    let mut cmd = Command::new(oha_path);
    cmd.arg("--no-tui")
        .arg("--output-format")
        .arg("json")
        .arg("-c")
        .arg(concurrency.to_string())
        .arg("-z")
        .arg(format!("{duration_secs}s"))
        .arg("-m")
        .arg(method_str(prepared.method));

    if let Some(q) = qps {
        cmd.arg("-q").arg(q.to_string());
    }

    for (k, v) in &prepared.headers {
        if k.eq_ignore_ascii_case("content-length") {
            // oha computes Content-Length itself; passing our pre-substituted
            // value can cause 400s if the raw body has {{...}} placeholders.
            continue;
        }
        cmd.arg("-H").arg(format!("{k}: {v}"));
    }
    if !prepared.body.is_empty() {
        cmd.arg("-d")
            .arg(String::from_utf8_lossy(&prepared.body).to_string());
    }
    cmd.arg(&prepared.url);

    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    cmd.kill_on_drop(true);

    let total_timeout = Duration::from_secs((duration_secs as u64).saturating_add(30));
    let child = cmd.spawn().map_err(|e| format!("spawn oha: {e}"))?;
    let output = tokio::time::timeout(total_timeout, child.wait_with_output())
        .await
        .map_err(|_| "oha timed out".to_string())?
        .map_err(|e| format!("oha wait: {e}"))?;

    if !output.status.success() {
        let code = output.status.code().unwrap_or(-1);
        let stderr = String::from_utf8_lossy(&output.stderr).to_string();
        return Err(format!("oha exited {code}: {stderr}"));
    }

    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    serde_json::from_str::<OhaJsonReport>(&stdout).map_err(|e| format!("parse oha JSON: {e}"))
}

/// Convenience: resolve the effective variable map for a request in the project
/// (global < env; folder/request variables are applied per-request by the UI).
pub fn effective_vars(project: &Project) -> BTreeMap<String, String> {
    use crate::state::models::effective_variables;
    effective_variables(
        &project.global_variables,
        project.active_env_variables(),
        &[],
        &[],
    )
}

/// Minimal PATH search (no extra crate dependency).
fn which_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.exists() {
            return Some(candidate);
        }
    }
    None
}

fn method_str(m: crate::state::models::RequestMethod) -> &'static str {
    use crate::state::models::RequestMethod;
    match m {
        RequestMethod::Get => "GET",
        RequestMethod::Post => "POST",
        RequestMethod::Put => "PUT",
        RequestMethod::Delete => "DELETE",
        RequestMethod::Patch => "PATCH",
        RequestMethod::Head => "HEAD",
        RequestMethod::Options => "OPTIONS",
    }
}
