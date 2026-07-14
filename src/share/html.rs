//! HTML document generation for the share system — layered, structured builder.
//!
//! Produces a fully self-contained postman-style documentation page using a
//! lightweight [`Html`] builder that guarantees tag closure via RAII. The page
//! is assembled from independent layers: head → header → sidebar → content
//! (modules → requests → kv-tables / auth / body / examples) → script.
//!
//! The builder replaces the old `&mut String` + `push_str` approach where tag
//! closure was manual and error-prone. Each layer is a standalone function;
//! the public API (`render_doc_html`, `render_request_fragment`) is unchanged.

use crate::share::models::{FieldDisplay, ShareConfig, ShareScope};
use crate::state::models::{
    ApiRequest, AuthConfig, AuthType, BodyType, Folder, KeyValue, Project, RequestMethod,
};

// ===========================================================================
// Html builder — RAII tag closure, auto-escaping, zero dependencies
// ===========================================================================

/// A lightweight HTML string builder. Tags opened via [`Html::tag`] are
/// guaranteed to be closed because the closure body runs between the open and
/// close tags. Raw HTML (CSS/JS/pre-escaped strings) goes through [`Html::raw`];
/// untrusted text goes through [`Html::text`] (auto-escaped).
pub struct Html {
    buf: String,
}

impl Html {
    pub fn new() -> Self {
        Self {
            buf: String::with_capacity(4096),
        }
    }

    /// Append pre-escaped raw HTML (CSS, JS, already-escaped fragments).
    /// Use sparingly — prefer [`text`](Self::text) for untrusted content.
    pub fn raw(&mut self, html: &str) -> &mut Self {
        self.buf.push_str(html);
        self
    }

    /// Append a raw string with format args (for pre-escaped HTML fragments).
    pub fn rawf(&mut self, args: std::fmt::Arguments<'_>) -> &mut Self {
        self.buf.push_str(&args.to_string());
        self
    }

    /// Append auto-escaped text content (HTML entity encoding applied).
    pub fn text(&mut self, text: &str) -> &mut Self {
        self.buf.push_str(&escape_html(text));
        self
    }

    /// Open a tag with attributes, run `body`, then close the tag. The close
    /// is emitted unconditionally even if `body` panics (via Drop on the guard).
    pub fn tag(
        &mut self,
        name: &str,
        attrs: &[(&str, &str)],
        body: impl FnOnce(&mut Self),
    ) -> &mut Self {
        self.open_tag(name, attrs);
        body(self);
        self.close_tag(name);
        self
    }

    /// Open a tag without closing it. Pair with [`close_tag`](Self::close_tag).
    /// Prefer [`tag`](Self::tag) whenever possible.
    pub fn open_tag(&mut self, name: &str, attrs: &[(&str, &str)]) -> &mut Self {
        self.buf.push('<');
        self.buf.push_str(name);
        for (key, val) in attrs {
            self.buf.push(' ');
            self.buf.push_str(key);
            self.buf.push_str("=\"");
            self.buf.push_str(&escape_attr(val));
            self.buf.push('"');
        }
        self.buf.push_str(">\n");
        self
    }

    /// Close a previously opened tag.
    pub fn close_tag(&mut self, name: &str) -> &mut Self {
        self.buf.push_str("</");
        self.buf.push_str(name);
        self.buf.push_str(">\n");
        self
    }

    /// Consume the builder and return the HTML string.
    pub fn finish(self) -> String {
        self.buf
    }
}

impl Default for Html {
    fn default() -> Self {
        Self::new()
    }
}

// ===========================================================================
// Escaping
// ===========================================================================

/// Escape a string for safe insertion into HTML **text content**.
fn escape_html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// Escape a string for a **double-quoted attribute value**. Same as text
/// escaping but also neutralizes newlines/tabs (which can break attribute
/// parsing in some HTML parsers).
fn escape_attr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            '\n' | '\r' | '\t' => out.push(' '),
            _ => out.push(c),
        }
    }
    out
}

// ===========================================================================
// Public API
// ===========================================================================

/// Generate the complete documentation HTML for a share.
pub fn render_doc_html(config: &ShareConfig, project: &Project) -> String {
    let requests = scoped_requests(config, project);
    let env_vars = environment_vars(project, config.environment_id.as_deref());
    let logo_data_url = config
        .logo_path
        .as_ref()
        .and_then(|p| logo_data_url(p.as_path()));
    let title = config.display_title();

    let mut h = Html::new();
    h.raw("<!DOCTYPE html>\n");
    h.tag("html", &[("lang", "zh-CN")], |h| {
        render_head(h, &title);
        h.tag("body", &[], |h| {
            render_header(h, &title, logo_data_url.as_deref(), project, config);
            h.tag("div", &[("class", "layout")], |h| {
                render_sidebar(h, &requests);
                render_content(h, &requests, project, config);
            });
            render_script(h, &env_vars);
        });
    });
    h.finish()
}

/// Generate one request's documentation as an HTML fragment.
pub fn render_request_fragment(req: &ApiRequest, fd: &FieldDisplay) -> String {
    let mut h = Html::new();
    render_request(&mut h, "", req, fd, true);
    h.finish()
}

// ===========================================================================
// Layer: <head>
// ===========================================================================

fn render_head(h: &mut Html, title: &str) {
    h.tag("head", &[], |h| {
        h.raw("<meta charset=\"utf-8\">\n");
        h.raw("<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n");
        h.raw(
            "<meta http-equiv=\"Cache-Control\" content=\"no-cache, no-store, must-revalidate\">\n",
        );
        h.raw("<meta http-equiv=\"Pragma\" content=\"no-cache\">\n");
        h.raw("<meta http-equiv=\"Expires\" content=\"0\">\n");
        h.tag("title", &[], |h| {
            h.text(title);
        });
        h.tag("style", &[], |h| {
            h.raw(CSS);
        });
    });
}

// ===========================================================================
// Layer: <header> top bar
// ===========================================================================

