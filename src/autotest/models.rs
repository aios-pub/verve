//! Data models for automated test suites.
//!
//! Mirrors Postman collection-runner concepts:
//!
//! - [`TestSuite`]  — a named collection of related test cases.
//! - [`TestCase`]   — one executable scenario: a sequence of ordered steps.
//! - [`TestCaseStep`] — one step in a case; can be an HTTP request, a conditional
//!   (if/else), a loop (for/while/repeat), a delay, or a standalone script.
//! - [`TestRunResult`] / [`TestStepResult`] — execution reports produced by
//!   the runner.
//!
//! All models are `Serialize`/`Deserialize` and persist into
//! `workspace.json` under `Project::test_suites`.

use serde::{Deserialize, Serialize};

use crate::state::models::{KeyValue, new_id};

/// A suite of test cases, analogous to a folder in Postman's collection runner.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestSuite {
    pub id: String,
    pub name: String,
    pub description: String,
    /// Cases are ordered; the runner executes them in Vec order.
    pub test_cases: Vec<TestCase>,
    /// Suite-level variables (overridden by case / step-level variables).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<KeyValue>,
}

impl TestSuite {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            description: String::new(),
            test_cases: Vec::new(),
            variables: Vec::new(),
        }
    }
}

/// A single executable test case — an ordered sequence of steps that share
/// the same variable scope (variables extracted by step N are visible to
/// step N+1).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestCase {
    pub id: String,
    pub name: String,
    pub description: String,
    pub steps: Vec<TestCaseStep>,
    /// Number of times to run the whole scenario (default = 1). For
    /// single-run load-style repeat use, see the stress-test module.
    #[serde(default = "default_iteration_count")]
    pub iteration_count: u32,
    /// Case-level variables.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub variables: Vec<KeyValue>,
    /// Optional folder id used only for UI grouping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<String>,
}

fn default_iteration_count() -> u32 {
    1
}

impl TestCase {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            description: String::new(),
            steps: Vec::new(),
            iteration_count: 1,
            variables: Vec::new(),
            folder_id: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Step types
// ---------------------------------------------------------------------------

/// One step in a test case. The runner dispatches on the variant.
///
/// Serialized with an internal `type` tag (`request`, `if`, `loop`, `wait`,
/// `script`). Old on-disk data (which was a flat struct with no tag) is
/// transparently deserialized as a legacy `Request` step.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum TestCaseStep {
    /// Execute an HTTP request (the common case).
    Request(RequestStep),
    /// Conditional branch — runs `then_steps` if `condition` evaluates truthy,
    /// otherwise runs `else_steps`.
    If(IfStep),
    /// Loop — repeats `body_steps` according to the loop mode.
    Loop(LoopStep),
    /// Wait / sleep for a fixed number of milliseconds.
    Wait(WaitStep),
    /// Run a standalone script (no HTTP call). Useful for setting variables,
    /// assertions that don't depend on a response, or logging.
    Script(ScriptStep),
}

// Custom deserialize to support legacy (untagged) on-disk format from v0.3.
// Old steps were flat structs with `request_id` and no `type` field — those
// become `Request` steps.
impl<'de> Deserialize<'de> for TestCaseStep {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, Visitor};
        use std::fmt;

        struct StepVisitor;

        // Helper tagged enum so serde does the heavy lifting.
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "lowercase")]
        enum Tagged {
            Request(RequestStep),
            If(IfStep),
            Loop(LoopStep),
            Wait(WaitStep),
            Script(ScriptStep),
        }

        impl<'de> Visitor<'de> for StepVisitor {
            type Value = TestCaseStep;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a TestCaseStep (map with or without `type`)")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                // Buffer into a JsonObject so we can inspect/augment it.
                let mut obj = serde_json::Map::new();
                while let Some((k, v)) = map.next_entry::<String, serde_json::Value>()? {
                    obj.insert(k, v);
                }

                // Legacy format: no `type` field but has `request_id` → Request.
                if !obj.contains_key("type") {
                    obj.insert("type".into(), serde_json::Value::String("request".into()));
                }

                let v = serde_json::Value::Object(obj);
                match Tagged::deserialize(v) {
                    Ok(Tagged::Request(s)) => Ok(TestCaseStep::Request(s)),
                    Ok(Tagged::If(s)) => Ok(TestCaseStep::If(s)),
                    Ok(Tagged::Loop(s)) => Ok(TestCaseStep::Loop(s)),
                    Ok(Tagged::Wait(s)) => Ok(TestCaseStep::Wait(s)),
                    Ok(Tagged::Script(s)) => Ok(TestCaseStep::Script(s)),
                    Err(e) => Err(de::Error::custom(e)),
                }
            }
        }

        deserializer.deserialize_map(StepVisitor)
    }
}

