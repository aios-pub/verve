//! Mock rule management and matching logic.
//!
//! The actual HTTP serving is handled by the unified share server in
//! `src/share/server.rs`, which serves both docs and mock responses on the
//! same port (default 3097).
//!
//! Matching dimensions (in priority order):
//! 1. HTTP method (if `match_method` is set on a rule it must equal the request)
//! 2. Path, evaluated as Exact → Prefix → Regex; Exact wins over Prefix wins over Regex
//! 3. Query parameters in `match_query` must be present; non-empty value must equal
//! 4. Headers in `match_headers` must be present; non-empty value must equal
//!
//! When multiple rules could match, the highest-priority one (Exact > Prefix > Regex,
//! then earliest in declaration order) wins.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use regex::Regex;

use crate::state::models::{KeyValue, MockRule, PathPattern, Project, RequestMethod};

/// Default Mock server port (now unified with share server port 3097).
pub const DEFAULT_PORT: u16 = crate::share::server::DEFAULT_PORT;

/// Trait abstracting over request representations for rule matching, so the
/// same matching logic works for both the standalone legacy server and the
/// integrated share server.
pub trait MockRequestLike {
    fn method(&self) -> &str;
    fn path(&self) -> &str;
    fn query(&self) -> &HashMap<String, String>;
    fn headers(&self) -> &HashMap<String, String>;
}

/// A single rule entry with priority pre-computed so we can sort candidates.
#[derive(Clone)]
pub struct RuleEntry {
    pub(crate) rule: MockRule,
    /// Lower = higher priority. Exact=0, Prefix=1, Regex=2.
    priority: u8,
    /// The pre-resolved match value (the Exact/Prefix/Regex pattern string).
    pattern: String,
    /// Compiled regex if `priority == 2`.
    regex: Option<Regex>,
    /// Name of the request this rule was attached to (for logging).
    #[allow(dead_code)]
    name: String,
}

/// Build the rule lookup table from a project. Each rule carries its compiled
/// regex (if any) and a priority score.
///
/// Rules whose `match_path` is an empty Exact get backfilled from the request's
/// URL path (preserving the legacy v0.1 behavior where the path was derived from
/// the URL automatically).
pub fn rule_map(project: &Project) -> Arc<Vec<RuleEntry>> {
    let mut entries = Vec::new();
    let mut collect = |name: String, url: String, mock: &MockRule| {
        if !mock.enabled {
            return;
        }
        let mut rule = mock.clone();
        // Backfill empty path pattern from the request's URL path (legacy behavior).
        if let PathPattern::Exact(s) = &rule.match_path {
            if s.is_empty() {
                if let Some(auto) = path_of(&url) {
                    rule.match_path = PathPattern::Exact(auto);
                }
            }
        }
        let (priority, pattern, regex) = match &rule.match_path {
            PathPattern::Exact(s) => (0u8, s.clone(), None),
            PathPattern::Prefix(s) => (1u8, s.clone(), None),
            PathPattern::Regex(s) => {
                let anchored = format!("^(?:{s})$");
                match Regex::new(&anchored) {
                    Ok(r) => (2u8, s.clone(), Some(r)),
                    Err(e) => {
                        log::warn!("mock: bad regex {s:?}: {e}; skipping rule");
                        return;
                    }
                }
            }
        };
        entries.push(RuleEntry {
            rule,
            priority,
            pattern,
            regex,
            name,
        });
    };
    for req in &project.requests {
        if let Some(m) = req.mock.as_ref() {
            collect(req.name.clone(), req.url.clone(), m);
        }
    }
    for folder in &project.folders {
        walk_folder(folder, &mut collect);
    }
    // Stable sort: lower priority first; keep source order within a tier.
    entries.sort_by_key(|e| e.priority);
    Arc::new(entries)
}

fn walk_folder(
    folder: &crate::state::models::Folder,
    collect: &mut impl FnMut(String, String, &MockRule),
) {
    for req in &folder.requests {
        if let Some(m) = req.mock.as_ref() {
            collect(req.name.clone(), req.url.clone(), m);
        }
    }
    for sub in &folder.folders {
        walk_folder(sub, collect);
    }
}

/// Extract the URL path from a (possibly templated) URL string.
pub(crate) fn path_of(url: &str) -> Option<String> {
    let url = if url.starts_with("{{") {
        if let Some(end) = url.find("}}") {
            &url[end + 2..]
        } else {
            url
        }
    } else {
        url
    };
    let url = if let Some(idx) = url.find("://") {
        let rest = &url[idx + 3..];
        match rest.find('/') {
            Some(p) => &rest[p..],
            None => return None,
        }
    } else {
        url
    };
    let path = url.split('?').next().unwrap_or(url);
    if path.is_empty() {
        None
    } else {
        Some(path.to_string())
    }
}

