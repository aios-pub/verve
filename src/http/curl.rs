//! Render an executable `curl` command for a request.
//!
//! The two UI entry points (request panel "Curl" tab and the project tree's
//! "复制为 cURL" context action) both funnel through [`render`], so the
//! generated command stays consistent with the real send path
//! ([`crate::http::client::prepare`]): query params are appended to the URL
//! (never moved into a body), cookies collapse into a `Cookie` header, API-key
//! auth lands in the header or query, and bodies keep their Content-Type.
//!
//! Inputs must already be variable-substituted; this module only renders.

use crate::state::models::{AuthConfig, AuthTarget, AuthType, RequestMethod};

/// One `-F` form part: a text field or a file upload.
pub enum CurlFormPart {
    Field { name: String, value: String },
    File { name: String, path: String },
}

/// The body of the generated command. Empty bodies must be expressed as
/// `CurlBody::None` so no `-d`/`-F` (and no auto Content-Type) is emitted.
pub enum CurlBody {
    None,
    /// Raw text body with the language's Content-Type (e.g. `application/json`).
    Raw { text: String, content_type: String },
    /// `application/x-www-form-urlencoded` pairs.
    Urlencoded(Vec<(String, String)>),
    /// `multipart/form-data` parts; curl derives the Content-Type itself.
    Form(Vec<CurlFormPart>),
}

/// A fully-resolved snapshot of a request, ready to render.
pub struct CurlSpec {
    pub method: RequestMethod,
    /// URL after substitution + base_url join + scheme normalize, WITHOUT the
    /// appended query params (they come from `params`).
    pub url: String,
    pub params: Vec<(String, String)>,
    pub headers: Vec<(String, String)>,
    pub cookies: Vec<(String, String)>,
    pub auth: AuthConfig,
    pub body: CurlBody,
}

/// Render the curl command as a shell-ready multi-line string.
pub fn render(spec: &CurlSpec) -> String {
    let mut parts: Vec<String> = vec!["curl".into(), format!("-X {}", spec.method)];

    // --- URL: append query params (form-encoded), then API-key-in-query. ---
    let mut params = spec.params.clone();
    if spec.auth.auth_type == AuthType::ApiKey
        && spec.auth.add_to == AuthTarget::Query
        && !spec.auth.key.trim().is_empty()
    {
        params.push((spec.auth.key.clone(), spec.auth.value.clone()));
    }
    let final_url = match build_query(&spec.url, &params) {
        Some(q) => q,
        None => spec.url.clone(),
    };
    parts.push(shell_quote(&final_url));

    // --- Headers. Track Content-Type so a body doesn't override the user's. ---
    let mut has_content_type = false;
    for (k, v) in &spec.headers {
        if k.eq_ignore_ascii_case("content-type") {
            has_content_type = true;
        }
        parts.push(format!("-H {}", shell_quote(&format!("{k}: {v}"))));
    }

    // --- Cookies → single Cookie header (mirrors prepare()). ---
    let has_cookie_header = spec
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("cookie"));
    if !spec.cookies.is_empty() && !has_cookie_header {
        let joined = spec
            .cookies
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("; ");
        parts.push(format!("-H {}", shell_quote(&format!("Cookie: {joined}"))));
    }

    // --- Auth (mirrors prepare(): never override an existing header). ---
    let has_auth_header = spec
        .headers
        .iter()
        .any(|(k, _)| k.eq_ignore_ascii_case("authorization"));
    match spec.auth.auth_type {
        AuthType::Bearer => {
            if !spec.auth.token.is_empty() && !has_auth_header {
                parts.push(format!(
                    "-H {}",
                    shell_quote(&format!("Authorization: Bearer {}", spec.auth.token))
                ));
            }
        }
        AuthType::Basic => {
            if !spec.auth.username.is_empty() {
                parts.push(format!(
                    "-u {}",
                    shell_quote(&format!("{}:{}", spec.auth.username, spec.auth.password))
                ));
            }
        }
        AuthType::ApiKey
            if spec.auth.add_to == AuthTarget::Header
                && !spec.auth.key.trim().is_empty()
                && !spec
                    .headers
                    .iter()
                    .any(|(k, _)| k.eq_ignore_ascii_case(&spec.auth.key)) =>
        {
            parts.push(format!(
                "-H {}",
                shell_quote(&format!("{}: {}", spec.auth.key, spec.auth.value))
            ));
        }
        _ => {}
    }

    // --- Body. `--data-raw` (not `-d`) so a body starting with `@` is sent
    //     literally instead of being read as a file. ---
    match &spec.body {
        CurlBody::None => {}
        CurlBody::Raw { text, content_type } => {
            if !has_content_type {
                parts.push(format!(
                    "-H {}",
                    shell_quote(&format!("Content-Type: {content_type}"))
                ));
            }
            parts.push(format!("--data-raw {}", shell_quote(text)));
        }
        // Empty pair/form lists render nothing (guards against trailing
        // empty kv rows producing `-d ''`).
        CurlBody::Urlencoded(pairs) if !pairs.is_empty() => {
            if !has_content_type {
                parts.push(format!(
                    "-H {}",
                    shell_quote("Content-Type: application/x-www-form-urlencoded")
                ));
            }
            parts.push(format!("--data-raw {}", shell_quote(&encode_form_pairs(pairs))));
        }
        CurlBody::Form(items) if !items.is_empty() => {
            for item in items {
                let part = match item {
                    CurlFormPart::Field { name, value } => {
                        format!("{name}={value}")
                    }
                    // `@` + quoted path: curl reads and uploads the file.
                    CurlFormPart::File { name, path } => {
                        format!("{name}=@\"{}\"", path.replace('"', "\\\""))
                    }
                };
                parts.push(format!("-F {}", shell_quote(&part)));
            }
            // curl sets multipart/form-data (with boundary) for -F itself.
        }
        _ => {}
    }

    parts.join(" \\\n  ")
}

