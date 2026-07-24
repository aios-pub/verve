//! Pre/Post-request script execution (PRD §5.2).
//!
//! Scripts are JavaScript (ES6+) run in a sandboxed [boa_engine] interpreter.
//! The PRD's `apt` global object exposes:
//!
//! - `apt.variables.set(name, value)` / `apt.variables.get(name)`
//! - `apt.setVariable(name, value)` / `apt.getVariable(name)` (aliases)
//! - `apt.environment.set(name, value)` / `apt.environment.get(name)`
//! - `apt.assert(condition, message?)` — records an assertion result
//! - `apt.echo(...)` / `console.log(...)` — captures console output
//!
//! In **post-request** scripts a global `response` object is available:
//!
//! ```js
//! response.status   // number
//! response.body     // string (raw)
//! response.json     // parsed object (when the body is JSON)
//! response.headers  // object {key: value}
//! response.time     // number (ms)
//! ```
//!
//! Variable mutations (`apt.variables.set` / `apt.environment.set`) are
//! captured as side effects and applied to the live state by the caller.

use std::collections::BTreeMap;

use boa_engine::object::ObjectInitializer;
use boa_engine::object::builtins::JsFunction;
use boa_engine::property::Attribute;
use boa_engine::{
    Context, JsArgs, JsValue, NativeFunction, Source, js_string, object::FunctionObjectBuilder,
};

use crate::state::models::Response;

/// Which variable scope a `set` targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarScope {
    /// The active environment (the default — survives across requests).
    Environment,
    /// Per-request.
    Request,
}

/// A captured side effect from script execution.
#[derive(Debug, Clone)]
pub enum SideEffect {
    SetVariable {
        scope: VarScope,
        name: String,
        value: String,
    },
    /// A console line (from `apt.echo` / `console.log`).
    Log(String),
    /// An assertion outcome.
    Assert { passed: bool, message: String },
}

/// The collected side effects + any runtime error from a script run.
#[derive(Debug, Default, Clone)]
pub struct ScriptResult {
    pub effects: Vec<SideEffect>,
    /// Console + assertion lines for display.
    pub logs: Vec<String>,
    pub assertions_passed: usize,
    pub assertions_failed: usize,
    /// A JS runtime error, if the script threw.
    pub error: Option<String>,
}

/// Run a pre-request script. `vars` is the resolved variable pool so
/// `apt.variables.get` returns the effective values.
pub fn run_pre_request(script: &str, vars: &BTreeMap<String, String>) -> ScriptResult {
    run(script, vars, None)
}

/// Run a post-request (Tests) script. The `response` global is populated from
/// the captured [`Response`].
pub fn run_post_request(
    script: &str,
    vars: &BTreeMap<String, String>,
    response: &Response,
) -> ScriptResult {
    run(script, vars, Some(response))
}

/// Run a standalone script (no `response` global). Used for Script steps.
pub fn run_standalone_script(script: &str, vars: &BTreeMap<String, String>) -> ScriptResult {
    run(script, vars, None)
}

/// Evaluate a JS expression in a variable scope and return whether it's truthy.
/// Used by If/While conditions. Returns `(truthy, error_message)`.
pub fn eval_condition(expr: &str, vars: &BTreeMap<String, String>) -> (bool, Option<String>) {
    match eval_expression_value(expr, vars) {
        Ok(val) => (js_value_truthy(&val), None),
        Err(e) => (false, Some(e)),
    }
}

/// Evaluate a JS expression and return its value as a serde_json::Value.
/// Primarily used for for-each loop source expressions.
pub fn eval_expression(
    expr: &str,
    vars: &BTreeMap<String, String>,
) -> Result<serde_json::Value, String> {
    eval_expression_value(expr, vars)
}

