//! Sequential test-case runner.
//!
//! Given a [`TestCase`] and the containing [`Project`], `run_case` walks the
//! steps in order, maintaining a per-run variable scope that is fed
//! pre-request variables, scripts, and extractors. Progress (step results as
//! they finish) is sent through an `mpsc` channel so the UI can render a live
//! report.
//!
//! Flow control:
//! - `If` steps evaluate a JS condition and run `then_steps` or `else_steps`.
//! - `Loop` steps support repeat-N, for-each-over-JSON-array, and while modes.
//! - `Wait` steps sleep for N ms.
//! - `Script` steps run a standalone JS snippet with no HTTP call.
//! - `Request` steps behave as before.
//!
//! Reuses:
//! - `http::prepare` / `http::execute`  for the actual HTTP call
//! - `scripting::run_pre_request` / `run_post_request` for JS execution
//! - `state::models::effective_variables` for scope chaining

use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;
use http_client::HttpClient;
use serde_json::Value as JsonValue;
use tokio::sync::mpsc;

use crate::http;
use crate::scripting;
use crate::state::models::{
    ApiRequest, KeyValue, Project, RequestBody, Response, effective_variables,
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

    let iterations = case.iteration_count.max(1);
    let mut all_step_results: Vec<TestStepResult> = Vec::new();
    let mut case_passed = true;

    for iter in 0..iterations {
        if iter > 0 {
            let _ = tx.send(RunEvent::StepFinished(TestStepResult {
                step_id: format!("__iter_sep_{iter}"),
                request_name: format!("--- 迭代 {} ---", iter + 1),
                status: 0,
                passed: true,
                time_ms: 0,
                error: None,
                assertions_passed: 0,
                assertions_failed: 0,
                logs: vec![],
                extracted: vec![],
                children: vec![],
            }));
        }

        let mut ctx = RunContext {
            project,
            http_client,
            scope: scope.clone(),
            step_results: Vec::new(),
            case_passed: true,
            hard_failed: false,
        };

        for step in &case.steps {
            if ctx.hard_failed {
                break;
            }
            execute_step(step, &mut ctx).await;
        }

        // Carry scope forward across iterations (so extracted vars persist).
        scope = ctx.scope;
        case_passed = case_passed && ctx.case_passed;
        all_step_results.extend(ctx.step_results);
    }

    let total_ms = started.elapsed().as_millis() as u64;
    let result = TestRunResult {
        case_id: case.id.clone(),
        case_name: case.name.clone(),
        started_at,
        finished_at: unix_secs(),
        total_ms,
        steps: all_step_results,
        passed: case_passed,
        iteration: 1,
    };
    let _ = tx.send(RunEvent::Finished(result.clone()));
    Ok(result)
}

/// Mutable context threaded through step execution (including nested control flow).
struct RunContext<'a> {
    project: &'a Project,
    http_client: &'a dyn HttpClient,
    scope: BTreeMap<String, String>,
    step_results: Vec<TestStepResult>,
    case_passed: bool,
    hard_failed: bool,
}

/// Dispatch a step to its executor and append the result (recursing into
/// if/loop bodies as needed). Uses `Box::pin` for recursive calls.
fn execute_step<'a>(
    step: &'a TestCaseStep,
    ctx: &'a mut RunContext<'_>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        if ctx.hard_failed {
            return;
        }
        match step {
            TestCaseStep::Request(s) => execute_request_step(s, ctx).await,
            TestCaseStep::If(s) => execute_if_step(s, ctx).await,
            TestCaseStep::Loop(s) => execute_loop_step(s, ctx).await,
            TestCaseStep::Wait(s) => execute_wait_step(s, ctx).await,
            TestCaseStep::Script(s) => execute_script_step(s, ctx).await,
        }
    })
}

// ---------------------------------------------------------------------------
// Per-step executors
// ---------------------------------------------------------------------------