impl TestCaseStep {
    /// Unique id for this step (used for UI keys + run result correlation).
    pub fn id(&self) -> &str {
        match self {
            TestCaseStep::Request(s) => &s.id,
            TestCaseStep::If(s) => &s.id,
            TestCaseStep::Loop(s) => &s.id,
            TestCaseStep::Wait(s) => &s.id,
            TestCaseStep::Script(s) => &s.id,
        }
    }

    /// Display label for the step list.
    pub fn display_label(&self) -> String {
        match self {
            TestCaseStep::Request(s) => s
                .name_override
                .clone()
                .unwrap_or_else(|| "HTTP 请求".into()),
            TestCaseStep::If(s) => format!("如果: {}", truncate(&s.condition, 40)),
            TestCaseStep::Loop(s) => match &s.mode {
                LoopMode::Repeat { count } => format!("重复 {} 次", count),
                LoopMode::ForEach { var, source } => format!("遍历 {} 为 {}", source, var),
                LoopMode::While { condition } => format!("当: {}", truncate(condition, 30)),
            },
            TestCaseStep::Wait(s) => format!("等待 {} ms", s.duration_ms),
            TestCaseStep::Script(_) => "脚本".into(),
        }
    }

    /// Build a new Request step referencing the given ApiRequest id.
    pub fn new_request(request_id: impl Into<String>) -> Self {
        TestCaseStep::Request(RequestStep::new(request_id))
    }

    /// Build a new If step with empty branches.
    pub fn new_if(condition: impl Into<String>) -> Self {
        TestCaseStep::If(IfStep::new(condition))
    }

    /// Build a new Repeat-N loop step.
    pub fn new_repeat(count: u32) -> Self {
        TestCaseStep::Loop(LoopStep::new(LoopMode::Repeat { count }))
    }

    /// Build a new for-each loop.
    pub fn new_for_each(var: impl Into<String>, source: impl Into<String>) -> Self {
        TestCaseStep::Loop(LoopStep::new(LoopMode::ForEach {
            var: var.into(),
            source: source.into(),
        }))
    }

    /// Build a new while loop.
    pub fn new_while(condition: impl Into<String>) -> Self {
        TestCaseStep::Loop(LoopStep::new(LoopMode::While {
            condition: condition.into(),
        }))
    }

    /// Build a new Wait step.
    pub fn new_wait(duration_ms: u64) -> Self {
        TestCaseStep::Wait(WaitStep::new(duration_ms))
    }

    /// Build a new Script step.
    pub fn new_script(script: impl Into<String>) -> Self {
        TestCaseStep::Script(ScriptStep::new(script))
    }
}

impl Default for TestCaseStep {
    fn default() -> Self {
        TestCaseStep::Request(RequestStep::default())
    }
}

// ---------------------------------------------------------------------------
// Individual step structs
// ---------------------------------------------------------------------------

/// Execute an HTTP request referenced by id.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RequestStep {
    pub id: String,
    /// Display name override (defaults to the target request's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_override: Option<String>,
    /// The `ApiRequest::id` this step executes.
    pub request_id: String,
    /// Pre-request script override. If empty, the request's own pre_request
    /// script is used (set `inherit_request_scripts` = false to suppress it).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pre_script: String,
    /// Post-request / test script override. Same inheritance rules as
    /// `pre_script`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub post_script: String,
    /// When true (default), the request's own pre/post scripts run before the
    /// step-level overrides. When false, only the step-level scripts run.
    #[serde(default = "default_true")]
    pub inherit_request_scripts: bool,
    /// Variables extracted after this step completes successfully.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extractors: Vec<VariableExtractor>,
    /// When true, the case continues to the next step even if this step
    /// fails (assertion failure or HTTP error).
    #[serde(default)]
    pub continue_on_error: bool,
}