/// Shared evaluator: builds a boa context, installs `apt` + variables, evaluates,
/// and converts the result back to JSON.
///
/// Variables are injected as direct bindings so conditions can write
/// `token && token.length > 0` rather than `apt.variables.get('token')`.
fn eval_expression_value(
    expr: &str,
    vars: &BTreeMap<String, String>,
) -> Result<serde_json::Value, String> {
    use boa_engine::{Context, JsValue, Source, js_string, property::Attribute};
    let mut ctx = Context::default();
    let vars_json = serde_json::to_string(vars).unwrap_or_else(|_| "{}".into());
    let _ = ctx.register_global_property(
        js_string!("__verve_vars__"),
        JsValue::from(js_string!(vars_json.as_str())),
        Attribute::all(),
    );
    let _ = ctx.register_global_property(
        js_string!("__verve_effects__"),
        JsValue::from(js_string!("")),
        Attribute::all(),
    );
    install_apt(&mut ctx).map_err(|e| e.to_string())?;

    // Build a vars object and wrap the expression in `with (vars) { return (expr); }`
    // so that `token` resolves as a property lookup.
    let mut bindings =
        String::from("(function() { var __v = JSON.parse(__verve_vars__); with (__v) { return (");
    bindings.push_str(expr);
    bindings.push_str("); } })()");

    let value = ctx
        .eval(Source::from_bytes(&bindings))
        .map_err(|e| e.to_string())?;
    js_to_json(&value, &mut ctx)
}

/// Convert a boa JsValue to a serde_json::Value.
fn js_to_json(val: &JsValue, ctx: &mut Context) -> Result<serde_json::Value, String> {
    use boa_engine::JsValue as V;
    if val.is_undefined() || val.is_null() {
        Ok(serde_json::Value::Null)
    } else if val.is_boolean() {
        Ok(serde_json::Value::Bool(val.to_boolean()))
    } else if val.is_number() {
        if let Some(n) = val.to_number(ctx).ok() {
            Ok(serde_json::json!(n))
        } else {
            Ok(serde_json::Value::Null)
        }
    } else if val.is_string() {
        let s = val
            .to_string(ctx)
            .map(|j| j.to_std_string().unwrap_or_default())
            .unwrap_or_default();
        Ok(serde_json::Value::String(s))
    } else if val.is_object() {
        // Try to JSON.stringify the object.
        let json_str = ctx
            .global_object()
            .get(js_string!("JSON"), ctx)
            .map_err(|e| e.to_string())?
            .as_object()
            .ok_or("JSON not found")?
            .get(js_string!("stringify"), ctx)
            .map_err(|e| e.to_string())?
            .as_callable()
            .ok_or("JSON.stringify not callable")?
            .call(&JsValue::undefined(), std::slice::from_ref(val), ctx)
            .map_err(|e| e.to_string())?;
        let s = json_str
            .to_string(ctx)
            .map(|j| j.to_std_string().unwrap_or_default())
            .unwrap_or_default();
        serde_json::from_str(&s).map_err(|e| e.to_string())
    } else {
        Ok(serde_json::Value::Null)
    }
}

/// JS truthiness rules.
fn js_value_truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Null => false,
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => {
            n.as_f64().map(|f| f != 0.0 && !f.is_nan()).unwrap_or(false)
        }
        serde_json::Value::String(s) => !s.is_empty(),
        serde_json::Value::Array(a) => !a.is_empty(),
        serde_json::Value::Object(_) => true,
    }
}

fn run(script: &str, vars: &BTreeMap<String, String>, response: Option<&Response>) -> ScriptResult {
    if script.trim().is_empty() {
        return ScriptResult::default();
    }
    let mut ctx = Context::default();

    // Stash the variable pool + an effects list as globals so the `fn`-ptr
    // native functions can read/write them (they cannot capture state).
    let vars_json = serde_json::to_string(vars).unwrap_or_else(|_| "{}".into());
    let _ = ctx.register_global_property(
        js_string!("__verve_vars__"),
        JsValue::from(js_string!(vars_json.as_str())),
        Attribute::all(),
    );
    let _ = ctx.register_global_property(
        js_string!("__verve_effects__"),
        JsValue::from(js_string!("")),
        Attribute::all(),
    );

    if let Err(e) = install_apt(&mut ctx) {
        return ScriptResult {
            error: Some(format!("failed to init script runtime: {e}")),
            ..Default::default()
        };
    }
    if let Err(e) = install_response(&mut ctx, response) {
        return ScriptResult {
            error: Some(format!("failed to init response object: {e}")),
            ..Default::default()
        };
    }

    let eval_result = ctx.eval(Source::from_bytes(script));
    let collected = collect_effects(&mut ctx);

    let mut result = ScriptResult::default();
    for effect in &collected {
        match effect {
            SideEffect::Log(line) => result.logs.push(line.clone()),
            SideEffect::Assert { passed, message } => {
                if *passed {
                    result.assertions_passed += 1;
                } else {
                    result.assertions_failed += 1;
                }
                result.logs.push(format!(
                    "{}: {message}",
                    if *passed { "✓ PASS" } else { "✗ FAIL" }
                ));
            }
            SideEffect::SetVariable { .. } => {}
        }
    }
    result.effects = collected;
    if let Err(e) = eval_result {
        result.error = Some(e.to_string());
    }
    result
}