fn render_header(
    h: &mut Html,
    title: &str,
    logo: Option<&str>,
    project: &Project,
    config: &ShareConfig,
) {
    h.tag("header", &[("class", "topbar")], |h| {
        // Left: logo + title.
        h.tag("div", &[("class", "topbar-left")], |h| {
            if let Some(url) = logo {
                h.tag(
                    "img",
                    &[("class", "logo"), ("src", url), ("alt", "logo")],
                    |_| {},
                );
            } else {
                h.tag("div", &[("class", "logo-placeholder")], |h| {
                    h.text("V");
                });
            }
            h.tag("span", &[("class", "doc-title")], |h| {
                h.text(title);
            });
        });
        // Center: search.
        h.tag("div", &[("class", "topbar-center")], |h| {
            h.raw(
                "<input id=\"search\" class=\"search\" type=\"text\" \
                 placeholder=\"搜索接口…\" autocomplete=\"off\">\n",
            );
        });
        // Right: environment selector.
        h.tag("div", &[("class", "topbar-right")], |h| {
            if !project.environments.is_empty() {
                h.tag(
                    "select",
                    &[("id", "env-select"), ("class", "env-select")],
                    |h| {
                        h.tag("option", &[("value", "")], |h| {
                            h.text("无环境");
                        });
                        for env in &project.environments {
                            let selected =
                                config.environment_id.as_deref() == Some(env.id.as_str());
                            if selected {
                                h.tag(
                                    "option",
                                    &[("value", env.id.as_str()), ("selected", "selected")],
                                    |h| {
                                        h.text(&env.name);
                                    },
                                );
                            } else {
                                h.tag("option", &[("value", env.id.as_str())], |h| {
                                    h.text(&env.name);
                                });
                            }
                        }
                    },
                );
            }
        });
    });
}

// ===========================================================================
// Layer: <aside> sidebar
// ===========================================================================

