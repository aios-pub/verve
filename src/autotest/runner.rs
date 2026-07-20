//! Sequential test-case runner.
//!
//! Given a [`TestCase`] and the containing [`Project`], `run_case` walks the
//! steps in order, maintaining a per-run variable scope that is fed
//! pre-request variables, scripts, and extractors. Progress (step results as
//! they finish) is sent through an `mpsc` channel so the UI can render a live
//! report.
//!
//! Reuses:
//! - `http::prepare` / `http::execute`  for the actual HTTP call
//! - `scripting::run_pre_request` / `run_post_request` for JS execution
//! - `state::models::effective_variables` for scope chaining

use std::collections::BTreeMap;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use http_client::HttpClient;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;

use crate::http;
use crate::scripting;
use crate::state::models::{
    effective_variables, ApiRequest, KeyValue, Project, RequestBody, Response,
};

use super::models::*;

/// Event emitted by the runner as a case executes. The UI subscribes to these
/// via the channel passed to [`run_case`].
#[derive(Debug, Clone)]
pub enum RunEvent {
    /// The case started (payload = case_id).
    Started(String),
    /// A step finished (full step result).
    StepFinished(TestStepResult),
    /// The entire case finished (full run result).
    Finished(TestRunResult),
}

/// Execute a [`TestCase`] sequentially, streaming [`RunEvent`]s to `tx`.
///
/// `http_client` is the shared reqwest client (same one used by the request
/// panel). `override_vars` lets callers seed case-level variables (used by the
/// stress runner to give each worker an isolated scope).
pub async fn run_case(
    case: &TestCase,
    project: &Project,
    http_client: &dyn HttpClient,
    tx: mpsc::UnboundedSender<RunEvent>,
    override_vars: Option<BTreeMap<String, String>>,
) -> Result<TestRunResult> {
    let started = Instant::now();
    let started_at = unix_secs();
    let _ = tx.send(RunEvent::Started(case.id.clone()));

    let env_vars = project
        .active_environment
        .as_ref()
        .and_then(|id| project.environments.iter().find(|e| &e.id == id))
        .map(|e| e.variables.clone())
        .unwrap_or_default();

    // Case-level scope: suite.vars < project.globals < env < case.vars < override.
    // We do not resolve the folder chain here because steps reference a flat
    // request by id; use project.find_request which gives us the folder chain
    // per step.
    let mut scope: BTreeMap<String, String> = BTreeMap::new();
    for kv in &project.global_variables {
        if kv.enabled && !kv.key.trim().is_empty() {
            scope.insert(kv.key.clone(), kv.value.clone());
        }
    }
    for kv in &env_vars {
        if kv.enabled && !kv.key.trim().is_empty() {
            scope.insert(kv.key.clone(), kv.value.clone());
        }
    }
    for kv in &case.variables {
        if kv.enabled && !kv.key.trim().is_empty() {
            scope.insert(kv.key.clone(), kv.value.clone());
        }
    }
    if let Some(ov) = override_vars {
        for (k, v) in ov {
            scope.insert(k, v);
        }
    }

    let iteration = 1u32; // multi-iteration for this runner is 1; stress module wraps it
    let mut step_results: Vec<TestStepResult> = Vec::with_capacity(case.steps.len());
    let mut case_passed = true;

    for step in &case.steps {
        let res = run_step(step, project, http_client, &mut scope).await;
        let hard_fail = !res.passed && !step.continue_on_error;
        case_passed = case_passed && res.passed;
        let _ = tx.send(RunEvent::StepFinished(res.clone()));
        step_results.push(res);
        if hard_fail {
            break;
        }
    }

    let total_ms = started.elapsed().as_millis() as u64;
    let result = TestRunResult {
        case_id: case.id.clone(),
        case_name: case.name.clone(),
        started_at,
        finished_at: unix_secs(),
        total_ms,
        steps: step_results,
        passed: case_passed,
        iteration,
    };
    let _ = tx.send(RunEvent::Finished(result.clone()));
    Ok(result)
}