/// Append `params` to `url` as a percent-encoded query string. Returns `None`
/// when there is nothing to append.
fn build_query(url: &str, params: &[(String, String)]) -> Option<String> {
    if params.is_empty() {
        return None;
    }
    let encoded = encode_form_pairs(params);
    if encoded.is_empty() {
        return None;
    }
    let sep = if url.contains('?') { "&" } else { "?" };
    Some(format!("{url}{sep}{encoded}"))
}

/// `application/x-www-form-urlencoded`-encode pairs (`k=v&k=v`), matching the
/// real send path's `serde_urlencode`.
fn encode_form_pairs(pairs: &[(String, String)]) -> String {
    pairs
        .iter()
        .map(|(k, v)| {
            format!(
                "{}={}",
                form_component(k),
                form_component(v)
            )
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// Percent-encode one query/form component.
fn form_component(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Quote for POSIX shells: wrap in single quotes, escaping embedded quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(method: RequestMethod, url: &str) -> CurlSpec {
        CurlSpec {
            method,
            url: url.to_string(),
            params: Vec::new(),
            headers: Vec::new(),
            cookies: Vec::new(),
            auth: AuthConfig::default(),
            body: CurlBody::None,
        }
    }

    #[test]
    fn get_params_go_to_url_not_body() {
        let mut s = spec(RequestMethod::Get, "https://api.io/users");
        s.params = vec![
            ("page".into(), "1".into()),
            ("q".into(), "a b&c=2".into()),
        ];
        let out = render(&s);
        assert!(out.contains("-X GET"), "method flag expected: {out}");
        assert!(
            out.contains("'https://api.io/users?page=1&q=a+b%26c%3D2'"),
            "params must be url-encoded onto the URL: {out}"
        );
        assert!(!out.contains("-d"), "GET without body must have no -d: {out}");
        assert!(!out.contains("--data-raw"), "{out}");
    }

    #[test]
    fn get_with_body_keeps_get_method() {
        let mut s = spec(RequestMethod::Get, "https://api.io/ping");
        s.body = CurlBody::Raw {
            text: "{\"a\":1}".into(),
            content_type: "application/json".into(),
        };
        let out = render(&s);
        // Without -X GET, curl would turn a --data-raw request into POST.
        assert!(out.contains("-X GET"), "{out}");
        assert!(out.contains("--data-raw '{\"a\":1}'"), "{out}");
        assert!(out.contains("Content-Type: application/json"), "{out}");
    }

    #[test]
    fn existing_query_string_uses_ampersand() {
        let mut s = spec(RequestMethod::Get, "https://api.io/x?flag=1");
        s.params = vec![("k".into(), "v".into())];
        let out = render(&s);
        assert!(out.contains("'https://api.io/x?flag=1&k=v'"), "{out}");
    }

    #[test]
    fn api_key_query_auth_appended_to_url() {
        let mut s = spec(RequestMethod::Get, "https://api.io/x");
        s.auth = AuthConfig {
            auth_type: AuthType::ApiKey,
            key: "token".into(),
            value: "abc".into(),
            add_to: AuthTarget::Query,
            ..AuthConfig::default()
        };
        let out = render(&s);
        assert!(out.contains("'https://api.io/x?token=abc'"), "{out}");
    }

    #[test]
    fn cookies_collapse_into_single_header() {
        let mut s = spec(RequestMethod::Get, "https://api.io/x");
        s.cookies = vec![("a".into(), "1".into()), ("b".into(), "2".into())];
        let out = render(&s);
        assert!(out.contains("-H 'Cookie: a=1; b=2'"), "{out}");
    }

    #[test]
    fn empty_urlencoded_body_emits_nothing() {
        let mut s = spec(RequestMethod::Get, "https://api.io/x");
        s.body = CurlBody::Urlencoded(Vec::new());
        let out = render(&s);
        assert!(!out.contains("--data-raw"), "{out}");
        assert!(
            !out.contains("x-www-form-urlencoded"),
            "no CT without a body: {out}"
        );
    }

    #[test]
    fn single_quotes_in_values_are_escaped() {
        let mut s = spec(RequestMethod::Post, "https://api.io/x");
        s.body = CurlBody::Raw {
            text: "it's".into(),
            content_type: "text/plain".into(),
        };
        let out = render(&s);
        assert!(out.contains("--data-raw 'it'\\''s'"), "{out}");
    }

    #[test]
    fn user_content_type_is_not_duplicated() {
        let mut s = spec(RequestMethod::Post, "https://api.io/x");
        s.headers = vec![("Content-Type".into(), "text/csv".into())];
        s.body = CurlBody::Raw {
            text: "a,b".into(),
            content_type: "application/json".into(),
        };
        let out = render(&s);
        assert_eq!(out.matches("Content-Type").count(), 1, "{out}");
        assert!(out.contains("-H 'Content-Type: text/csv'"), "{out}");
    }

    #[test]
    fn form_file_part_uses_at_path() {
        let mut s = spec(RequestMethod::Post, "https://api.io/up");
        s.body = CurlBody::Form(vec![
            CurlFormPart::Field {
                name: "note".into(),
                value: "hi".into(),
            },
            CurlFormPart::File {
                name: "file".into(),
                path: "/tmp/my file.png".into(),
            },
        ]);
        let out = render(&s);
        assert!(out.contains("-F 'note=hi'"), "{out}");
        assert!(out.contains("-F 'file=@\"/tmp/my file.png\"'"), "{out}");
        assert!(!out.contains("multipart"), "curl sets it itself: {out}");
    }

    #[test]
    fn chinese_params_are_percent_encoded() {
        let mut s = spec(RequestMethod::Get, "https://api.io/search");
        s.params = vec![("kw".into(), "中文".into())];
        let out = render(&s);
        assert!(
            out.contains("'https://api.io/search?kw=%E4%B8%AD%E6%96%87'"),
            "{out}"
        );
    }
}
