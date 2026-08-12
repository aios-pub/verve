//! Data models for the document-sharing system (postman-style).
//!
//! A [`ShareConfig`] is the complete recipe for one shareable document: which
//! scope (whole project / a single request / a folder), how long it stays
//! valid, whether it's password-protected, which environment's variables to
//! inject, which fields to render, and the document logo. Configs persist to
//! `shares.json` (cross-workspace, git-ignored) and are enforced strictly by
//! the local HTTP server in [`super::server`].

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::state::models::{id_suffix, new_id};

/// One document share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShareConfig {
    /// Short id used in the share URL: `/s/<id>`.
    pub id: String,
    /// Project the share was created from.
    pub project_id: String,
    /// Snapshot of the project name (projects may be renamed/deleted later).
    pub project_name: String,
    /// What the share covers.
    #[serde(default)]
    pub scope: ShareScope,
    /// Target request/folder id when `scope` is `Request`/`Folder`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_id: Option<String>,
    /// Snapshot of the target's name (request/folder), for display.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_name: Option<String>,
    /// Document title shown in the viewer header.
    pub title: String,
    /// Creation time, Unix seconds.
    pub created_at: i64,
    /// Validity window.
    #[serde(default)]
    pub expire: Expiration,
    /// Access control (public / password).
    #[serde(default)]
    pub access: AccessControl,
    /// Environment whose variables get injected when rendering the docs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_id: Option<String>,
    /// Per-field visibility toggles ("字段展示控制").
    #[serde(default)]
    pub field_display: FieldDisplay,
    /// How the user chose to share ("分享方式").
    #[serde(default)]
    pub share_methods: Vec<ShareMethod>,
    /// Optional document logo path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo_path: Option<PathBuf>,
    /// Visit counter (incremented server-side on each successful view).
    #[serde(default)]
    pub visits: u64,
    /// Last visit time, Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_visit: Option<i64>,
}

impl ShareConfig {
    /// Build a new share with sensible defaults and a fresh id.
    pub fn new(project_id: impl Into<String>, project_name: impl Into<String>) -> Self {
        Self {
            id: short_id(),
            project_id: project_id.into(),
            project_name: project_name.into(),
            scope: ShareScope::Project,
            target_id: None,
            target_name: None,
            title: String::new(),
            created_at: now_ts(),
            expire: Expiration::Forever,
            access: AccessControl::public(),
            environment_id: None,
            field_display: FieldDisplay::default(),
            share_methods: vec![ShareMethod::Link],
            logo_path: None,
            visits: 0,
            last_visit: None,
        }
    }

    /// Whether the share is still valid at the given Unix timestamp.
    pub fn is_valid_at(&self, now: i64) -> bool {
        match self.expire {
            Expiration::Forever => true,
            Expiration::Days(d) => {
                let secs = (d as i64).saturating_mul(86_400);
                now < self.created_at.saturating_add(secs)
            }
        }
    }

    /// Human-readable scope label, e.g. "整个项目" / "单个接口".
    pub fn scope_label(&self) -> &'static str {
        match self.scope {
            ShareScope::Project => "整个项目",
            ShareScope::Request => "单个接口",
            ShareScope::Folder => "文件夹",
        }
    }

    /// Human-readable expiration label, e.g. "永久有效" / "30 天".
    pub fn expire_label(&self) -> String {
        match self.expire {
            Expiration::Forever => "永久有效".to_string(),
            Expiration::Days(d) => format!("{d} 天"),
        }
    }

    /// Human-readable access label, e.g. "公开" / "密码".
    pub fn access_label(&self) -> &'static str {
        if self.access.public {
            "公开"
        } else {
            "密码"
        }
    }

    /// Full document title, falling back to project/target name.
    pub fn display_title(&self) -> String {
        if self.title.trim().is_empty() {
            match (&self.scope, &self.target_name) {
                (ShareScope::Request, Some(n)) | (ShareScope::Folder, Some(n)) => {
                    format!("{} · {}", self.project_name, n)
                }
                _ => self.project_name.clone(),
            }
        } else {
            self.title.clone()
        }
    }

    /// Prefix the share id with a tenant namespace for multi-tenant isolation.
    pub fn with_tenant(mut self, tenant: &str) -> Self {
        if !self.id.contains('/') {
            self.id = format!("{tenant}/{}", self.id);
        }
        self
    }
}