impl RequestStep {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            name_override: None,
            request_id: request_id.into(),
            pre_script: String::new(),
            post_script: String::new(),
            inherit_request_scripts: true,
            extractors: Vec::new(),
            continue_on_error: false,
        }
    }
}

/// Conditional branch.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct IfStep {
    pub id: String,
    /// JS expression evaluated against the current scope. Truthy → then branch.
    pub condition: String,
    /// Steps executed when `condition` is truthy.
    #[serde(default)]
    pub then_steps: Vec<TestCaseStep>,
    /// Steps executed when `condition` is falsy (optional).
    #[serde(default)]
    pub else_steps: Vec<TestCaseStep>,
}

impl IfStep {
    pub fn new(condition: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            condition: condition.into(),
            then_steps: Vec::new(),
            else_steps: Vec::new(),
        }
    }
}

/// Loop mode — how to repeat the body.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum LoopMode {
    /// Repeat the body exactly `count` times.
    Repeat { count: u32 },
    /// Iterate over a JSON array referenced by `source` (a JS expression,
    /// e.g. `response.json.data.items`). Each element is bound to `var`.
    ForEach { var: String, source: String },
    /// Repeat while `condition` evaluates truthy. Guarded by an iteration cap
    /// to avoid runaway loops.
    While { condition: String },
}

impl Default for LoopMode {
    fn default() -> Self {
        LoopMode::Repeat { count: 1 }
    }
}

/// Loop step.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LoopStep {
    pub id: String,
    #[serde(flatten)]
    pub mode: LoopMode,
    /// Steps executed for each iteration.
    #[serde(default)]
    pub body_steps: Vec<TestCaseStep>,
    /// Maximum iterations for While loops to prevent infinite loops (default 1000).
    #[serde(default = "default_loop_max")]
    pub max_iterations: u32,
}

fn default_loop_max() -> u32 {
    1000
}

impl LoopStep {
    pub fn new(mode: LoopMode) -> Self {
        Self {
            id: new_id(),
            mode,
            body_steps: Vec::new(),
            max_iterations: 1000,
        }
    }
}

/// Wait / delay step.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WaitStep {
    pub id: String,
    /// Milliseconds to sleep.
    pub duration_ms: u64,
}

impl WaitStep {
    pub fn new(duration_ms: u64) -> Self {
        Self {
            id: new_id(),
            duration_ms,
        }
    }
}

/// Standalone script step (no HTTP call).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScriptStep {
    pub id: String,
    /// JavaScript source. Runs in the same scripting context as a post-request
    /// script (no `response` binding, but `apt` and variables are available).
    pub script: String,
    /// When true, continue even if the script throws (default false).
    #[serde(default)]
    pub continue_on_error: bool,
}

impl ScriptStep {
    pub fn new(script: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            script: script.into(),
            continue_on_error: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Extractors & results
// ---------------------------------------------------------------------------

/// Declares one variable to extract from a step's response and expose to
/// downstream steps under `name`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariableExtractor {
    /// Variable name (written into the case scope via `apt.setVariable`).
    pub name: String,
    pub source: ExtractSource,
    /// Source-selector string:
    /// - JsonPath: a simple `a.b[0].c` dot-path (full JSONPath reserved for later).
    /// - Header: the header name.
    /// - Body/StatusCode/ResponseTime: ignored (empty).
    pub path: String,
}

/// Which part of the response the extractor reads from.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum ExtractSource {
    /// Dot-path into the JSON response body (e.g. `data.token`).
    #[default]
    JsonPath,
    /// Response header value (`path` = header name, case-insensitive).
    Header,
    /// HTTP status code (number).
    StatusCode,
    /// Total response time in milliseconds.
    ResponseTime,
    /// Raw response body as string.
    Body,
}