/// Walk every request in the project and synthesize a default MockRule for any
/// that don't have one yet (200, Content-Type: application/json, empty body `{}`),
/// returning the list of (request_id, new_rule) pairs so the caller can persist
/// them onto the project. Requests that already have a mock rule are skipped.
pub fn generate_missing(project: &Project) -> Vec<(String, MockRule)> {
    let mut out = Vec::new();
    let mut walk = |req: &crate::state::models::ApiRequest| {
        if req.mock.is_some() {
            return;
        }
        let rule = MockRule {
            enabled: true,
            status: 200,
            headers: vec![KeyValue::new("Content-Type", "application/json")],
            body: "{}".to_string(),
            delay_ms: 0,
            match_method: None,
            match_path: PathPattern::Exact(path_of(&req.url).unwrap_or_else(|| "/".into())),
            match_query: Vec::new(),
            match_headers: Vec::new(),
            enable_templates: false,
        };
        out.push((req.id.clone(), rule));
    };
    for r in &project.requests {
        walk(r);
    }
    for f in &project.folders {
        walk_folder_req(f, &mut walk);
    }
    out
}

fn walk_folder_req<F: FnMut(&crate::state::models::ApiRequest)>(
    folder: &crate::state::models::Folder,
    walk: &mut F,
) {
    for r in &folder.requests {
        walk(r);
    }
    for s in &folder.folders {
        walk_folder_req(s, walk);
    }
}

/// Shared, swap-able rule set. The mock server re-reads this on every request
/// so UI actions (e.g. "一键生成 Mock", toggling a rule) take effect without
/// a restart.
pub type SharedRules = Arc<RwLock<Arc<Vec<RuleEntry>>>>;

/// Build a SharedRules handle from a rule snapshot.
pub fn shared_rules(entries: Vec<RuleEntry>) -> SharedRules {
    Arc::new(RwLock::new(Arc::new(entries)))
}

/// Atomically replace the rules served by a running mock server.
pub fn swap_rules(shared: &SharedRules, entries: Vec<RuleEntry>) {
    if let Ok(mut guard) = shared.write() {
        *guard = Arc::new(entries);
    }
}

/// Extract a current snapshot of the rules from a SharedRules handle.
pub fn current_rules(shared: &SharedRules) -> Arc<Vec<RuleEntry>> {
    shared.read().map(|g| g.clone()).unwrap_or_default()
}