/// Build a native function via `FunctionObjectBuilder`.
fn func(ctx: &mut Context, native: NativeFunction, name: &str, length: usize) -> JsFunction {
    FunctionObjectBuilder::new(ctx.realm(), native)
        .name(js_string!(name))
        .length(length)
        .constructor(false)
        .build()
}

/// Install the `apt` global, `console`, and stashed globals.
fn install_apt(ctx: &mut Context) -> Result<(), boa_engine::JsError> {
    // --- console.log ---
    let console = {
        let mut obj = ObjectInitializer::new(ctx);
        obj.function(
            NativeFunction::from_fn_ptr(log_native),
            js_string!("log"),
            0,
        );
        obj.build()
    };
    ctx.register_global_property(js_string!("console"), console, Attribute::all())?;

    // --- apt.variables / apt.environment (get/set) ---
    let variables = {
        let mut obj = ObjectInitializer::new(ctx);
        obj.function(
            NativeFunction::from_fn_ptr(var_get_native),
            js_string!("get"),
            1,
        );
        obj.function(
            NativeFunction::from_fn_ptr(var_set_env_native),
            js_string!("set"),
            2,
        );
        obj.build()
    };
    let environment = variables.clone();

    let mut apt = ObjectInitializer::new(ctx);
    apt.function(
        NativeFunction::from_fn_ptr(echo_native),
        js_string!("echo"),
        0,
    );
    apt.function(
        NativeFunction::from_fn_ptr(assert_native),
        js_string!("assert"),
        2,
    );
    apt.property(js_string!("variables"), variables, Attribute::all());
    apt.property(js_string!("environment"), environment, Attribute::all());
    apt.function(
        NativeFunction::from_fn_ptr(var_set_env_native),
        js_string!("setVariable"),
        2,
    );
    apt.function(
        NativeFunction::from_fn_ptr(var_get_native),
        js_string!("getVariable"),
        1,
    );
    let apt_obj = apt.build();

    ctx.register_global_property(js_string!("apt"), apt_obj, Attribute::all())?;
    Ok(())
}

/// Install the global `response` object for post-request scripts.
fn install_response(
    ctx: &mut Context,
    response: Option<&Response>,
) -> Result<(), boa_engine::JsError> {
    // Compute the parsed JSON value first (borrows ctx), before building the
    // response object (which also borrows ctx mutably).
    let json_val: Option<JsValue> = match response {
        Some(r) if r.is_json => serde_json::from_str::<serde_json::Value>(&r.body)
            .ok()
            .map(|v| json_to_js_value(ctx, &v))
            .transpose()?,
        _ => None,
    };

    let resp_obj = ObjectInitializer::new(ctx).build();
    if let Some(r) = response {
        let _ = resp_obj.set(
            js_string!("status"),
            JsValue::from(r.status as f64),
            false,
            ctx,
        );
        let _ = resp_obj.set(
            js_string!("time"),
            JsValue::from(r.time_ms as f64),
            false,
            ctx,
        );
        let _ = resp_obj.set(
            js_string!("body"),
            JsValue::from(js_string!(r.body.as_str())),
            false,
            ctx,
        );
        if let Some(jv) = json_val {
            let _ = resp_obj.set(js_string!("json"), jv, false, ctx);
        }
        // headers object
        let headers = ObjectInitializer::new(ctx).build();
        for kv in &r.headers {
            let _ = headers.set(
                js_string!(kv.key.as_str()),
                JsValue::from(js_string!(kv.value.as_str())),
                false,
                ctx,
            );
        }
        let _ = resp_obj.set(js_string!("headers"), headers, false, ctx);
    } else {
        let _ = resp_obj.set(js_string!("status"), JsValue::from(0.0f64), false, ctx);
        let _ = resp_obj.set(js_string!("body"), JsValue::undefined(), false, ctx);
    }
    ctx.register_global_property(js_string!("response"), resp_obj, Attribute::all())?;
    Ok(())
}