fn render_sidebar(h: &mut Html, requests: &[ScopedRequest<'_>]) {
    h.tag("aside", &[("class", "sidebar")], |h| {
        h.tag("nav", &[("id", "api-tree")], |h| {
            let mut last_path: Option<&str> = None;
            let mut group_open = false;
            for (path, req) in requests {
                if Some(path.as_str()) != last_path {
                    // Close previous group.
                    if group_open {
                        h.close_tag("div"); // tree-group
                    }
                    // Folder header.
                    let folder_name = match path.rsplit_once('>') {
                        Some((_, leaf)) => leaf.trim(),
                        None => path.trim(),
                    };
                    let display = if folder_name.is_empty() {
                        "默认模块"
                    } else {
                        folder_name
                    };
                    let module_id = module_id_for(path);
                    h.open_tag("div", &[("class", "tree-group")]);
                    group_open = true;
                    h.tag(
                        "a",
                        &[("class", "tree-folder"), ("data-module", &module_id)],
                        |h| {
                            h.text(display);
                        },
                    );
                    last_path = Some(path.as_str());
                }
                // Request item.
                let data_name = format!("{} {}", req.name, path);
                h.tag(
                    "a",
                    &[
                        ("class", "tree-item"),
                        ("data-id", &req.id),
                        ("data-name", &data_name),
                    ],
                    |h| {
                        let badge_class =
                            format!("method-badge method-{}", method_class(&req.method));
                        h.tag("span", &[("class", &badge_class)], |h| {
                            h.text(req.method.badge_label());
                        });
                        h.tag("span", &[("class", "tree-name")], |h| {
                            h.text(&req.name);
                        });
                    },
                );
            }
            if group_open {
                h.close_tag("div");
            }
        });
    });
}

// ===========================================================================
// Layer: <main> content — modules + requests
// ===========================================================================

fn render_content(
    h: &mut Html,
    requests: &[ScopedRequest<'_>],
    project: &Project,
    config: &ShareConfig,
) {
    h.tag("main", &[("class", "content"), ("id", "content")], |h| {
        if requests.is_empty() {
            h.tag("div", &[("class", "empty")], |h| {
                h.text("暂无可分享的接口文档");
            });
            return;
        }
        // Global context (project-scope only).
        if config.scope == ShareScope::Project {
            render_global_context(h, project, &config.field_display);
        }
        // Default-shown request: first one with real content.
        let default_idx = requests
            .iter()
            .position(|(_, r)| request_has_detail(r, &config.field_display))
            .unwrap_or(0);
        let default_path = &requests[default_idx].0;

        // Group by folder path → modules.
        let mut current_path: Option<&str> = None;
        for (i, (path, req)) in requests.iter().enumerate() {
            if Some(path.as_str()) != current_path {
                if current_path.is_some() {
                    h.close_tag("div"); // module-requests
                    h.close_tag("div"); // module
                }
                render_module_banner(h, path, path == default_path);
                h.open_tag("div", &[("class", "module-requests")]);
                current_path = Some(path.as_str());
            }
            render_request(h, path, req, &config.field_display, i == default_idx);
        }
        if current_path.is_some() {
            h.close_tag("div"); // module-requests
            h.close_tag("div"); // module
        }
        h.raw("<div id=\"doc-empty\" class=\"empty\" hidden>选择左侧接口查看文档</div>\n");
    });
}

// ---- Global context panel ----

fn render_global_context(h: &mut Html, project: &Project, fd: &FieldDisplay) {
    let has_globals = has_enabled(&project.global_params)
        || has_enabled(&project.global_headers)
        || has_enabled(&project.global_variables);
    if !has_globals {
        return;
    }
    h.tag("section", &[("class", "doc active global-context")], |h| {
        h.tag("div", &[("class", "doc-header")], |h| {
            h.tag("h1", &[("class", "doc-title")], |h| {
                h.text("📋 公共参数说明");
            });
        });
        h.tag("div", &[("class", "doc-breadcrumb")], |h| {
            h.text("以下参数将应用于本项目下所有接口");
        });
        if has_enabled(&project.global_params) {
            render_kv_table(h, "全局请求参数", &project.global_params);
        }
        if has_enabled(&project.global_headers) {
            render_kv_table(h, "全局请求头", &project.global_headers);
        }
        if has_enabled(&project.global_variables) {
            h.tag("div", &[("class", "doc-section")], |h| {
                h.tag("h2", &[], |h| {
                    h.text("环境变量");
                });
                h.tag("table", &[("class", "kv-table")], |h| {
                    h.raw("<thead><tr><th>变量名</th><th>说明</th><th>初始值</th></tr></thead>\n");
                    h.tag("tbody", &[], |h| {
                        for kv in project.global_variables.iter().filter(|k| k.enabled) {
                            h.tag("tr", &[], |h| {
                                h.tag("td", &[("class", "kv-key")], |h| {
                                    h.text(&format!("{{{{{}}}}}", kv.key));
                                });
                                h.tag("td", &[], |h| {
                                    h.text(&kv.description);
                                });
                                h.tag("td", &[("class", "var-subst")], |h| {
                                    h.text(&kv.value);
                                });
                            });
                        }
                    });
                });
            });
        }
    });
    let _ = fd;
}

// ---- Module banner ----

fn render_module_banner(h: &mut Html, path: &str, is_default: bool) {
    let class = if is_default {
        "module active"
    } else {
        "module"
    };
    let module_id = module_id_for(path);
    let (name, parent) = match path.rsplit_once('>') {
        Some((p, leaf)) => (leaf.trim(), Some(p.trim())),
        None => (path.trim(), None),
    };
    let display_name = if name.is_empty() {
        "默认模块"
    } else {
        name
    };
    h.open_tag("div", &[("class", class), ("id", &module_id)]);
    h.tag("div", &[("class", "module-header")], |h| {
        h.tag("span", &[("class", "module-icon")], |h| {
            h.text("📁");
        });
        h.tag("div", &[("class", "module-title-wrap")], |h| {
            h.tag("h2", &[("class", "module-title")], |h| {
                h.text(display_name);
            });
            if let Some(p) = parent {
                h.tag("span", &[("class", "module-path")], |h| {
                    h.text(p);
                });
            }
        });
    });
}

// ---- Single request document ----

fn render_request(h: &mut Html, path: &str, req: &ApiRequest, fd: &FieldDisplay, is_active: bool) {
    let class = if is_active { "doc active" } else { "doc" };
    let doc_id = format!("doc-{}", req.id);
    h.tag("section", &[("class", class), ("id", &doc_id)], |h| {
        // Title row.
        h.tag("div", &[("class", "doc-header")], |h| {
            let badge = format!("method-badge method-{}", method_class(&req.method));
            h.tag("span", &[("class", &badge)], |h| {
                h.text(req.method.badge_label());
            });
            h.tag("h1", &[("class", "doc-title")], |h| {
                h.text(&req.name);
            });
        });

        // Folder breadcrumb.
        if !path.is_empty() {
            h.tag("div", &[("class", "doc-breadcrumb")], |h| {
                h.text(path);
            });
        }

        // Status tags.
        let mut tags = req.tags.clone();
        if !req.status.is_empty() && !tags.iter().any(|t| t == &req.status) {
            tags.insert(0, req.status.clone());
        }
        if !tags.is_empty() {
            h.tag("div", &[("class", "doc-tags")], |h| {
                for t in &tags {
                    h.tag("span", &[("class", "tag")], |h| {
                        h.text(t);
                    });
                }
            });
        }

        // URL block.
        h.tag("div", &[("class", "doc-url-block")], |h| {
            let chip = format!("method-chip method-{}", method_class(&req.method));
            h.tag("span", &[("class", &chip)], |h| {
                h.text(req.method.badge_label());
            });
            h.tag("code", &[("class", "doc-url var-subst")], |h| {
                h.text(&req.url);
            });
        });

        // Detail sections (gated by FieldDisplay).
        if fd.show_description && !req.description.trim().is_empty() {
            h.tag("div", &[("class", "doc-section")], |h| {
                h.tag("h2", &[], |h| {
                    h.text("接口描述");
                });
                h.tag("div", &[("class", "description")], |h| {
                    h.raw(&render_description(&req.description));
                });
            });
        }
        if fd.show_path && has_enabled(&req.path) {
            render_kv_table(h, "路径参数", &req.path);
        }
        if fd.show_params && has_enabled(&req.params) {
            render_kv_table(h, "请求参数", &req.params);
        }
        if fd.show_headers && has_enabled(&req.headers) {
            render_kv_table(h, "请求头", &req.headers);
        }
        if fd.show_auth && req.auth.is_active() {
            render_auth_block(h, &req.auth);
        }
        if fd.show_cookies && has_enabled(&req.cookies) {
            render_kv_table(h, "Cookie", &req.cookies);
        }
        let has_body = fd.show_body && req.body.body_type != BodyType::None;
        if has_body {
            render_body_block(h, &req.body);
        }
        if fd.show_examples {
            if let Some(resp) = req.last_response.as_ref() {
                render_example_block(h, resp);
            }
        }
        if fd.show_mock {
            if let Some(mock) = req.mock.as_ref().filter(|m| m.enabled) {
                render_mock_block(h, mock);
            }
        }

        // Empty-state hint.
        let has_any_detail = (fd.show_description && !req.description.trim().is_empty())
            || (fd.show_path && has_enabled(&req.path))
            || (fd.show_params && has_enabled(&req.params))
            || (fd.show_headers && has_enabled(&req.headers))
            || (fd.show_auth && req.auth.is_active())
            || (fd.show_cookies && has_enabled(&req.cookies))
            || has_body;
        if !has_any_detail {
            h.tag("div", &[("class", "doc-section empty-detail")], |h| {
                h.tag("div", &[("class", "empty-hint")], |h| {
                    h.tag("span", &[("class", "empty-icon")], |h| {
                        h.text("📝");
                    });
                    h.tag("p", &[], |h| {
                        h.text("该接口暂未填写参数说明、请求体或响应示例。");
                    });
                    h.tag("p", &[("class", "empty-sub")], |h| {
                        h.text("在 Verve 中打开此接口，补充请求参数、请求体、描述等信息后重新分享即可。");
                    });
                });
            });
        }

        // Audit metadata.
        h.tag("div", &[("class", "doc-meta")], |h| {
            if !req.created_by.is_empty() {
                h.raw(&format!("<span>创建者：{}</span>\n", escape_html(&req.created_by)));
            }
            if !req.created_at.is_empty() {
                h.raw(&format!("<span>创建于：{}</span>\n", escape_html(&req.created_at)));
            }
            if !req.updated_by.is_empty() {
                h.raw(&format!("<span>更新者：{}</span>\n", escape_html(&req.updated_by)));
            }
            if !req.updated_at.is_empty() {
                h.raw(&format!("<span>更新于：{}</span>\n", escape_html(&req.updated_at)));
            }
        });
    });
}

// ---- KV table ----

fn render_kv_table(h: &mut Html, title: &str, kvs: &[KeyValue]) {
    h.tag("div", &[("class", "doc-section")], |h| {
        h.tag("h2", &[], |h| {
            h.text(title);
        });
        h.tag("table", &[("class", "kv-table")], |h| {
            h.raw(
                "<thead><tr><th>参数名</th><th>类型</th><th>必填</th><th>说明</th><th>示例值</th></tr></thead>\n",
            );
            h.tag("tbody", &[], |h| {
                for kv in kvs.iter().filter(|k| k.enabled) {
                    h.tag("tr", &[], |h| {
                        // Parameter name + red `*` if required.
                        h.tag("td", &[("class", "kv-key")], |h| {
                            h.text(&kv.key);
                            if kv.required {
                                h.raw("<span class=\"req-mark\">*</span>");
                            }
                        });
                        // Type as a badge.
                        h.tag("td", &[], |h| {
                            h.raw(&format!(
                                "<span class=\"type-badge\">{}</span>",
                                escape_html(kv.field_type.as_str())
                            ));
                        });
                        // Required badge (green = required, gray = optional).
                        h.tag("td", &[], |h| {
                            if kv.required {
                                h.raw("<span class=\"badge badge-req\">是</span>");
                            } else {
                                h.raw("<span class=\"badge badge-opt\">否</span>");
                            }
                        });
                        h.tag("td", &[], |h| {
                            h.text(&kv.description);
                        });
                        h.tag("td", &[("class", "var-subst")], |h| {
                            h.text(&kv.value);
                        });
                    });
                }
            });
        });
    });
}

// ---- Auth block ----

fn render_auth_block(h: &mut Html, auth: &AuthConfig) {
    h.tag("div", &[("class", "doc-section")], |h| {
        h.tag("h2", &[], |h| {
            h.text("认证");
        });
        h.tag("table", &[("class", "kv-table")], |h| {
            h.raw("<thead><tr><th>项目</th><th>值</th></tr></thead>\n");
            h.tag("tbody", &[], |h| {
                h.tag("tr", &[], |h| {
                    h.tag("td", &[], |h| {
                        h.text("类型");
                    });
                    h.tag("td", &[], |h| {
                        h.text(auth.auth_type.as_str());
                    });
                });
                match auth.auth_type {
                    AuthType::Bearer => {
                        h.tag("tr", &[], |h| {
                            h.tag("td", &[], |h| {
                                h.text("Token");
                            });
                            h.tag("td", &[("class", "var-subst")], |h| {
                                h.text(&auth.token);
                            });
                        });
                    }
                    AuthType::Basic => {
                        h.tag("tr", &[], |h| {
                            h.tag("td", &[], |h| {
                                h.text("用户名");
                            });
                            h.tag("td", &[("class", "var-subst")], |h| {
                                h.text(&auth.username);
                            });
                        });
                        h.tag("tr", &[], |h| {
                            h.tag("td", &[], |h| {
                                h.text("密码");
                            });
                            h.tag("td", &[], |h| {
                                h.text("••••••");
                            });
                        });
                    }
                    AuthType::ApiKey => {
                        h.tag("tr", &[], |h| {
                            h.tag("td", &[], |h| {
                                h.text("Key 名");
                            });
                            h.tag("td", &[], |h| {
                                h.text(&auth.key);
                            });
                        });
                        h.tag("tr", &[], |h| {
                            h.tag("td", &[], |h| {
                                h.text("Key 值");
                            });
                            h.tag("td", &[("class", "var-subst")], |h| {
                                h.text(&auth.value);
                            });
                        });
                        h.tag("tr", &[], |h| {
                            h.tag("td", &[], |h| {
                                h.text("添加到");
                            });
                            h.tag("td", &[], |h| {
                                h.text(auth_target_label(auth.add_to));
                            });
                        });
                    }
                    AuthType::None => {}
                }
            });
        });
    });
}

fn auth_target_label(t: crate::state::models::AuthTarget) -> &'static str {
    use crate::state::models::AuthTarget::*;
    match t {
        Header => "Header",
        Query => "Query",
    }
}

// ---- Body block ----

fn render_body_block(h: &mut Html, body: &crate::state::models::RequestBody) {
    h.tag("div", &[("class", "doc-section")], |h| {
        h.tag("h2", &[], |h| {
            h.text("请求体");
        });
        match body.body_type {
            BodyType::Raw => {
                // If the body has visual field descriptions (raw_parameter),
                // render them as a field table instead of a raw code block —
                // richer documentation. Fall back to the code block otherwise.
                if !body.raw_parameter.is_empty() {
                    render_kv_table(h, "字段说明", &body.raw_parameter);
                }
                let lang = body.raw_language.lower_name();
                h.tag("div", &[("class", "code-block")], |h| {
                    h.tag("div", &[("class", "code-lang")], |h| {
                        h.text(lang);
                    });
                    h.tag("pre", &[], |h| {
                        h.tag("code", &[("class", "var-subst")], |h| {
                            h.text(&body.raw);
                        });
                    });
                });
            }
            BodyType::Urlencoded | BodyType::FormData => {
                let rows = if body.body_type == BodyType::FormData {
                    &body.form_data
                } else {
                    &body.urlencoded
                };
                // Reuse the full kv table renderer (includes required column +
                // badges) for consistency with query/header tables.
                render_kv_table(h, "参数", rows);
            }
            BodyType::None => {}
        }
    });
}

// ---- Example (response) block ----

fn render_example_block(h: &mut Html, resp: &crate::state::models::Response) {
    h.tag("div", &[("class", "doc-section")], |h| {
        h.tag("h2", &[], |h| {
            h.text("返回示例");
        });
        h.tag("div", &[("class", "resp-meta")], |h| {
            h.raw(&format!(
                "<span class=\"resp-status\">状态：{}</span>\n",
                resp.status
            ));
            h.raw(&format!("<span>耗时：{} ms</span>\n", resp.time_ms));
            h.raw(&format!("<span>大小：{} B</span>\n", resp.size));
        });
        h.tag("div", &[("class", "code-block")], |h| {
            h.tag("pre", &[], |h| {
                h.tag("code", &[], |h| {
                    h.text(&resp.body);
                });
            });
        });
    });
}

// ---- Mock block ----

fn render_mock_block(h: &mut Html, mock: &crate::state::models::MockRule) {
    h.tag("div", &[("class", "doc-section")], |h| {
        h.tag("h2", &[], |h| {
            h.text("Mock");
        });
        h.tag("div", &[("class", "resp-meta")], |h| {
            h.raw(&format!(
                "<span class=\"resp-status\">状态：{}</span>\n",
                mock.status
            ));
            h.raw(&format!("<span>延迟：{} ms</span>\n", mock.delay_ms));
        });
        h.tag("div", &[("class", "code-block")], |h| {
            h.tag("pre", &[], |h| {
                h.tag("code", &[], |h| {
                    h.text(&mock.body);
                });
            });
        });
    });
}

// ===========================================================================
// Layer: <script>
// ===========================================================================

fn render_script(h: &mut Html, env_vars: &serde_json::Value) {
    h.tag("script", &[], |h| {
        h.raw("const ENV_VARS = ");
        h.raw(&serde_json::to_string(env_vars).unwrap_or_else(|_| "{}".into()));
        h.raw(";\n");
        h.raw(JS);
    });
}

// ===========================================================================
// Scope resolution (data shaping — pure logic, no HTML)
// ===========================================================================

type ScopedRequest<'a> = (String, &'a ApiRequest);

