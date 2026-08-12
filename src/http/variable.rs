//! `{{variable}}` substitution.
//!
//! Variables are resolved against a scope-ordered map built by
//! [`crate::state::models::effective_variables`]. Unknown variables are left
//! as-is (including the braces) so the user can spot typos.
//!
//! In addition to user-defined variables, Postman-style *dynamic* variables
//! are supported: any name starting with `$` (e.g. `{{$random}}`,
//! `{{$uuid}}`, `{{$timestamp}}`) is expanded to a freshly generated value on
//! every call — *unless* the user has defined a variable of the same name, in
//! which case the user value wins (user scope has higher priority).

use std::collections::BTreeMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Replace every `{{name}}` occurrence in `input` with the matching value.
pub fn substitute(input: &str, vars: &BTreeMap<String, String>) -> String {
    let mut out = String::with_capacity(input.len());
    let bytes = input.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'{' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = find_close(&bytes[i + 2..]) {
                let name = input[i + 2..i + 2 + end].trim();
                // User-defined variables take priority over dynamic ones so a
                // user can override e.g. {{$timestamp}} with a fixed value.
                match vars.get(name) {
                    Some(val) => out.push_str(val),
                    None => {
                        if let Some(dynamic) = dynamic_variable(name) {
                            out.push_str(&dynamic);
                        } else {
                            out.push_str("{{");
                            out.push_str(name);
                            out.push_str("}}");
                        }
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

/// Postman-style dynamic variables. Recognised names are:
/// - `{{$random}}`    → 10-char alphanumeric string (A-Za-z0-9), new each call.
///   Useful for unique-per-request headers like `x-request-id`.
/// - `{{$uuid}}`      → a lowercase UUID v4 string.
/// - `{{$sparkid}}`   → a 21-char Base58, time-sortable id (sparkid). Shorter
///   than a UUID and lexicographically ordered; handy when a backend expects a
///   compact, sortable identifier.
/// - `{{$timestamp}}` → Unix seconds since the epoch.
///
/// Returns `None` for anything else (left intact by `substitute`).
fn dynamic_variable(name: &str) -> Option<String> {
    match name {
        "$random" => Some(random_alphanumeric(10)),
        "$uuid" => Some(uuid::Uuid::new_v4().to_string()),
        "$sparkid" => Some(sparkid::SparkId::new().to_string()),
        "$timestamp" => Some(current_unix_seconds()),
        _ => None,
    }
}

/// Current Unix time in seconds (defensive: falls back to "0" if the clock is
/// before the epoch, which never happens in practice).
fn current_unix_seconds() -> String {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_else(|_| "0".to_string())
}

/// Generate an `n`-character alphanumeric string (A-Z a-z 0-9) without pulling
/// in a `rand` dependency. Seeds from high-resolution wall-clock nanos and
/// mixes each draw through a splitmix64 step; this is **not** cryptographically
/// secure — its purpose is producing distinct, human-friendly id values for
/// debugging/tracing headers (matching Postman `$random`'s intent).
fn random_alphanumeric(n: usize) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
    // Seed: nanos since epoch. Combining with an internal counter is not needed
    // because nanosecond resolution already differentiates rapid successive
    // calls well enough for this use case.
    let mut state = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x9E3779B97F4A7C15);
    let mut buf = String::with_capacity(n);
    for _ in 0..n {
        // splitmix64 — good bit mixing, cheap, dependency-free.
        state = state.wrapping_add(0x9E3779B97F4A7C15);
        let mut z = state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        z ^= z >> 31;
        // Index into the 62-char alphabet; modulus is safe (ALPHABET is non-empty).
        let idx = (z % ALPHABET.len() as u64) as usize;
        // ALPHABET is ASCII, so pushing a byte as a char is boundary-safe.
        buf.push(ALPHABET[idx] as char);
    }
    buf
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

    #[test]
    fn dynamic_random_is_10_alphanumeric() {
        let m = vars(&[]);
        let v = substitute("{{$random}}", &m);
        assert_eq!(v.len(), 10);
        assert!(v.chars().all(|c| c.is_ascii_alphanumeric()));
        // Two calls should differ (nanosecond seed makes collisions vanishingly
        // rare). On the off chance they collide, retry a couple of times before
        // failing so the test is robust rather than flaky.
        let mut differ = false;
        for _ in 0..5 {
            let v2 = substitute("{{$random}}", &m);
            if v2 != v {
                differ = true;
                break;
            }
        }
        assert!(differ, "{{$random}} produced the same value repeatedly");
    }

    #[test]
    fn dynamic_uuid_is_valid_v4() {
        let m = vars(&[]);
        let v = substitute("{{$uuid}}", &m);
        assert_eq!(v.len(), 36, "expected canonical UUID length");
        // Version nibble for v4 is '4' at position 14.
        let bytes = v.as_bytes();
        assert_eq!(bytes[14], b'4', "expected UUID v4 marker");
        // Hyphens at the canonical positions 8, 13, 18, 23.
        for &pos in &[8usize, 13, 18, 23] {
            assert_eq!(bytes[pos], b'-', "expected '-' at position {pos}");
        }
    }

    #[test]
    fn dynamic_sparkid_is_valid() {
        let m = vars(&[]);
        let v = substitute("{{$sparkid}}", &m);
        assert_eq!(v.len(), 21, "expected 21-char sparkid, got {v}");
        // Bitcoin Base58 alphabet (excludes 0, O, I, l to stay unambiguous).
        const BASE58: &[u8] =
            b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";
        assert!(
            v.bytes().all(|b| BASE58.contains(&b)),
            "expected Base58 chars only, got {v}"
        );
    }

    #[test]
    fn dynamic_timestamp_is_numeric() {
        let m = vars(&[]);
        let v = substitute("{{$timestamp}}", &m);
        assert!(!v.is_empty());
        assert!(
            v.chars().all(|c| c.is_ascii_digit()),
            "expected digits only"
        );
    }

    #[test]
    fn user_variable_overrides_dynamic() {
        // A user-defined $random should win over the dynamic generator.
        let m = vars(&[("$random", "fixed-value")]);
        assert_eq!(substitute("{{$random}}", &m), "fixed-value");
    }

    #[test]
    fn unknown_placeholder_still_intact() {
        let m = vars(&[]);
        assert_eq!(substitute("{{$notDynamic}}", &m), "{{$notDynamic}}");
        assert_eq!(substitute("{{normal}}", &m), "{{normal}}");
    }
}
