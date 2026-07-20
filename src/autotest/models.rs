//! Data models for automated test suites.
//!
//! Mirrors Postman collection-runner concepts:
//!
//! - [`TestSuite`]  — a named collection of related test cases.
//! - [`TestCase`]   — one executable scenario: a sequence of ordered steps.
//! - [`TestCaseStep`] — one step: references an existing [`ApiRequest`] by id,
//!   optionally overrides its pre/post scripts, and declares variable
//!   extractors that feed subsequent steps.
//! - [`TestRunResult`] / [`TestStepResult`] — execution reports produced by
//!   the runner.
//!
//! All models are `Serialize`/`Deserialize` and persist into
//! `workspace.json` under `Project::test_suites`.

use serde::{Deserialize, Serialize};

use crate::state::models::{new_id, KeyValue};

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

/// One step in a test case. References an existing `ApiRequest` by id; the
/// runner executes that request with optional script overrides and extracts
/// variables from the response for downstream steps.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TestCaseStep {
    pub id: String,
    /// Display name override (defaults to the target request's name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_override: Option<String>,
    /// The `ApiRequest::id` this step executes.
    pub request_id: String,
    /// Pre-request script override. If empty, the request's own pre_request
    /// script is used (set [`TestCaseStep::inherit_request_scripts`] = false
    /// to suppress it entirely).
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

fn default_true() -> bool {
    true
}

impl TestCaseStep {
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
pub enum ExtractSource {
    /// Dot-path into the JSON response body (e.g. `data.token`).
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

impl Default for ExtractSource {
    fn default() -> Self {
        ExtractSource::JsonPath
    }
}

/// Result of a single step within a test run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestStepResult {
    pub step_id: String,
    /// Resolved display name (override or request name).
    pub request_name: String,
    pub status: u16,
    pub passed: bool,
    pub time_ms: u64,
    /// Error message if the request/script failed to execute.
    pub error: Option<String>,
    pub assertions_passed: usize,
    pub assertions_failed: usize,
    /// Human-readable log lines from scripts (`apt.echo`) plus the runner.
    pub logs: Vec<String>,
    /// (name, value) pairs extracted by this step. Reported here for the UI
    /// to show in the run report.
    pub extracted: Vec<(String, String)>,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ids_unique() {
        let a = TestCase::new("a");
        let b = TestCase::new("b");
        assert_ne!(a.id, b.id);
        let s1 = TestCaseStep::new("r1");
        let s2 = TestCaseStep::new("r1");
        assert_ne!(s1.id, s2.id);
    }

    #[test]
    fn defaults() {
        let c = TestCase::new("t");
        assert_eq!(c.iteration_count, 1);
        assert!(c.steps.is_empty());
        let s = TestCaseStep::new("req-id");
        assert!(s.inherit_request_scripts);
        assert!(!s.continue_on_error);
    }

    #[test]
    fn serde_roundtrip() {
        let mut suite = TestSuite::new("suite 1");
        let mut case = TestCase::new("case 1");
        case.steps.push(TestCaseStep {
            id: new_id(),
            request_id: "r1".into(),
            extractors: vec![VariableExtractor {
                name: "token".into(),
                source: ExtractSource::JsonPath,
                path: "data.token".into(),
            }],
            ..Default::default()
        });
        suite.test_cases.push(case);

        let s = serde_json::to_string(&suite).unwrap();
        let back: TestSuite = serde_json::from_str(&s).unwrap();
        assert_eq!(back.test_cases.len(), 1);
        assert_eq!(back.test_cases[0].steps[0].extractors[0].path, "data.token");
        assert_eq!(back.test_cases[0].iteration_count, 1);
    }
}
