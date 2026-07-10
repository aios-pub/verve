//! `{{variable}}` substitution.
//!
//! Variables are resolved against a scope-ordered map built by
//! [`crate::state::models::effective_variables`]. Unknown variables are left
//! as-is (including the braces) so the user can spot typos.

use std::collections::BTreeMap;

/// Replace every `{{name}}` occurrence in `input` with the matching value.
pub fn substitute(input: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = find_close(&bytes[i + 2..]) {
                let name = input[i + 2..i + 2 + end].trim();
                match vars.get(name) {
                    Some(val) => out.push_str(val),
                    None => {
                        out.push_str("{{");
                        out.push_str(name);
                        out.push_str("}}");
                    }
                }
                i += 2 + end + 2;
                continue;
            }
        }
        // safe because '{' and normal chars are valid UTF-8 boundaries here
        let ch = input[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

/// Find the offset of the closing `}}` relative to the start of the slice.
fn find_close(s: &[u8]) -> Option<usize> {
    let mut i = 0;
    while i + 1 < s.len() {
        if s[i] == b'}' && s[i + 1] == b'}' {
            return Some(i);
        }
        i += 1;
    }
    None
}

/// Collect all `{{name}}` placeholders found in `input`, in order of appearance.
pub fn collect_placeholders(input: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = find_close(&bytes[i + 2..]) {
                let name = input[i + 2..i + 2 + end].trim().to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
                i += 2 + end + 2;
                continue;
            }
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn substitutes_known() {
        let m = vars(&[("baseUrl", "https://x.test"), ("id", "42")]);
        assert_eq!(
            substitute("{{baseUrl}}/api/{{id}}", &m),
            "https://x.test/api/42"
        );
    }

    #[test]
    fn leaves_unknown_intact() {
        let m = vars(&[]);
        assert_eq!(substitute("{{missing}}", &m), "{{missing}}");
    }

    #[test]
    fn collects_unique_in_order() {
        assert_eq!(
            collect_placeholders("{{a}}{{b}}{{a}} {{ c }}"),
            vec!["a".to_string(), "b".to_string(), "c".to_string()]
        );
    }
}