async fn run_step(
    step: &TestCaseStep,
    project: &Project,
    http_client: &dyn HttpClient,
    scope: &mut BTreeMap<String, String>,
) -> TestStepResult {
    let started = Instant::now();

    // 1. Locate the referenced request.
    let (folder_chain, req) = match project.find_request(&step.request_id) {
        Some(v) => v,
        None => {
            return TestStepResult {
                step_id: step.id.clone(),
                request_name: step
                    .name_override
                    .clone()
                    .unwrap_or_else(|| "<missing request>".into()),
                status: 0,
                passed: false,
                time_ms: 0,
                error: Some(format!("request {} not found", step.request_id)),
                assertions_passed: 0,
                assertions_failed: 0,
                logs: vec![],
                extracted: vec![],
            };
        }
    };
    let display_name = step.name_override.clone().unwrap_or_else(|| req.name.clone());

    // 2. Build the effective variable map for this step by merging folder
    //    variables (walked from root to parent) then the case scope, and the
    //    request's own variables.
    let folder_vars_chain = project.folder_variables_chain(&folder_chain);
    let mut step_scope = scope.clone();
    for kv_slice in &folder_vars_chain {
        for kv in kv_slice.iter() {
            if kv.enabled && !kv.key.trim().is_empty() {
                step_scope.insert(kv.key.clone(), kv.value.clone());
            }
        }
    }
    for kv in &req.variables {
        if kv.enabled && !kv.key.trim().is_empty() {
            step_scope.insert(kv.key.clone(), kv.value.clone());
        }
    }

    // 3. Pre-request script: run request's pre_script (if inheriting) then the
    //    step's override. Collect SetVariable effects into step_scope.
    let mut logs: Vec<String> = Vec::new();
    let mut assertions_passed = 0usize;
    let mut assertions_failed = 0usize;

    let pre_to_run = if step.inherit_request_scripts && !step.pre_script.is_empty() {
        // Request script then step override.
        vec![req.pre_script.as_str(), step.pre_script.as_str()]
    } else if step.inherit_request_scripts {
        vec![req.pre_script.as_str()]
    } else {
        vec![step.pre_script.as_str()]
    };
    for script in pre_to_run {
        if script.trim().is_empty() {
            continue;
        }
        let res = scripting::run_pre_request(script, &step_scope);
        for l in res.logs {
            logs.push(l);
        }
        for eff in res.effects {
            apply_effect(eff, scope, &mut step_scope, &mut logs);
        }
        assertions_passed += res.assertions_passed;
        assertions_failed += res.assertions_failed;
        if let Some(e) = res.error {
            logs.push(format!("pre-script error: {e}"));
        }
    }

    // 4. Prepare + execute the HTTP call.
    let (prepared, prepared_err) = match http::client::prepare(
        req.method,
        &req.url,
        &req.params,
        &req.headers,
        &req.path,
        &req.cookies,
        &req.auth,
        &req.body,
        &step_scope,
        30,
    ) {
        Ok(p) => (Some(p), None),
        Err(e) => (None, Some(format!("prepare: {e}"))),
    };

    let mut response = if let Some(p) = prepared {
        http::client::execute(http_client, p, 30).await
    } else {
        Response {
            status: 0,
            status_text: "Error".into(),
            time_ms: started.elapsed().as_millis() as u64,
            size: 0,
            headers: Vec::new(),
            body: String::new(),
            is_json: false,
            error: prepared_err,
            streaming: false,
        }
    };

    let time_ms = response.time_ms.max(started.elapsed().as_millis() as u64);
    let http_ok = response.error.is_none() && response.status >= 200 && response.status < 500;

    // 5. Post-request / test script.
    let post_to_run = if step.inherit_request_scripts && !step.post_script.is_empty() {
        vec![req.tests_script.as_str(), step.post_script.as_str()]
    } else if step.inherit_request_scripts {
        vec![req.tests_script.as_str()]
    } else {
        vec![step.post_script.as_str()]
    };
    for script in post_to_run {
        if script.trim().is_empty() {
            continue;
        }
        let res = scripting::run_post_request(script, &step_scope, &response);
        for l in res.logs {
            logs.push(l);
        }
        for eff in res.effects {
            apply_effect(eff, scope, &mut step_scope, &mut logs);
        }
        assertions_passed += res.assertions_passed;
        assertions_failed += res.assertions_failed;
        if let Some(e) = res.error {
            logs.push(format!("post-script error: {e}"));
        }
    }

    // 6. Run extractors (applied regardless of pass/fail, but only if HTTP succeeded).
    let mut extracted: Vec<(String, String)> = Vec::new();
    if http_ok {
        for ex in &step.extractors {
            match extract_value(ex, &response) {
                Ok(Some(v)) => {
                    logs.push(format!("extracted {} = {}", ex.name, truncate(&v, 80)));
                    scope.insert(ex.name.clone(), v.clone());
                    extracted.push((ex.name.clone(), v));
                }
                Ok(None) => {
                    logs.push(format!("extractor {}: no match", ex.name));
                }
                Err(e) => {
                    logs.push(format!("extractor {}: {e}", ex.name));
                }
            }
        }
    }

    let passed = http_ok && assertions_failed == 0;

    TestStepResult {
        step_id: step.id.clone(),
        request_name: display_name,
        status: response.status,
        passed,
        time_ms,
        error: response.error.take(),
        assertions_passed,
        assertions_failed,
        logs,
        extracted,
    }
}