/// What a share covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ShareScope {
    /// Every request in the project (top-level + nested folders).
    #[default]
    Project,
    /// A single API request (`target_id`).
    Request,
    /// A folder and its subtree (`target_id`).
    Folder,
}

impl ShareScope {
    pub fn label(self) -> &'static str {
        match self {
            ShareScope::Project => "整个项目",
            ShareScope::Request => "单个接口",
            ShareScope::Folder => "文件夹",
        }
    }
}

/// Validity window for a share.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Expiration {
    /// Never expires.
    #[default]
    Forever,
    /// Expires after N days. Common values: 1/7/30/90/180/365.
    Days(u32),
}

impl Expiration {
    /// The standard preset durations shown in the dropdown.
    pub const PRESETS: &'static [(Expiration, &'static str)] = &[
        (Expiration::Forever, "永久有效"),
        (Expiration::Days(1), "1 天"),
        (Expiration::Days(7), "7 天"),
        (Expiration::Days(30), "30 天"),
        (Expiration::Days(90), "90 天"),
        (Expiration::Days(180), "180 天"),
        (Expiration::Days(365), "365 天"),
    ];

    pub fn label(self) -> &'static str {
        Self::PRESETS
            .iter()
            .find(|(e, _)| *e == self)
            .map(|(_, l)| *l)
            .unwrap_or("自定义")
    }
}

/// Access control for a share.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccessControl {
    /// `true` = public (no password). `false` = password required.
    #[serde(default = "default_true")]
    pub public: bool,
    /// The password when `public == false`. Stored in plaintext locally; this
    /// is an offline tool and the file is git-ignored + user-owned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

impl Default for AccessControl {
    fn default() -> Self {
        Self::public()
    }
}

impl AccessControl {
    pub fn public() -> Self {
        Self {
            public: true,
            password: None,
        }
    }

    pub fn password(pw: impl Into<String>) -> Self {
        Self {
            public: false,
            password: Some(pw.into()),
        }
    }

    /// Whether `candidate` unlocks this share. Public shares always accept.
    pub fn accepts(&self, candidate: Option<&str>) -> bool {
        if self.public {
            return true;
        }
        match (&self.password, candidate) {
            (Some(pw), Some(c)) => !pw.is_empty() && pw == c,
            _ => false,
        }
    }
}

/// Per-field visibility toggles ("字段展示控制" in the share dialog).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FieldDisplay {
    #[serde(default = "default_true")]
    pub show_description: bool,
    #[serde(default = "default_true")]
    pub show_params: bool,
    #[serde(default = "default_true")]
    pub show_headers: bool,
    #[serde(default = "default_true")]
    pub show_body: bool,
    #[serde(default = "default_true")]
    pub show_auth: bool,
    #[serde(default = "default_true")]
    pub show_cookies: bool,
    #[serde(default = "default_true")]
    pub show_path: bool,
    #[serde(default = "default_true")]
    pub show_examples: bool,
    #[serde(default = "default_true")]
    pub show_mock: bool,
}

impl Default for FieldDisplay {
    fn default() -> Self {
        Self {
            show_description: true,
            show_params: true,
            show_headers: true,
            show_body: true,
            show_auth: true,
            show_cookies: true,
            show_path: true,
            show_examples: true,
            show_mock: true,
        }
    }
}