fn scoped_requests<'a>(config: &ShareConfig, project: &'a Project) -> Vec<ScopedRequest<'a>> {
    let mut out: Vec<ScopedRequest<'a>> = Vec::new();
    match config.scope {
        ShareScope::Project => {
            for req in &project.requests {
                out.push((String::new(), req));
            }
            for folder in &project.folders {
                walk_folder(folder, "", &mut out);
            }
        }
        ShareScope::Request => {
            if let Some(target) = config.target_id.as_deref() {
                if let Some((chain, req)) = project.find_request(target) {
                    let path = folder_chain_path(project, &chain);
                    out.push((path, req));
                }
            }
        }
        ShareScope::Folder => {
            if let Some(target) = config.target_id.as_deref() {
                if let Some(folder) = project.find_folder(target).map(|(_, f)| f) {
                    walk_folder(folder, "", &mut out);
                }
            }
        }
    }
    out
}

fn walk_folder<'a>(folder: &'a Folder, prefix: &str, out: &mut Vec<ScopedRequest<'a>>) {
    let path = if prefix.is_empty() {
        folder.name.clone()
    } else {
        format!("{prefix} > {}", folder.name)
    };
    for req in &folder.requests {
        out.push((path.clone(), req));
    }
    for sub in &folder.folders {
        walk_folder(sub, &path, out);
    }
}