/// Convert a serde_json::Value into a boa JsValue. Uses `JsObject::set` to
/// avoid holding a mutable context borrow across nested construction.
fn json_to_js_value(
    ctx: &mut Context,
    value: &serde_json::Value,
) -> Result<JsValue, boa_engine::JsError> {
    Ok(match value {
        serde_json::Value::Null => JsValue::null(),
        serde_json::Value::Bool(b) => JsValue::from(*b),
        serde_json::Value::Number(n) => JsValue::from(n.as_f64().unwrap_or(0.0)),
        serde_json::Value::String(s) => JsValue::from(js_string!(s.as_str())),
        serde_json::Value::Array(arr) => {
            let array = boa_engine::object::builtins::JsArray::new(ctx);
            for v in arr {
                array.push(json_to_js_value(ctx, v)?, ctx)?;
            }
            array.into()
        }
        serde_json::Value::Object(map) => {
            let obj = ObjectInitializer::new(ctx).build();
            for (k, v) in map {
                let val = json_to_js_value(ctx, v)?;
                let _ = obj.set(js_string!(k.as_str()), val, false, ctx);
            }
            obj.into()
        }
    })
}

// ---------------------------------------------------------------------------
// Native functions (`fn` pointers — no captures; state via context globals).
// ---------------------------------------------------------------------------