/// Result of a single step within a test run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestStepResult {
    pub step_id: String,
    /// Resolved display name (override or request name, or step label).
    pub request_name: String,
    /// HTTP status (0 for non-request steps).
    pub status: u16,
    pub passed: bool,
    pub time_ms: u64,
    /// Error message if the step failed to execute.
    pub error: Option<String>,
    pub assertions_passed: usize,
    pub assertions_failed: usize,
    /// Human-readable log lines from scripts (`apt.echo`) plus the runner.
    pub logs: Vec<String>,
    /// (name, value) pairs extracted by this step. Reported here for the UI
    /// to show in the run report.
    pub extracted: Vec<(String, String)>,
    /// Nested results (for If/Loop body steps).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<TestStepResult>,
}

/// Full result of executing one [`TestCase`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestRunResult {
    pub case_id: String,
    pub case_name: String,
    pub started_at: i64,
    pub finished_at: i64,
    pub total_ms: u64,
    pub steps: Vec<TestStepResult>,
    /// True if every step passed (or `continue_on_error` applied and no
    /// hard failure occurred).
    pub passed: bool,
    /// Iteration number (1-based). Relevant when `iteration_count > 1`.
    #[serde(default = "default_iteration_one")]
    pub iteration: u32,
}

fn default_iteration_one() -> u32 {
    1
}

fn default_true() -> bool {
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ids_unique() {
        let a = TestCase::new("a");
        let b = TestCase::new("b");
        assert_ne!(a.id, b.id);
        let s1 = TestCaseStep::new_request("r1");
        let s2 = TestCaseStep::new_request("r1");
        assert_ne!(s1.id(), s2.id());
    }

    #[test]
    fn defaults() {
        let c = TestCase::new("t");
        assert_eq!(c.iteration_count, 1);
        assert!(c.steps.is_empty());
        let s = RequestStep::new("req-id");
        assert!(s.inherit_request_scripts);
        assert!(!s.continue_on_error);
    }

    #[test]
    fn serde_roundtrip_request() {
        let mut suite = TestSuite::new("suite 1");
        let mut case = TestCase::new("case 1");
        case.steps.push(TestCaseStep::Request(RequestStep {
            id: new_id(),
            request_id: "r1".into(),
            extractors: vec![VariableExtractor {
                name: "token".into(),
                source: ExtractSource::JsonPath,
                path: "data.token".into(),
            }],
            ..Default::default()
        }));
        suite.test_cases.push(case);

        let s = serde_json::to_string(&suite).unwrap();
        let back: TestSuite = serde_json::from_str(&s).unwrap();
        assert_eq!(back.test_cases.len(), 1);
        let step = &back.test_cases[0].steps[0];
        match step {
            TestCaseStep::Request(r) => {
                assert_eq!(r.extractors[0].path, "data.token");
            }
            _ => panic!("expected Request step"),
        }
        assert_eq!(back.test_cases[0].iteration_count, 1);
    }

    #[test]
    fn serde_roundtrip_control_steps() {
        let mut case = TestCase::new("case 2");
        case.steps.push(TestCaseStep::new_wait(500));
        case.steps
            .push(TestCaseStep::new_if("response.status === 200"));
        case.steps.push(TestCaseStep::new_repeat(3));
        case.steps
            .push(TestCaseStep::new_for_each("item", "response.json.items"));
        case.steps.push(TestCaseStep::new_while("i < 10"));
        case.steps.push(TestCaseStep::new_script("apt.log('hi');"));

        let s = serde_json::to_string(&case).unwrap();
        let back: TestCase = serde_json::from_str(&s).unwrap();
        assert_eq!(back.steps.len(), 6);
        assert!(matches!(back.steps[0], TestCaseStep::Wait(_)));
        assert!(matches!(back.steps[1], TestCaseStep::If(_)));
        assert!(matches!(back.steps[2], TestCaseStep::Loop(_)));
        assert!(matches!(back.steps[3], TestCaseStep::Loop(_)));
        assert!(matches!(back.steps[4], TestCaseStep::Loop(_)));
        assert!(matches!(back.steps[5], TestCaseStep::Script(_)));
    }
}