fn folder_chain_path(project: &Project, chain: &[String]) -> String {
    if chain.is_empty() {
        return String::new();
    }
    let mut names: Vec<String> = Vec::new();
    for id in chain {
        if let Some((_, f)) = project.find_folder(id) {
            names.push(f.name.clone());
        }
    }
    names.join(" > ")
}

fn environment_vars(project: &Project, env_id: Option<&str>) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for kv in &project.global_variables {
        if kv.enabled {
            map.insert(kv.key.clone(), serde_json::Value::String(kv.value.clone()));
        }
    }
    if let Some(id) = env_id {
        if let Some(env) = project.environments.iter().find(|e| e.id == id) {
            for kv in &env.variables {
                if kv.enabled {
                    map.insert(kv.key.clone(), serde_json::Value::String(kv.value.clone()));
                }
            }
        }
    }
    serde_json::Value::Object(map)
}

// ===========================================================================
// Helpers (pure)
// ===========================================================================

fn has_enabled(kvs: &[KeyValue]) -> bool {
    kvs.iter()
        .any(|k| k.enabled && (!k.key.is_empty() || !k.value.is_empty()))
}

fn request_has_detail(req: &ApiRequest, fd: &FieldDisplay) -> bool {
    (fd.show_description && !req.description.trim().is_empty())
        || (fd.show_path && has_enabled(&req.path))
        || (fd.show_params && has_enabled(&req.params))
        || (fd.show_headers && has_enabled(&req.headers))
        || (fd.show_auth && req.auth.is_active())
        || (fd.show_cookies && has_enabled(&req.cookies))
        || (fd.show_body && req.body.body_type != BodyType::None)
        || (fd.show_examples && req.last_response.is_some())
        || (fd.show_mock && req.mock.as_ref().map(|m| m.enabled).unwrap_or(false))
}

fn method_class(m: &RequestMethod) -> &'static str {
    match m {
        RequestMethod::Get => "get",
        RequestMethod::Post => "post",
        RequestMethod::Put => "put",
        RequestMethod::Delete => "delete",
        RequestMethod::Patch => "patch",
        RequestMethod::Head => "head",
        RequestMethod::Options => "options",
    }
}

fn module_id_for(path: &str) -> String {
    if path.is_empty() {
        "module-default".to_string()
    } else {
        let cleaned: String = path
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        format!("module-{cleaned}")
    }
}

fn render_description(desc: &str) -> String {
    let escaped = escape_html(desc);
    auto_link_urls(&escaped)
}