impl FieldDisplay {
    /// Each toggle's (field-key, label). Order matches the dialog layout.
    pub const FIELDS: &'static [(&'static str, &'static str)] = &[
        ("show_description", "接口描述"),
        ("show_params", "请求参数"),
        ("show_headers", "请求头"),
        ("show_body", "请求体"),
        ("show_auth", "认证"),
        ("show_cookies", "Cookie"),
        ("show_path", "路径参数"),
        ("show_examples", "示例"),
        ("show_mock", "Mock"),
    ];

    /// Get a field by its key string (for the dialog to read/write toggles).
    pub fn get(&self, key: &str) -> bool {
        match key {
            "show_description" => self.show_description,
            "show_params" => self.show_params,
            "show_headers" => self.show_headers,
            "show_body" => self.show_body,
            "show_auth" => self.show_auth,
            "show_cookies" => self.show_cookies,
            "show_path" => self.show_path,
            "show_examples" => self.show_examples,
            "show_mock" => self.show_mock,
            _ => true,
        }
    }

    /// Set a field by its key string.
    pub fn set(&mut self, key: &str, val: bool) {
        match key {
            "show_description" => self.show_description = val,
            "show_params" => self.show_params = val,
            "show_headers" => self.show_headers = val,
            "show_body" => self.show_body = val,
            "show_auth" => self.show_auth = val,
            "show_cookies" => self.show_cookies = val,
            "show_path" => self.show_path = val,
            "show_examples" => self.show_examples = val,
            "show_mock" => self.show_mock = val,
            _ => {}
        }
    }
}

/// How the user chose to distribute the share ("分享方式").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShareMethod {
    /// Copy/share a link.
    Link,
    /// Generate a QR code (encodes the link).
    QrCode,
    /// Export a self-contained HTML file.
    ExportHtml,
}

impl ShareMethod {
    pub const ALL: &'static [ShareMethod] = &[
        ShareMethod::Link,
        ShareMethod::QrCode,
        ShareMethod::ExportHtml,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShareMethod::Link => "链接",
            ShareMethod::QrCode => "二维码",
            ShareMethod::ExportHtml => "导出 HTML",
        }
    }
}

fn default_true() -> bool {
    true
}

/// Current Unix timestamp (seconds). Centralised so the server and models
/// share one clock source.
pub fn now_ts() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Generate a short 8-char id suitable for URLs (`/s/<id>`).
pub fn short_id() -> String {
    // Trailing 8 chars of a sparkid cover its random tail, so two codes minted
    // in the same timestamp tick still differ (the leading 8 are the timestamp
    // prefix and would collide). 8 Base58 chars (~46 bits) is ample for a
    // single-user offline tool's share URLs.
    id_suffix(&new_id(), 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forever_never_expires() {
        let cfg = ShareConfig::new("p", "P");
        assert!(cfg.is_valid_at(cfg.created_at + 10_000_000));
    }

    #[test]
    fn days_expiry_is_strict() {
        let mut cfg = ShareConfig::new("p", "P");
        cfg.expire = Expiration::Days(7);
        // Just before the window closes → valid.
        assert!(cfg.is_valid_at(cfg.created_at + 7 * 86_400 - 1));
        // Exactly at / after → invalid.
        assert!(!cfg.is_valid_at(cfg.created_at + 7 * 86_400));
        assert!(!cfg.is_valid_at(cfg.created_at + 7 * 86_400 + 1));
    }

    #[test]
    fn public_accepts_anything() {
        let ac = AccessControl::public();
        assert!(ac.accepts(None));
        assert!(ac.accepts(Some("anything")));
    }

    #[test]
    fn password_is_strict() {
        let ac = AccessControl::password("s3cret");
        assert!(!ac.accepts(None));
        assert!(!ac.accepts(Some("wrong")));
        assert!(ac.accepts(Some("s3cret")));
    }

    #[test]
    fn empty_password_rejects() {
        let ac = AccessControl::password("");
        assert!(!ac.accepts(Some("")));
        assert!(!ac.accepts(Some("x")));
    }

    #[test]
    fn short_id_is_8_chars() {
        let id = short_id();
        assert_eq!(id.len(), 8);
    }

    #[test]
    fn field_display_round_trip() {
        let mut fd = FieldDisplay::default();
        fd.set("show_body", false);
        assert!(!fd.get("show_body"));
        assert!(fd.get("show_params"));
    }
}