fn log_native(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> Result<JsValue, boa_engine::JsError> {
    let msg = join_args(args, ctx);
    push_effect(ctx, SideEffect::Log(msg));
    Ok(JsValue::undefined())
}

fn echo_native(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> Result<JsValue, boa_engine::JsError> {
    let msg = join_args(args, ctx);
    push_effect(ctx, SideEffect::Log(msg));
    Ok(JsValue::undefined())
}

fn assert_native(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> Result<JsValue, boa_engine::JsError> {
    let passed = args.get_or_undefined(0).to_boolean();
    let message = args
        .get(1)
        .and_then(|v| {
            if v.is_undefined() {
                None
            } else {
                v.to_string(ctx).ok().map(|s| s.to_std_string_escaped())
            }
        })
        .unwrap_or_else(|| "assertion".to_string());
    push_effect(ctx, SideEffect::Assert { passed, message });
    Ok(JsValue::undefined())
}

/// `apt.variables.get(name)` / `apt.getVariable(name)`.
fn var_get_native(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> Result<JsValue, boa_engine::JsError> {
    let name = arg_string(args, 0, ctx);
    let value = current_var(ctx, &name);
    Ok(JsValue::from(js_string!(value.as_str())))
}

/// `apt.variables.set(name, value)` / `apt.environment.set` / `apt.setVariable`.
fn var_set_env_native(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> Result<JsValue, boa_engine::JsError> {
    let name = arg_string(args, 0, ctx);
    let value = arg_string(args, 1, ctx);
    push_effect(
        ctx,
        SideEffect::SetVariable {
            scope: VarScope::Environment,
            name,
            value,
        },
    );
    Ok(JsValue::undefined())
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn join_args(args: &[JsValue], ctx: &mut Context) -> String {
    args.iter()
        .map(|v| {
            if v.is_undefined() {
                "undefined".to_string()
            } else {
                v.to_string(ctx)
                    .map(|s| s.to_std_string_escaped())
                    .unwrap_or_default()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn arg_string(args: &[JsValue], ix: usize, ctx: &mut Context) -> String {
    args.get_or_undefined(ix)
        .to_string(ctx)
        .map(|s| s.to_std_string_escaped())
        .unwrap_or_default()
}

/// Serialize a side effect and append to the `__verve_effects__` global string.
fn push_effect(ctx: &mut Context, effect: SideEffect) {
    let tag = match &effect {
        SideEffect::Log(msg) => format!("LOG\x1f{}", escape(msg)),
        SideEffect::Assert { passed, message } => {
            format!("ASSERT\x1f{passed}\x1f{}", escape(message))
        }
        SideEffect::SetVariable { scope, name, value } => {
            let s = if scope == &VarScope::Environment {
                "env"
            } else {
                "req"
            };
            format!("SET\x1f{s}\x1f{}\x1f{}", escape(name), escape(value))
        }
    };
    let cur = read_global_string(ctx, "__verve_effects__");
    let next = if cur.is_empty() {
        tag
    } else {
        format!("{cur}\x1e{tag}")
    };
    write_global_string(ctx, "__verve_effects__", &next);
}

/// Read the effects list back from the context at the end of a run.
fn collect_effects(ctx: &mut Context) -> Vec<SideEffect> {
    let raw = read_global_string(ctx, "__verve_effects__");
    if raw.is_empty() {
        return Vec::new();
    }
    raw.split('\x1e')
        .filter_map(|chunk| {
            let mut parts = chunk.splitn(4, '\x1f');
            match parts.next()? {
                "LOG" => {
                    let m = unescape(parts.next().unwrap_or(""));
                    Some(SideEffect::Log(m))
                }
                "ASSERT" => {
                    let passed = parts.next() == Some("true");
                    let message = unescape(parts.next().unwrap_or("assertion"));
                    Some(SideEffect::Assert { passed, message })
                }
                "SET" => {
                    let scope_s = parts.next().unwrap_or("env");
                    let name = unescape(parts.next().unwrap_or(""));
                    let value = unescape(parts.next().unwrap_or(""));
                    let scope = if scope_s == "req" {
                        VarScope::Request
                    } else {
                        VarScope::Environment
                    };
                    Some(SideEffect::SetVariable { scope, name, value })
                }
                _ => None,
            }
        })
        .collect()
}

/// Read a variable's current value from the stashed vars pool.
fn current_var(ctx: &mut Context, name: &str) -> String {
    let json = read_global_string(ctx, "__verve_vars__");
    let map: BTreeMap<String, String> = serde_json::from_str(&json).unwrap_or_default();
    map.get(name).cloned().unwrap_or_default()
}

fn read_global_string(ctx: &mut Context, key: &str) -> String {
    ctx.global_object()
        .get(js_string!(key), ctx)
        .ok()
        .and_then(|v| {
            v.as_string()
                .map(|s| s.to_std_string_escaped())
                .or_else(|| v.as_number().map(|n| n.to_string()))
        })
        .unwrap_or_default()
}

fn write_global_string(ctx: &mut Context, key: &str, value: &str) {
    let _ = ctx.global_object().set(
        boa_engine::property::PropertyKey::String(js_string!(key)),
        JsValue::from(js_string!(value)),
        false,
        ctx,
    );
}

/// Escape the record/unit separators we use as delimiters.
fn escape(s: &str) -> String {
    s.replace('\x1e', "\u{FFFE}").replace('\x1f', "\u{FFFD}")
}

fn unescape(s: &str) -> String {
    s.replace('\u{FFFE}', "\x1e").replace('\u{FFFD}', "\x1f")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars() -> BTreeMap<String, String> {
        BTreeMap::new()
    }

    #[test]
    fn pre_request_sets_variable() {
        let result = run_pre_request("apt.setVariable('token', 'abc123')", &vars());
        assert!(result.error.is_none(), "{:?}", result.error);
        let set = result
            .effects
            .iter()
            .filter_map(|e| match e {
                SideEffect::SetVariable { name, value, .. } => Some((name.clone(), value.clone())),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(set.iter().any(|(n, v)| n == "token" && v == "abc123"));
    }

    #[test]
    fn get_variable_works() {
        let mut v = vars();
        v.insert("host".to_string(), "example.com".to_string());
        let result = run_pre_request("apt.echo(apt.getVariable('host'))", &v);
        assert!(result.error.is_none(), "{:?}", result.error);
        assert!(result.logs.iter().any(|l| l.contains("example.com")));
    }

    #[test]
    fn assert_records_results() {
        let result = run_pre_request(
            "apt.assert(1 === 1, 'one'); apt.assert(1 === 2, 'two')",
            &vars(),
        );
        assert_eq!(result.assertions_passed, 1);
        assert_eq!(result.assertions_failed, 1);
    }

    #[test]
    fn post_request_reads_response() {
        let resp = Response {
            status: 200,
            body: r#"{"code":0,"token":"xyz"}"#.to_string(),
            is_json: true,
            ..Default::default()
        };
        let result = run_post_request(
            "if (response.json.code === 0) { apt.setVariable('token', response.json.token); } apt.assert(response.status === 200, 'ok status')",
            &vars(),
            &resp,
        );
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.assertions_passed, 1);
        let token = result.effects.iter().find_map(|e| match e {
            SideEffect::SetVariable { name, value, .. } if name == "token" => Some(value.clone()),
            _ => None,
        });
        assert_eq!(token.as_deref(), Some("xyz"));
    }

    #[test]
    fn empty_script_is_noop() {
        let result = run_pre_request("   ", &vars());
        assert!(result.effects.is_empty());
        assert!(result.error.is_none());
    }

    #[test]
    fn syntax_error_is_captured() {
        let result = run_pre_request("apt.assert(", &vars());
        assert!(result.error.is_some());
    }
}