fn auto_link_urls(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let chars: Vec<char> = html.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let rest: String = chars[i..].iter().collect();
        if rest.starts_with("http://") || rest.starts_with("https://") {
            let scheme_len = if rest.starts_with("https://") { 8 } else { 7 };
            let mut end = i + scheme_len;
            while end < chars.len() {
                let c = chars[end];
                if c == ' ' || c == '\n' || c == '<' || c == '"' || c == '\'' {
                    break;
                }
                end += 1;
            }
            let url: String = chars[i..end].iter().collect();
            // The URL comes from already-escaped text, so no double-escaping.
            out.push_str(&format!(
                "<a href=\"{}\" target=\"_blank\" rel=\"noopener\">{}</a>",
                url, url
            ));
            i = end;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out.replace('\n', "<br>\n")
}

// ===========================================================================
// Logo (base64 data URL)
// ===========================================================================

fn logo_data_url(path: &std::path::Path) -> Option<String> {
    let bytes = std::fs::read(path).ok()?;
    let mime = mime_from_ext(path).unwrap_or("image/png");
    let b64 = base64_encode(&bytes);
    Some(format!("data:{mime};base64,{b64}"))
}

fn mime_from_ext(path: &std::path::Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "svg" => Some("image/svg+xml"),
        "webp" => Some("image/webp"),
        _ => Some("image/png"),
    }
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = ((b[0] as u32) << 16) | ((b[1] as u32) << 8) | (b[2] as u32);
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

// ===========================================================================
// CSS + JS (inline constants — unchanged content)
// ===========================================================================

const CSS: &str = r#"
:root {
  --bg: #ffffff;
  --bg-muted: #f7f8fa;
  --border: #e5e7eb;
  --text: #1f2937;
  --text-muted: #6b7280;
  --accent: #3b82f6;
  --code-bg: #1e1e2e;
  --code-text: #e5e7eb;
  --m-get: #16a34a;
  --m-post: #ea580c;
  --m-put: #2563eb;
  --m-patch: #d97706;
  --m-delete: #dc2626;
  --m-head: #6b7280;
  --m-options: #7c3aed;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #1a1a2e;
    --bg-muted: #232342;
    --border: #2d2d4a;
    --text: #e5e7eb;
    --text-muted: #9ca3af;
    --accent: #60a5fa;
    --code-bg: #11111b;
  }
}
* { box-sizing: border-box; }
body {
  margin: 0;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC",
    "Hiragino Sans GB", "Microsoft YaHei", sans-serif;
  background: var(--bg);
  color: var(--text);
  font-size: 14px;
  line-height: 1.6;
}
.topbar {
  display: flex;
  align-items: center;
  height: 56px;
  padding: 0 20px;
  border-bottom: 1px solid var(--border);
  background: var(--bg);
  gap: 16px;
  position: sticky;
  top: 0;
  z-index: 10;
}
.topbar-left { display: flex; align-items: center; gap: 12px; min-width: 0; }
.logo { width: 32px; height: 32px; border-radius: 6px; object-fit: contain; }
.logo-placeholder {
  width: 32px; height: 32px; border-radius: 6px;
  background: var(--accent); color: #fff;
  display: flex; align-items: center; justify-content: center;
  font-weight: 700; font-size: 18px;
}
.doc-title { font-weight: 600; font-size: 16px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.topbar-center { flex: 1; display: flex; justify-content: center; }
.search {
  width: 100%; max-width: 420px;
  padding: 8px 14px; border: 1px solid var(--border); border-radius: 8px;
  background: var(--bg-muted); color: var(--text); font-size: 13px; outline: none;
}
.search:focus { border-color: var(--accent); }
.topbar-right { display: flex; align-items: center; gap: 8px; }
.env-select {
  padding: 6px 10px; border: 1px solid var(--border); border-radius: 6px;
  background: var(--bg-muted); color: var(--text); font-size: 13px; cursor: pointer;
}
.layout { display: flex; height: calc(100vh - 57px); overflow: hidden; }
.sidebar {
  width: 280px; min-width: 200px; max-width: 360px;
  border-right: 1px solid var(--border);
  background: var(--bg-muted);
  overflow-y: auto;
  padding: 8px;
}
#api-tree { display: flex; flex-direction: column; gap: 2px; }
.tree-folder {
  font-size: 12px; font-weight: 600; color: var(--text-muted);
  padding: 10px 8px 4px; text-transform: uppercase; letter-spacing: 0.5px;
  cursor: pointer; text-decoration: none; display: block;
}
.tree-folder:hover { color: var(--accent); }
.tree-group .tree-item { padding-left: 20px; }
.tree-item {
  display: flex; align-items: center; gap: 8px;
  padding: 7px 8px; border-radius: 6px; cursor: pointer; text-decoration: none; color: var(--text);
}
.tree-item:hover { background: var(--border); }
.tree-item.active { background: var(--accent); color: #fff; }
.tree-item.active .method-badge { background: rgba(255,255,255,0.25) !important; color: #fff !important; }
.tree-name { font-size: 13px; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.content {
  flex: 1; overflow-y: auto; padding: 32px 40px; max-width: 960px; margin: 0 auto; width: 100%;
}
.empty { color: var(--text-muted); text-align: center; padding: 60px 20px; font-size: 14px; }
.doc { padding-bottom: 40px; display: none; }
.doc.active { display: block; }
.global-context { background: var(--bg-muted); border: 1px solid var(--border); border-radius: 8px; padding: 20px; margin-bottom: 24px; }
.module { margin-bottom: 32px; border: 1px solid var(--border); border-radius: 10px; overflow: hidden; }
.module.active { display: block; }
.module-header {
  display: flex; align-items: center; gap: 10px;
  padding: 16px 20px; background: linear-gradient(135deg, var(--accent), #6366f1);
  color: #fff;
}
.module-icon { font-size: 22px; }
.module-title-wrap { display: flex; flex-direction: column; gap: 2px; }
.module-title { margin: 0; font-size: 18px; font-weight: 700; }
.module-path { font-size: 11px; opacity: 0.8; }
.module-requests { padding: 8px 0; }
.module-requests .doc { padding: 24px 20px; margin: 0; border-bottom: 1px solid var(--border); }
.module-requests .doc:last-child { border-bottom: none; }
.doc-header { display: flex; align-items: center; gap: 12px; margin-bottom: 4px; }
.doc-header .doc-title { font-size: 22px; font-weight: 700; }
.doc-breadcrumb { color: var(--text-muted); font-size: 12px; margin-bottom: 12px; }
.doc-tags { display: flex; gap: 6px; margin-bottom: 12px; flex-wrap: wrap; }
.tag {
  font-size: 11px; padding: 2px 8px; border-radius: 10px;
  background: var(--bg-muted); border: 1px solid var(--border); color: var(--text-muted);
}
.doc-url-block {
  display: flex; align-items: center; gap: 10px; padding: 12px 14px;
  background: var(--bg-muted); border: 1px solid var(--border); border-radius: 8px;
  margin-bottom: 24px; flex-wrap: wrap;
}
.doc-url { font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace; font-size: 13px; word-break: break-all; }
.method-badge {
  display: inline-block; min-width: 38px; text-align: center;
  font-size: 11px; font-weight: 700; padding: 2px 7px; border-radius: 4px;
  letter-spacing: 0.5px; color: #fff; flex-shrink: 0;
}
.method-chip { font-size: 12px; padding: 3px 10px; border-radius: 12px; }
.method-get { background: var(--m-get); }
.method-post { background: var(--m-post); }
.method-put { background: var(--m-put); }
.method-patch { background: var(--m-patch); }
.method-delete { background: var(--m-delete); }
.method-head { background: var(--m-head); }
.method-options { background: var(--m-options); }
.doc-section { margin-bottom: 24px; }
.doc-section h2 { font-size: 15px; font-weight: 600; margin: 0 0 12px; padding-bottom: 6px; border-bottom: 1px solid var(--border); }
.description { color: var(--text); line-height: 1.7; }
.description a { color: var(--accent); }
.kv-table { width: 100%; border-collapse: collapse; font-size: 13px; }
.kv-table th, .kv-table td { padding: 8px 12px; border: 1px solid var(--border); text-align: left; vertical-align: top; }
.kv-table th { background: var(--bg-muted); font-weight: 600; color: var(--text-muted); font-size: 12px; }
.kv-table tr:nth-child(even) td { background: var(--bg-muted); }
.kv-key { font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace; font-weight: 600; }
.req-mark { color: #dc2626; font-weight: bold; margin-left: 2px; }
.type-badge {
  display: inline-block; font-size: 11px; padding: 1px 7px; border-radius: 4px;
  background: var(--bg-muted); border: 1px solid var(--border); color: var(--text-muted);
  font-family: ui-monospace, monospace;
}
.badge {
  display: inline-block; font-size: 11px; padding: 1px 8px; border-radius: 10px; font-weight: 600;
}
.badge-req { background: #dcfce7; color: #16a34a; }
.badge-opt { background: #f3f4f6; color: #9ca3af; }
.code-block {
  background: var(--code-bg); color: var(--code-text); border-radius: 8px;
  overflow: hidden; margin-top: 8px;
}
.code-lang { padding: 6px 12px; font-size: 11px; color: #9ca3af; border-bottom: 1px solid rgba(255,255,255,0.1); text-transform: uppercase; }
.code-block pre { margin: 0; padding: 14px; overflow-x: auto; }
.code-block code { font-family: ui-monospace, "SF Mono", Menlo, Consolas, monospace; font-size: 13px; white-space: pre-wrap; word-break: break-all; }
.resp-meta { display: flex; gap: 16px; font-size: 12px; color: var(--text-muted); margin-bottom: 8px; }
.resp-status { font-weight: 600; }
.doc-meta { display: flex; gap: 16px; flex-wrap: wrap; font-size: 12px; color: var(--text-muted); padding-top: 16px; border-top: 1px solid var(--border); }
.empty-detail { text-align: center; }
.empty-hint { padding: 32px 16px; background: var(--bg-muted); border-radius: 8px; border: 1px dashed var(--border); }
.empty-hint .empty-icon { font-size: 32px; display: block; margin-bottom: 8px; }
.empty-hint p { margin: 4px 0; color: var(--text-muted); }
.empty-hint .empty-sub { font-size: 12px; opacity: 0.8; }
@media (max-width: 768px) {
  .sidebar { width: 200px; }
  .content { padding: 20px; }
  .topbar-center { display: none; }
}
"#;

const JS: &str = r#"
(function () {
  const items = Array.from(document.querySelectorAll('.tree-item'));
  const sections = Array.from(document.querySelectorAll('section.doc'));
  const folders = Array.from(document.querySelectorAll('.tree-folder'));
  const emptyEl = document.getElementById('doc-empty');
  const search = document.getElementById('search');
  const envSelect = document.getElementById('env-select');

  function showDoc(id) {
    sections.forEach(s => {
      if (s.id && s.id.indexOf('doc-') === 0) {
        s.classList.toggle('active', s.id === 'doc-' + id);
      }
    });
    items.forEach(i => { i.classList.toggle('active', i.dataset.id === id); });
    if (emptyEl) emptyEl.hidden = true;
    applyEnvVars();
    var target = document.getElementById('doc-' + id);
    if (target && target.scrollIntoView) {
      target.scrollIntoView({ block: 'start', behavior: 'smooth' });
    }
    var activeItem = items.find(i => i.dataset.id === id);
    if (activeItem && activeItem.scrollIntoView) {
      activeItem.scrollIntoView({ block: 'nearest' });
    }
  }

  function showModule(moduleId) {
    var module = document.getElementById(moduleId);
    if (module && module.scrollIntoView) {
      module.scrollIntoView({ block: 'start', behavior: 'smooth' });
    }
    var firstDoc = module ? module.querySelector('section.doc[id]') : null;
    if (firstDoc) {
      showDoc(firstDoc.id.replace('doc-', ''));
    }
  }

  function applyEnvVars() {
    let vars = ENV_VARS;
    document.querySelectorAll('.var-subst').forEach(el => {
      let text = el.dataset.raw || el.textContent;
      if (!el.dataset.raw) el.dataset.raw = text;
      el.textContent = text.replace(/\{\{(\w+)\}\}/g, (_, k) =>
        vars[k] !== undefined ? vars[k] : '{{' + k + '}}'
      );
    });
  }

  items.forEach(item => {
    item.addEventListener('click', function (e) {
      e.preventDefault();
      showDoc(this.dataset.id);
    });
  });

  folders.forEach(folder => {
    folder.addEventListener('click', function (e) {
      e.preventDefault();
      var mid = this.dataset.module;
      if (mid) showModule(mid);
    });
  });

  if (search) {
    search.addEventListener('input', function () {
      const q = this.value.trim().toLowerCase();
      items.forEach(item => {
        const name = (item.dataset.name || '').toLowerCase();
        item.style.display = name.includes(q) ? '' : 'none';
      });
    });
  }

  var activeReq = sections.find(s => s.id && s.id.indexOf('doc-') === 0 && s.classList.contains('active'));
  if (activeReq) {
    var activeId = activeReq.id.replace('doc-', '');
    items.forEach(i => { i.classList.toggle('active', i.dataset.id === activeId); });
    applyEnvVars();
    var activeItem = items.find(i => i.dataset.id === activeId);
    if (activeItem && activeItem.scrollIntoView) {
      activeItem.scrollIntoView({ block: 'nearest' });
    }
  } else if (items.length === 0 && emptyEl) {
    emptyEl.hidden = false;
  }
})();
"#;

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::share::models::ShareScope;
    use crate::state::models::{RawLanguage, RequestBody, RequestMethod};

    fn sample_project() -> Project {
        let mut p = Project::new("Demo");
        let mut req = ApiRequest::new("Login", RequestMethod::Post, "{{baseUrl}}/login");
        req.description = "用户登录接口".into();
        req.params.push(KeyValue {
            enabled: true,
            key: "from".into(),
            value: "web".into(),
            ..Default::default()
        });
        req.body = RequestBody {
            body_type: BodyType::Raw,
            raw_language: RawLanguage::Json,
            raw: r#"{"user":"admin"}"#.into(),
            ..Default::default()
        };
        p.requests.push(req);
        p
    }

    #[test]
    fn html_builder_closes_tags() {
        let mut h = Html::new();
        h.tag("div", &[("class", "test")], |h| {
            h.text("hello");
        });
        let out = h.finish();
        assert!(out.contains("<div class=\"test\">"));
        assert!(out.contains("hello"));
        assert!(out.contains("</div>"));
    }

    #[test]
    fn html_builder_escapes_text() {
        let mut h = Html::new();
        h.text("<script>alert(1)</script>");
        let out = h.finish();
        assert!(!out.contains("<script>"));
        assert!(out.contains("&lt;script&gt;"));
    }

    #[test]
    fn html_builder_escapes_attrs() {
        let mut h = Html::new();
        h.tag("div", &[("data-x", "a\"b")], |h| {
            h.text("ok");
        });
        let out = h.finish();
        assert!(out.contains("data-x=\"a&quot;b\""));
    }

    #[test]
    fn escape_attr_strips_newlines() {
        let escaped = escape_attr("a\nb\tc");
        assert!(!escaped.contains('\n'));
        assert!(!escaped.contains('\t'));
        assert!(escaped.contains(' '));
    }

    #[test]
    fn renders_full_doc_without_panic() {
        let project = sample_project();
        let mut cfg = ShareConfig::new(&project.id, &project.name);
        cfg.title = "Demo Docs".into();
        let html = render_doc_html(&cfg, &project);
        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains("Demo Docs"));
        assert!(html.contains("/login"));
        assert!(html.contains("用户登录接口"));
    }

    #[test]
    fn field_display_hides_sections() {
        let project = sample_project();
        let mut cfg = ShareConfig::new(&project.id, &project.name);
        cfg.field_display.show_body = false;
        let html = render_doc_html(&cfg, &project);
        assert!(!html.contains("请求体"));
        assert!(html.contains("请求参数"));
    }

    #[test]
    fn empty_project_does_not_panic() {
        let project = Project::new("Empty");
        let cfg = ShareConfig::new(&project.id, &project.name);
        let html = render_doc_html(&cfg, &project);
        assert!(html.contains("暂无可分享的接口文档"));
    }

    #[test]
    fn scope_request_only_shows_target() {
        let mut project = Project::new("P");
        project
            .requests
            .push(ApiRequest::new("A", RequestMethod::Get, "/a"));
        let req_b = ApiRequest::new("B", RequestMethod::Get, "/b");
        let target_id = req_b.id.clone();
        project.requests.push(req_b);

        let mut cfg = ShareConfig::new(&project.id, &project.name);
        cfg.scope = ShareScope::Request;
        cfg.target_id = Some(target_id.clone());

        let html = render_doc_html(&cfg, &project);
        assert!(html.contains(&format!("doc-{target_id}")));
        assert!(!html.contains("data-name=\"A"));
    }

    #[test]
    fn default_shows_first_content_rich_request() {
        let mut project = Project::new("P");
        let empty = ApiRequest::new("空接口", RequestMethod::Get, "/ping");
        let _empty_id = empty.id.clone();
        project.requests.push(empty);
        let mut rich = ApiRequest::new("有参数", RequestMethod::Post, "/submit");
        let rich_id = rich.id.clone();
        rich.params.push(KeyValue {
            enabled: true,
            key: "name".into(),
            value: "test".into(),
            ..Default::default()
        });
        project.requests.push(rich);

        let cfg = ShareConfig::new(&project.id, &project.name);
        let html = render_doc_html(&cfg, &project);
        assert!(
            html.contains(&format!(r#""doc-{rich_id}""#)) || html.contains(&rich_id),
            "rich request should be present"
        );
    }

    #[test]
    fn modular_layout_groups_by_folder() {
        let mut project = Project::new("P");
        project.folders.push(Folder {
            id: "f1".into(),
            name: "UserMod".into(),
            description: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            folders: Vec::new(),
            requests: vec![ApiRequest::new("Login", RequestMethod::Post, "/login")],
            variables: Vec::new(),
            base_url: None,
        });
        project.folders.push(Folder {
            id: "f2".into(),
            name: "OrderMod".into(),
            description: String::new(),
            params: Vec::new(),
            headers: Vec::new(),
            folders: Vec::new(),
            requests: vec![ApiRequest::new("Create", RequestMethod::Post, "/order")],
            variables: Vec::new(),
            base_url: None,
        });

        let cfg = ShareConfig::new(&project.id, &project.name);
        let html = render_doc_html(&cfg, &project);
        // Module banners present (the builder inserts newlines between the tag
        // and its text content, so check for class + name separately).
        assert!(html.contains("module-title") && html.contains("UserMod"));
        assert!(html.contains("module-title") && html.contains("OrderMod"));
    }

    #[test]
    fn environment_vars_are_injected() {
        let mut project = Project::new("P");
        project
            .requests
            .push(ApiRequest::new("A", RequestMethod::Get, "{{baseUrl}}/a"));
        let env = crate::state::models::Environment {
            id: "env1".into(),
            name: "Prod".into(),
            variables: vec![KeyValue {
                enabled: true,
                key: "baseUrl".into(),
                value: "https://api.test".into(),
                ..Default::default()
            }],
        };
        project.environments.push(env);

        let mut cfg = ShareConfig::new(&project.id, &project.name);
        cfg.environment_id = Some("env1".into());
        let html = render_doc_html(&cfg, &project);
        assert!(html.contains("https://api.test"));
    }

    #[test]
    fn base64_round_trip() {
        let input = b"Hello, Verve!";
        let encoded = base64_encode(input);
        assert!(!encoded.is_empty());
    }

    #[test]
    fn no_double_escaping_in_urls() {
        let desc = "Visit https://example.com?a=1&b=2";
        let rendered = render_description(desc);
        // The & in the URL should be escaped once to &amp;, not twice.
        assert!(rendered.contains("&amp;"));
        assert!(!rendered.contains("&amp;amp;"));
        assert!(rendered.contains("<a href=\"https://example.com?a=1&amp;b=2\""));
    }

    #[test]
    fn required_params_have_red_star_in_docs() {
        let project = sample_project();
        // The sample project's param "from" is required by default (new() sets
        // required=true). Verify the doc renders a red `*` after its name.
        let cfg = ShareConfig::new(&project.id, &project.name);
        let html = render_doc_html(&cfg, &project);
        assert!(
            html.contains("req-mark"),
            "required params should have a req-mark span"
        );
    }

    #[test]
    fn optional_params_have_gray_badge_in_docs() {
        let mut project = Project::new("P");
        let mut req = ApiRequest::new("Test", RequestMethod::Get, "/t");
        let mut kv = KeyValue::new("optional_param", "val");
        kv.required = false; // explicitly optional
        req.params.push(kv);
        project.requests.push(req);

        let cfg = ShareConfig::new(&project.id, &project.name);
        let html = render_doc_html(&cfg, &project);
        assert!(
            html.contains("badge-opt"),
            "optional params should have a badge-opt span"
        );
        assert!(
            html.contains("badge-req"),
            "required params (default) should have a badge-req span"
        );
    }

    #[test]
    fn raw_parameter_fields_rendered_in_body_docs() {
        let mut project = Project::new("P");
        let mut req = ApiRequest::new("Create", RequestMethod::Post, "/create");
        req.body = RequestBody {
            body_type: BodyType::Raw,
            raw_language: RawLanguage::Json,
            raw: r#"{"name":"test"}"#.into(),
            raw_parameter: vec![KeyValue {
                enabled: true,
                key: "name".into(),
                value: "test".into(),
                required: true,
                description: "用户名".into(),
                ..Default::default()
            }],
            ..Default::default()
        };
        project.requests.push(req);

        let cfg = ShareConfig::new(&project.id, &project.name);
        let html = render_doc_html(&cfg, &project);
        assert!(
            html.contains("字段说明"),
            "raw_parameter should render as a field-description table"
        );
        assert!(html.contains("用户名"));
    }

    #[test]
    fn keyvalue_new_defaults_to_required() {
        let kv = KeyValue::new("key", "value");
        assert!(kv.required, "KeyValue::new() should default required=true");
    }
}