/// Apply a script SideEffect to the relevant scope(s). SetVariable on the env
/// scope propagates to the case-level scope (so downstream steps see it).
fn apply_effect(
    eff: scripting::SideEffect,
    case_scope: &mut BTreeMap<String, String>,
    step_scope: &mut BTreeMap<String, String>,
    logs: &mut Vec<String>,
) {
    match eff {
        scripting::SideEffect::SetVariable { name, value, .. } => {
            case_scope.insert(name.clone(), value.clone());
            step_scope.insert(name, value.clone());
        }
        scripting::SideEffect::Log(line) => logs.push(line),
        scripting::SideEffect::Assert { passed, message } => {
            logs.push(format!(
                "{}: {message}",
                if passed { "✓ PASS" } else { "✗ FAIL" }
            ));
        }
    }
}

/// Extract one value out of a response per the extractor config.
fn extract_value(ex: &VariableExtractor, resp: &Response) -> Result<Option<String>> {
    use ExtractSource::*;
    Ok(match ex.source {
        StatusCode => Some(resp.status.to_string()),
        ResponseTime => Some(resp.time_ms.to_string()),
        Body => Some(resp.body.clone()),
        Header => {
            let needle = ex.path.to_ascii_lowercase();
            resp.headers
                .iter()
                .find(|kv| kv.key.to_ascii_lowercase() == needle)
                .map(|kv| kv.value.clone())
        }
        JsonPath => {
            if !resp.is_json || resp.body.trim().is_empty() {
                return Ok(None);
            }
            let v: JsonValue = serde_json::from_str(&resp.body)?;
            walk_json_path(&v, &ex.path).map(|v| json_to_string(&v))
        }
    })
}

/// Walk a simple dot/bracket path (`a.b[0].c`) into a serde_json Value.
/// Returns None if any segment is missing. Tiny hand-written parser — no
/// regex dependency. Full JSONPath (filters, wildcards) can be added later
/// by swapping in jsonpath-rust without changing the extractor API.
fn walk_json_path<'a>(mut v: &'a JsonValue, path: &str) -> Option<&'a JsonValue> {
    let bytes = path.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'.' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b'.' && bytes[i] != b'[' {
                    i += 1;
                }
                if i == start {
                    return None;
                }
                let key = &path[start..i];
                v = v.as_object()?.get(key)?;
            }
            b'[' => {
                i += 1;
                let start = i;
                while i < bytes.len() && bytes[i] != b']' {
                    i += 1;
                }
                if i >= bytes.len() {
                    return None;
                }
                let idx_str = &path[start..i];
                let idx: usize = idx_str.parse().ok()?;
                i += 1; // skip ']'
                v = v.as_array()?.get(idx)?;
            }
            _ => {
                // Bare identifier at start (no leading '.').
                let start = i;
                while i < bytes.len() && bytes[i] != b'.' && bytes[i] != b'[' {
                    i += 1;
                }
                if i == start {
                    return None;
                }
                let key = &path[start..i];
                v = v.as_object()?.get(key)?;
            }
        }
    }
    Some(v)
}

fn json_to_string(v: &JsonValue) -> String {
    match v {
        JsonValue::String(s) => s.clone(),
        JsonValue::Null => String::new(),
        other => other.to_string(),
    }
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n).collect();
        t.push('…');
        t
    }
}