async fn execute_request_step(step: &RequestStep, ctx: &mut RunContext<'_>) {
    let started = Instant::now();

    // 1. Locate the referenced request.
    let (folder_chain, req) = match ctx.project.find_request(&step.request_id) {
        Some(v) => v,
        None => {
            let res = TestStepResult {
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
                children: vec![],
            };
            ctx.case_passed = false;
            ctx.hard_failed = !step.continue_on_error;
            ctx.step_results.push(res.clone());
            return;
        }
    };
    let display_name = step
        .name_override
        .clone()
        .unwrap_or_else(|| req.name.clone());

    // 2. Build effective variable map for this step.
    let folder_vars_chain = ctx.project.folder_variables_chain(&folder_chain);
    let mut step_scope = ctx.scope.clone();
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

    // 3. Pre-request script(s).
    let mut logs: Vec<String> = Vec::new();
    let mut assertions_passed = 0usize;
    let mut assertions_failed = 0usize;

    let pre_to_run = if step.inherit_request_scripts && !step.pre_script.is_empty() {
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
            apply_effect(eff, &mut ctx.scope, &mut step_scope, &mut logs);
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
        http::client::execute(ctx.http_client, p, 30).await
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
            apply_effect(eff, &mut ctx.scope, &mut step_scope, &mut logs);
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
                    ctx.scope.insert(ex.name.clone(), v.clone());
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

    let res = TestStepResult {
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
        children: vec![],
    };
    if !passed {
        ctx.case_passed = false;
        ctx.hard_failed = !step.continue_on_error;
    }
    ctx.step_results.push(res);
}

async fn execute_if_step(step: &IfStep, ctx: &mut RunContext<'_>) {
    let started = Instant::now();
    let (truthy, eval_err) = scripting::eval_condition(&step.condition, &ctx.scope);
    let mut logs = Vec::new();
    if let Some(e) = &eval_err {
        logs.push(format!("if condition error: {e}"));
    } else {
        logs.push(format!(
            "条件判定: {} → {}",
            truncate(&step.condition, 60),
            if truthy { "真" } else { "假" }
        ));
    }

    let chosen_branch = if truthy {
        &step.then_steps
    } else {
        &step.else_steps
    };
    let branch_label = if truthy { "then" } else { "else" };

    let mut branch_ctx = RunContext {
        project: ctx.project,
        http_client: ctx.http_client,
        scope: ctx.scope.clone(),
        step_results: Vec::new(),
        case_passed: true,
        hard_failed: false,
    };

    for child in chosen_branch {
        execute_step(child, &mut branch_ctx).await;
    }

    // Propagate scope + pass state upward.
    ctx.scope = branch_ctx.scope;
    let passed = eval_err.is_none() && branch_ctx.case_passed;
    if !passed {
        ctx.case_passed = false;
    }

    let res = TestStepResult {
        step_id: step.id.clone(),
        request_name: format!("如果 ({branch_label})"),
        status: 0,
        passed,
        time_ms: started.elapsed().as_millis() as u64,
        error: eval_err,
        assertions_passed: 0,
        assertions_failed: 0,
        logs,
        extracted: vec![],
        children: branch_ctx.step_results,
    };
    ctx.step_results.push(res);
}

async fn execute_loop_step(step: &LoopStep, ctx: &mut RunContext<'_>) {
    let started = Instant::now();
    let mut logs = Vec::new();
    let mut children: Vec<TestStepResult> = Vec::new();
    let mut loop_passed = true;

    match &step.mode {
        LoopMode::Repeat { count } => {
            logs.push(format!("重复 {} 次", count));
            let n = (*count).min(step.max_iterations);
            for i in 0..n {
                if ctx.hard_failed {
                    break;
                }
                let mut iter_ctx = RunContext {
                    project: ctx.project,
                    http_client: ctx.http_client,
                    scope: ctx.scope.clone(),
                    step_results: Vec::new(),
                    case_passed: true,
                    hard_failed: false,
                };
                // Inject loop counters.
                iter_ctx.scope.insert("$iteration".into(), i.to_string());
                iter_ctx.scope.insert("$index".into(), i.to_string());

                for child in &step.body_steps {
                    execute_step(child, &mut iter_ctx).await;
                }

                ctx.scope = iter_ctx.scope;
                if !iter_ctx.case_passed {
                    loop_passed = false;
                }
                children.extend(iter_ctx.step_results);
            }
        }
        LoopMode::ForEach { var, source } => {
            // Evaluate the source expression; expect a JSON array.
            let arr_val = match scripting::eval_expression(source, &ctx.scope) {
                Ok(v) => v,
                Err(e) => {
                    logs.push(format!("for-each source error: {e}"));
                    let res = TestStepResult {
                        step_id: step.id.clone(),
                        request_name: "遍历循环".into(),
                        status: 0,
                        passed: false,
                        time_ms: started.elapsed().as_millis() as u64,
                        error: Some(e),
                        assertions_passed: 0,
                        assertions_failed: 0,
                        logs,
                        extracted: vec![],
                        children: vec![],
                    };
                    ctx.case_passed = false;
                    ctx.step_results.push(res);
                    return;
                }
            };
            let items = match arr_val.as_array() {
                Some(a) => a.clone(),
                None => {
                    logs.push(format!(
                        "for-each source '{}' 不是数组",
                        truncate(source, 40)
                    ));
                    let res = TestStepResult {
                        step_id: step.id.clone(),
                        request_name: "遍历循环".into(),
                        status: 0,
                        passed: false,
                        time_ms: started.elapsed().as_millis() as u64,
                        error: Some("source is not an array".into()),
                        assertions_passed: 0,
                        assertions_failed: 0,
                        logs,
                        extracted: vec![],
                        children: vec![],
                    };
                    ctx.case_passed = false;
                    ctx.step_results.push(res);
                    return;
                }
            };
            logs.push(format!(
                "遍历 {} ({} 项)",
                truncate(source, 40),
                items.len()
            ));
            for (i, item) in items.iter().enumerate() {
                if ctx.hard_failed {
                    break;
                }
                let mut iter_ctx = RunContext {
                    project: ctx.project,
                    http_client: ctx.http_client,
                    scope: ctx.scope.clone(),
                    step_results: Vec::new(),
                    case_passed: true,
                    hard_failed: false,
                };
                // Bind the loop variable (JSON value as string).
                let val_str = match item {
                    JsonValue::String(s) => s.clone(),
                    other => other.to_string(),
                };
                iter_ctx.scope.insert(var.clone(), val_str);
                iter_ctx.scope.insert("$iteration".into(), i.to_string());
                iter_ctx.scope.insert("$index".into(), i.to_string());

                for child in &step.body_steps {
                    execute_step(child, &mut iter_ctx).await;
                }

                ctx.scope = iter_ctx.scope;
                if !iter_ctx.case_passed {
                    loop_passed = false;
                }
                children.extend(iter_ctx.step_results);
            }
        }
        LoopMode::While { condition } => {
            logs.push(format!("当: {}", truncate(condition, 40)));
            let mut iter = 0u32;
            loop {
                if iter >= step.max_iterations {
                    logs.push(format!("达到最大迭代次数 ({})", step.max_iterations));
                    break;
                }
                if ctx.hard_failed {
                    break;
                }
                let (truthy, err) = scripting::eval_condition(condition, &ctx.scope);
                if let Some(e) = err {
                    logs.push(format!("while condition error: {e}"));
                    loop_passed = false;
                    break;
                }
                if !truthy {
                    break;
                }
                let mut iter_ctx = RunContext {
                    project: ctx.project,
                    http_client: ctx.http_client,
                    scope: ctx.scope.clone(),
                    step_results: Vec::new(),
                    case_passed: true,
                    hard_failed: false,
                };
                iter_ctx.scope.insert("$iteration".into(), iter.to_string());
                iter_ctx.scope.insert("$index".into(), iter.to_string());

                for child in &step.body_steps {
                    execute_step(child, &mut iter_ctx).await;
                }

                ctx.scope = iter_ctx.scope;
                if !iter_ctx.case_passed {
                    loop_passed = false;
                }
                children.extend(iter_ctx.step_results);
                iter += 1;
            }
        }
    }

    if !loop_passed {
        ctx.case_passed = false;
    }

    let res = TestStepResult {
        step_id: step.id.clone(),
        request_name: match &step.mode {
            LoopMode::Repeat { count } => format!("重复 {} 次", count),
            LoopMode::ForEach { var, source } => {
                format!("遍历 {} in {}", var, truncate(source, 30))
            }
            LoopMode::While { condition } => format!("当 {}", truncate(condition, 30)),
        },
        status: 0,
        passed: loop_passed,
        time_ms: started.elapsed().as_millis() as u64,
        error: None,
        assertions_passed: 0,
        assertions_failed: 0,
        logs,
        extracted: vec![],
        children,
    };
    ctx.step_results.push(res);
}

async fn execute_wait_step(step: &WaitStep, ctx: &mut RunContext<'_>) {
    let started = Instant::now();
    smol::Timer::after(Duration::from_millis(step.duration_ms)).await;
    let res = TestStepResult {
        step_id: step.id.clone(),
        request_name: format!("等待 {} ms", step.duration_ms),
        status: 0,
        passed: true,
        time_ms: started.elapsed().as_millis() as u64,
        error: None,
        assertions_passed: 0,
        assertions_failed: 0,
        logs: vec![],
        extracted: vec![],
        children: vec![],
    };
    ctx.step_results.push(res);
}

async fn execute_script_step(step: &ScriptStep, ctx: &mut RunContext<'_>) {
    let started = Instant::now();
    let mut logs = Vec::new();
    let mut assertions_passed = 0usize;
    let mut assertions_failed = 0usize;
    let mut passed = true;

    if !step.script.trim().is_empty() {
        let res = scripting::run_standalone_script(&step.script, &ctx.scope);
        for l in res.logs {
            logs.push(l);
        }
        for eff in res.effects {
            match eff {
                scripting::SideEffect::SetVariable { name, value, .. } => {
                    ctx.scope.insert(name, value);
                }
                scripting::SideEffect::Log(line) => logs.push(line),
                scripting::SideEffect::Assert {
                    passed: ap,
                    message,
                } => {
                    if ap {
                        assertions_passed += 1;
                    } else {
                        assertions_failed += 1;
                        passed = false;
                    }
                    logs.push(format!(
                        "{}: {message}",
                        if ap { "✓ PASS" } else { "✗ FAIL" }
                    ));
                }
            }
        }
        if let Some(e) = res.error {
            logs.push(format!("script error: {e}"));
            passed = false;
        }
    }

    if !passed && !step.continue_on_error {
        ctx.hard_failed = true;
    }
    if !passed {
        ctx.case_passed = false;
    }

    let res = TestStepResult {
        step_id: step.id.clone(),
        request_name: "脚本".into(),
        status: 0,
        passed,
        time_ms: started.elapsed().as_millis() as u64,
        error: None,
        assertions_passed,
        assertions_failed,
        logs,
        extracted: vec![],
        children: vec![],
    };
    ctx.step_results.push(res);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
    use crate::autotest::models::{ExtractSource, RequestStep, VariableExtractor};
    use crate::state::models::{AuthConfig, RequestBody, RequestMethod};

    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    /// A minimal HTTP/1.1 server that returns canned JSON based on the path.
    /// GET /login → 200 {"data":{"token":"secret123", "items":[{"id":1},{"id":2}]}}
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
                let Ok(req_str) = std::str::from_utf8(&buf) else {
                    continue;
                };
                let path = req_str
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();

                let auth_hdr = req_str
                    .lines()
                    .find(|l| l.to_ascii_lowercase().starts_with("authorization:"))
                    .map(|l| {
                        l.split_once(':')
                            .map(|x| x.1)
                            .unwrap_or("")
                            .trim()
                            .to_string()
                    })
                    .unwrap_or_default();

                let (status, body) = if path.starts_with("/login") {
                    (
                        200,
                        r#"{"data":{"token":"secret123","items":[{"id":1},{"id":2}]}}"#.to_string(),
                    )
                } else if path.starts_with("/me") {
                    (200, format!(r#"{{"ok":true,"auth":"{auth_hdr}"}}"#))
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
        let v: JsonValue =
            serde_json::from_str(r#"{"data":{"token":"abc","arr":[{"x":1}]}}"#).unwrap();
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

        let mut login = ApiRequest::new(
            "Login",
            RequestMethod::Get,
            format!("http://127.0.0.1:{port}/login"),
        );
        login.tests_script = "apt.assert(response.status === 200);".into();
        let mut me = ApiRequest::new(
            "Me",
            RequestMethod::Get,
            format!("http://127.0.0.1:{port}/me"),
        );
        me.headers
            .push(KeyValue::new("Authorization", "Bearer {{token}}"));
        me.tests_script = "apt.assert(response.json.auth === 'Bearer secret123');".into();

        let mut project = Project::new("Test");
        project.requests.push(login.clone());
        project.requests.push(me.clone());

        let mut case = TestCase::new("scenario");
        let mut step1 = RequestStep::new(&login.id);
        step1.extractors.push(VariableExtractor {
            name: "token".into(),
            source: ExtractSource::JsonPath,
            path: "data.token".into(),
        });
        case.steps.push(TestCaseStep::Request(step1));
        case.steps
            .push(TestCaseStep::Request(RequestStep::new(&me.id)));

        let (tx, _rx) = mpsc::unbounded_channel::<RunEvent>();

        let result = smol::block_on(run_case(&case, &project, &*client, tx, None))
            .expect("run_case should return Ok");

        assert!(result.passed, "case should pass; steps: {:?}", result.steps);
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[0].status, 200);
        assert_eq!(result.steps[0].extracted.len(), 1);
        assert_eq!(result.steps[0].extracted[0].0, "token");
        assert_eq!(result.steps[0].extracted[0].1, "secret123");
        assert_eq!(result.steps[1].status, 200);
    }

    #[test]
    fn run_case_with_if_condition() {
        let port = spawn_fixture_server();
        let client = build_client();

        let mut login = ApiRequest::new(
            "Login",
            RequestMethod::Get,
            format!("http://127.0.0.1:{port}/login"),
        );
        login.tests_script = "apt.assert(response.status === 200);".into();
        let mut me = ApiRequest::new(
            "Me",
            RequestMethod::Get,
            format!("http://127.0.0.1:{port}/me"),
        );
        me.headers
            .push(KeyValue::new("Authorization", "Bearer {{token}}"));

        let mut project = Project::new("Test");
        project.requests.push(login.clone());
        project.requests.push(me.clone());

        let mut case = TestCase::new("if-scenario");
        let mut step1 = RequestStep::new(&login.id);
        step1.extractors.push(VariableExtractor {
            name: "token".into(),
            source: ExtractSource::JsonPath,
            path: "data.token".into(),
        });
        case.steps.push(TestCaseStep::Request(step1));

        // Add an If step that runs "me" only if token exists.
        let mut if_step = IfStep::new("token && token.length > 0");
        if_step
            .then_steps
            .push(TestCaseStep::Request(RequestStep::new(&me.id)));
        case.steps.push(TestCaseStep::If(if_step));

        let (tx, _rx) = mpsc::unbounded_channel::<RunEvent>();
        let result = smol::block_on(run_case(&case, &project, &*client, tx, None)).unwrap();
        assert!(result.passed, "case passed: {:?}", result.steps);
        // Expect 2 top-level steps (request + if), and the if has 1 child.
        assert_eq!(result.steps.len(), 2);
        assert_eq!(result.steps[1].children.len(), 1);
    }

    #[test]
    fn run_case_with_repeat_loop() {
        let port = spawn_fixture_server();
        let client = build_client();

        let login = ApiRequest::new(
            "Login",
            RequestMethod::Get,
            format!("http://127.0.0.1:{port}/login"),
        );
        let mut project = Project::new("Test");
        project.requests.push(login.clone());

        let mut case = TestCase::new("loop-scenario");
        let mut loop_step = LoopStep::new(LoopMode::Repeat { count: 3 });
        loop_step
            .body_steps
            .push(TestCaseStep::Request(RequestStep::new(&login.id)));
        case.steps.push(TestCaseStep::Loop(loop_step));

        let (tx, _rx) = mpsc::unbounded_channel::<RunEvent>();
        let result = smol::block_on(run_case(&case, &project, &*client, tx, None)).unwrap();
        assert!(result.passed);
        assert_eq!(result.steps.len(), 1);
        assert_eq!(result.steps[0].children.len(), 3);
    }

    #[test]
    fn run_case_missing_request_fails_step() {
        let client = build_client();
        let project = Project::new("Empty");
        let mut case = TestCase::new("bad");
        case.steps
            .push(TestCaseStep::Request(RequestStep::new("does-not-exist")));
        let (tx, _rx) = mpsc::unbounded_channel();
        let result = smol::block_on(run_case(&case, &project, &*client, tx, None)).unwrap();
        assert!(!result.passed);
        assert_eq!(result.steps.len(), 1);
        assert!(result.steps[0].error.is_some());
    }
}