/// Percent-decode a URL component (query param / path segment). Minimal impl
/// sufficient for mock match keys.
pub(crate) fn url_decode(s: &str) -> String {
    // Minimal percent decode — sufficient for mock match keys (JSON tokens, simple
    // ids). Full urlencoding is overkill here.
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Ok(b) =
                u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
            {
                out.push(b);
                i += 3;
                continue;
            }
        }
        if bytes[i] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[i]);
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub(crate) fn rule_matches_for(entry: &RuleEntry, req: &dyn MockRequestLike) -> bool {
    // Method.
    if let Some(m) = &entry.rule.match_method {
        if req.method().to_ascii_uppercase() != m.as_str() {
            return false;
        }
    }
    // Path.
    let path_match = match &entry.rule.match_path {
        PathPattern::Exact(s) => req.path() == *s,
        PathPattern::Prefix(s) => req.path().starts_with(s.as_str()),
        PathPattern::Regex(_) => entry
            .regex
            .as_ref()
            .map(|r| r.is_match(req.path()))
            .unwrap_or(false),
    };
    if !path_match {
        return false;
    }
    // Query.
    for kv in &entry.rule.match_query {
        if !kv.enabled || kv.key.is_empty() {
            continue;
        }
        let present = req.query().get(&kv.key.to_ascii_lowercase());
        match present {
            Some(v) if !kv.value.is_empty() && v != &kv.value => return false,
            None => return false,
            _ => {}
        }
    }
    // Headers.
    for kv in &entry.rule.match_headers {
        if !kv.enabled || kv.key.is_empty() {
            continue;
        }
        let present = req.headers().get(&kv.key.to_ascii_lowercase());
        match present {
            Some(v) if !kv.value.is_empty() && v != &kv.value => return false,
            None => return false,
            _ => {}
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::models::RequestMethod;

    fn rule_exact(path: &str) -> MockRule {
        MockRule {
            match_path: PathPattern::Exact(path.into()),
            ..Default::default()
        }
    }
    fn rule_prefix(path: &str) -> MockRule {
        MockRule {
            match_path: PathPattern::Prefix(path.into()),
            ..Default::default()
        }
    }
    fn rule_regex(pattern: &str) -> MockRule {
        MockRule {
            match_path: PathPattern::Regex(pattern.into()),
            ..Default::default()
        }
    }

    struct TestRequest {
        method: String,
        path: String,
        query: HashMap<String, String>,
        headers: HashMap<String, String>,
    }

    impl MockRequestLike for TestRequest {
        fn method(&self) -> &str {
            &self.method
        }
        fn path(&self) -> &str {
            &self.path
        }
        fn query(&self) -> &HashMap<String, String> {
            &self.query
        }
        fn headers(&self) -> &HashMap<String, String> {
            &self.headers
        }
    }

    fn req(method: &str, path: &str) -> TestRequest {
        let (p, q) = match path.find('?') {
            Some(i) => (path[..i].to_string(), &path[i + 1..]),
            None => (path.to_string(), ""),
        };
        let mut query = HashMap::new();
        for pair in q.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            query.insert(k.to_ascii_lowercase(), v.to_string());
        }
        TestRequest {
            method: method.into(),
            path: p,
            query,
            headers: HashMap::new(),
        }
    }

    #[test]
    fn path_extraction() {
        assert_eq!(path_of("{{baseUrl}}/api/login"), Some("/api/login".into()));
        assert_eq!(path_of("https://x.test/users/1"), Some("/users/1".into()));
        assert_eq!(path_of("{{baseUrl}}/items?detail=1"), Some("/items".into()));
    }

    #[test]
    fn exact_matches_only_exact_path() {
        let e = RuleEntry {
            rule: rule_exact("/a"),
            priority: 0,
            pattern: "/a".into(),
            regex: None,
            name: "t".into(),
        };
        assert!(rule_matches_for(&e, &req("GET", "/a")));
        assert!(!rule_matches_for(&e, &req("GET", "/a/b")));
    }

    #[test]
    fn prefix_matches_subpaths() {
        let e = RuleEntry {
            rule: rule_prefix("/api"),
            priority: 1,
            pattern: "/api".into(),
            regex: None,
            name: "t".into(),
        };
        assert!(rule_matches_for(&e, &req("GET", "/api")));
        assert!(rule_matches_for(&e, &req("GET", "/api/users")));
        assert!(!rule_matches_for(&e, &req("GET", "/other")));
    }

    #[test]
    fn regex_matches_pattern() {
        let e = RuleEntry {
            rule: rule_regex(r"/api/users/\d+"),
            priority: 2,
            pattern: r"/api/users/\d+".into(),
            regex: Some(Regex::new("^(?:/api/users/\\d+)$").unwrap()),
            name: "t".into(),
        };
        assert!(rule_matches_for(&e, &req("GET", "/api/users/42")));
        assert!(!rule_matches_for(&e, &req("GET", "/api/users/abc")));
    }

    #[test]
    fn method_filter_works() {
        let mut r = rule_exact("/a");
        r.match_method = Some(RequestMethod::Post);
        let e = RuleEntry {
            rule: r,
            priority: 0,
            pattern: "/a".into(),
            regex: None,
            name: "t".into(),
        };
        assert!(!rule_matches_for(&e, &req("GET", "/a")));
        assert!(rule_matches_for(&e, &req("POST", "/a")));
    }

    #[test]
    fn query_match_requires_presence_and_value() {
        let mut r = rule_exact("/a");
        r.match_query = vec![KeyValue::new("debug", "1")];
        let e = RuleEntry {
            rule: r,
            priority: 0,
            pattern: "/a".into(),
            regex: None,
            name: "t".into(),
        };
        assert!(!rule_matches_for(&e, &req("GET", "/a")));
        assert!(rule_matches_for(&e, &req("GET", "/a?debug=1")));
        assert!(!rule_matches_for(&e, &req("GET", "/a?debug=2")));
    }

    #[test]
    fn generate_missing_creates_rules() {
        let mut p = Project::new("t");
        let mut r1 = crate::state::models::ApiRequest::new("a", RequestMethod::Get, "/api/a");
        r1.mock = None;
        p.requests.push(r1);
        let mut r2 = crate::state::models::ApiRequest::new("b", RequestMethod::Get, "/api/b");
        r2.mock = Some(MockRule {
            enabled: false,
            ..Default::default()
        });
        p.requests.push(r2);
        let generated = generate_missing(&p);
        assert_eq!(
            generated.len(),
            1,
            "only the request without a rule gets one"
        );
        let (id, rule) = &generated[0];
        assert_eq!(p.requests[0].id, *id);
        assert!(rule.enabled);
        assert_eq!(rule.status, 200);
        assert_eq!(rule.body, "{}");
    }
}