fn unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::autotest::models::{ExtractSource, TestCaseStep, VariableExtractor};
    use crate::state::models::{AuthConfig, RequestBody, RequestMethod};

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// A minimal HTTP/1.1 server that returns canned JSON based on the path.
    /// GET /login → 200 {"data":{"token":"secret123"}}
    /// GET /me    → 200 {"ok":true} (with Authorization: Bearer echo in body)
    fn spawn_fixture_server() -> u16 {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        thread::spawn(move || {
            for stream in listener.incoming() {
                let mut stream = match stream {
                    Ok(s) => s,
                    Err(_) => continue,
                };
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf);
                // Parse first line to get path.
                let Ok(req_str) = std::str::from_utf8(&buf) else { continue };
                let path = req_str
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();

                let auth_hdr = req_str
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|l| l.splitn(2, ':').nth(1).unwrap_or("").trim().to_string())
                    .unwrap_or_default();

                let (status, body) = if path.starts_with("/login") {
                    (200, r#"{"data":{"token":"secret123"}}"#.to_string())
                } else if path.starts_with("/me") {
                    (
                        200,
                        format!(r#"{{"ok":true,"auth":"{auth_hdr}"}}"#),
                    )
                } else {
                    (404, r#"{"error":"not found"}"#.to_string())
                };

                let resp = format!(
                    "HTTP/1.1 {status} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.flush();
            }
        });
        port
    }

    fn build_client() -> std::sync::Arc<dyn HttpClient> {
        let client = reqwest_client::ReqwestClient::user_agent("verve-test/0.1").unwrap();
        std::sync::Arc::new(client)
    }

    #[test]
    fn walk_json_path_basic() {
        let v: JsonValue = serde_json::from_str(r#"{"data":{"token":"abc","arr":[{"x":1}]}}"#).unwrap();
        assert_eq!(
            walk_json_path(&v, "data.token").and_then(|v| v.as_str()),
            Some("abc")
        );
        assert_eq!(
            walk_json_path(&v, "data.arr[0].x").and_then(|v| v.as_i64()),
            Some(1)
        );
        assert!(walk_json_path(&v, "data.missing").is_none());
    }

    #[test]
    fn run_case_extracts_token_and_passes_to_next_step() {
        let port = spawn_fixture_server();
        let client = build_client();

        // Two requests:
        //   POST /login  (returns token in body)
        //   GET  /me     (uses {{token}} in Authorization header)
        let mut login = ApiRequest::new("Login", RequestMethod::Get, format!("http://127.0.0.1:{port}/login"));
        login.tests_script = "apt.assert(response.status === 200);".into();
        let mut me = ApiRequest::new("Me", RequestMethod::Get, format!("http://127.0.0.1:{port}/me"));
        me.headers
            .push(KeyValue::new("Authorization", "Bearer {{token}}"));
        me.tests_script = "apt.assert(response.json.auth === 'Bearer secret123');".into();

        let mut project = Project::new("Test");
        project.requests.push(login.clone());
        project.requests.push(me.clone());

        let mut case = TestCase::new("scenario");
        let mut step1 = TestCaseStep::new(&login.id);
        step1.extractors.push(VariableExtractor {
            name: "token".into(),
            source: ExtractSource::JsonPath,
            path: "data.token".into(),
        });
        case.steps.push(step1);
        case.steps.push(TestCaseStep::new(&me.id));

        let (tx, mut rx) = mpsc::unbounded_channel::<RunEvent>();

        // http::execute uses smol::Timer internally; run on a smol-compatible context.
        let result = smol::block_on(run_case(&case, &project, &*client, tx, None))
            .expect("run_case should return Ok");

        assert!(result.passed, "case should pass; steps: {:?}", result.steps);
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].status, 200);
        assert_eq!(result.steps[0].extracted.len(), 1);
        assert_eq!(result.steps[0].extracted[0].0, "token");
        assert_eq!(result.steps[0].extracted[0].1, "secret123");
        assert_eq!(result.steps[1].status, 200);

        // Channel is dropped at end of scope; the result above already asserts
        // everything we need from the run. No need to drain events here.
    }

    #[test]
    fn run_case_missing_request_fails_step() {
        let client = build_client();
        let project = Project::new("Empty");
        let mut case = TestCase::new("bad");
        case.steps.push(TestCaseStep::new("does-not-exist"));
        let (tx, _rx) = mpsc::unbounded_channel();
        let result = smol::block_on(run_case(&case, &project, &*client, tx, None)).unwrap();
        assert!(!result.passed);
        assert_eq!(result.steps.len(), 1);
        assert!(result.steps[0].error.is_some());
    }
}
