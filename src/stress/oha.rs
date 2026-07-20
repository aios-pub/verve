//! Driver for the [`oha`](https://github.com/hatoo/oha) HTTP load generator.
//!
//! oha is integrated directly as a library, so no external binary is required.
//! We configure oha from a [`crate::stress::StressConfig`] + [`crate::state::models::ApiRequest`],
//! run it in-process, and parse the JSON output into an [`OhaJsonReport`].

use std::collections::BTreeMap;
use std::fs;

use clap::Parser as _;

use crate::http;
use crate::state::models::ApiRequest;
use crate::state::models::Project;

use super::models::OhaJsonReport;

/// The oha version we're integrated with (documented for reference).
pub const OHA_VERSION: &str = "1.15.0";

/// Oha is integrated as a library, so this is a no-op kept for API compatibility.
pub fn find_or_download() -> Result<(), String> {
    Ok(())
}

/// Run oha against a single resolved request (variable-substituted via
/// `http::prepare`), returning the parsed JSON report.
pub async fn run_oha(
    _oha_path: &(),
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

    // Create a temporary file for oha to write JSON output to
    let mut temp_file = std::env::temp_dir();
    temp_file.push(format!("verve-oha-{}.json", uuid::Uuid::new_v4()));
    let temp_path = temp_file.to_string_lossy().to_string();

    // Build oha CLI arguments programmatically
    let mut args = vec![
        "oha".to_string(),
        "--no-tui".to_string(),
        "--output-format=json".to_string(),
        format!("--output={temp_path}"),
        format!("-c={concurrency}"),
        format!("-z={duration_secs}s"),
        format!("-m={}", method_str(prepared.method)),
        // Skip pre-lookup which uses hickory-resolver that can fail on macOS
        "--no-pre-lookup".to_string(),
        // Use IPv4 only to avoid IPv6 routing issues
        "--ipv4".to_string(),
        // Skip TLS certificate verification (useful for internal/test environments)
        "--insecure".to_string(),
        // Longer connect timeout
        "--connect-timeout=10s".to_string(),
    ];

    if let Some(q) = qps {
        args.push(format!("-q={q}"));
    }

    // Add headers (use -H=k:v form to avoid ambiguity if value starts with '-')
    for (k, v) in &prepared.headers {
        if k.eq_ignore_ascii_case("content-length") {
            // oha computes Content-Length itself
            continue;
        }
        args.push(format!("-H={k}: {v}"));
    }

    // Add body if present (use -d=<body> form to avoid ambiguity if body starts with '-')
    if !prepared.body.is_empty() {
        let body_str = String::from_utf8_lossy(&prepared.body).to_string();
        args.push(format!("-d={body_str}"));
    }

    // Add URL as the final positional argument
    args.push("--".to_string());
    args.push(prepared.url.clone());

    // Parse arguments into oha Opts
    let opts = oha::Opts::try_parse_from(args).map_err(|e| format!("parse oha opts: {e}"))?;

    // Run oha
    let result = oha::run(opts).await;

    // Read output file regardless of result
    let json_output = fs::read_to_string(&temp_file).unwrap_or_default();

    // Clean up temp file
    let _ = fs::remove_file(&temp_file);

    // Check for oha runtime error
    result.map_err(|e| {
        // If we got JSON output even with an error, we can still parse it
        if json_output.trim().starts_with('{') {
            // Silently ignore the error if we have valid JSON output
            // (warnings like DNS resolv.conf issues shouldn't fail the run if it completed)
            return String::new();
        }
        format!("oha run: {e}")
    })?;

    // If we got empty error but no JSON, return error
    if json_output.trim().is_empty() {
        return Err("oha produced no output".to_string());
    }

    // Parse JSON output
    serde_json::from_str::<OhaJsonReport>(&json_output)
        .map_err(|e| format!("parse oha JSON: {e}\nstdout: {}", &json_output[..json_output.len().min(500)]))
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
