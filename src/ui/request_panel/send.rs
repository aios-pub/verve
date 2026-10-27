//! Protocol send engine: HTTP/SSE/WebSocket/TCP/gRPC/Socket.IO dispatch,
//! the stop-flag machinery, and error-response helpers.
use super::RequestPanel;
use super::folder_helpers::{apply_autosave_example, resolve_effective_base_url};
use super::{register_stop, truncate_history_body, unregister_stop};
use crate::http;
use crate::state::AppEvent;
use crate::state::models::*;
use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::WindowExt as _;
use gpui_component::{
    Disableable as _, Selectable as _, Sizable as _, button::ButtonVariants as _,
};
use std::collections::BTreeMap;

impl RequestPanel {
    pub fn send(&mut self, cx: &mut Context<Self>) {
        let id = match self.request_id.clone() {
            Some(id) => id,
            None => return,
        };
        self.commit_to_model(cx);

        // Resolve the request fields from the model (post-commit).
        let (pre_script, tests_script, req_clone, mut vars) = {
            let st = self.state.read(cx);
            let project = match st.active_project() {
                Some(p) => p,
                None => return,
            };
            let (chain, req) = match project.find_request(&id) {
                Some(x) => x,
                None => return,
            };
            let folder_vars: Vec<KeyValue> = project
                .folder_variables_chain(&chain)
                .into_iter()
                .flatten()
                .cloned()
                .collect();
            // Merge project-global params/headers/cookies into the request's
            // own rows (per-request overrides global) so globals actually take
            // effect on the wire. The merge happens here on the clone, so every
            // dispatch branch below (HTTP/SSE/gRPC/…) inherits it without each
            // needing to know about project globals.
            let mut merged = req.clone();
            merged.params = crate::state::models::merge_kv(&project.global_params, &req.params);
            merged.headers = crate::state::models::merge_kv(&project.global_headers, &req.headers);
            merged.cookies = crate::state::models::merge_kv(&project.global_cookies, &req.cookies);
            // System variables: mock_server address
            let mut system = BTreeMap::new();
            system.insert(
                "mock_server".to_string(),
                format!(
                    "http://127.0.0.1:{}",
                    crate::share::server::DEFAULT_PORT
                ),
            );
            let mut vars = crate::state::models::effective_variables(
                &system,
                &project.global_variables,
                project.active_env_variables(),
                &folder_vars,
                &req.variables,
            );
            // Inject the effective base URL into vars so {{baseUrl}} and
            // relative-path resolution both work. Resolution is driven by the
            // tri-state `req_base_mode` (inherit / explicit-disable /
            // override); a returned None leaves both keys unset, so a relative
            // URL with an explicitly-disabled prefix stays relative.
            let req_base_raw = self.req_base_url.read(cx).value().to_string();
            if let Some(base) = resolve_effective_base_url(
                &self.req_base_mode,
                &req_base_raw,
                &vars,
                project,
                &chain,
            ) {
                vars.insert("__folder_base_url__".to_string(), base.clone());
                vars.entry("baseUrl".to_string()).or_insert(base);
            }
            (
                req.pre_script.clone(),
                req.tests_script.clone(),
                merged,
                vars,
            )
        };

        // Dispatch by protocol. HTTP/SSE/GraphQL share the HTTP prepare path;
        // the rest are handled (or stubbed) in their own branches.
        match req_clone.protocol {
            Protocol::Sse => {
                self.send_sse(id, req_clone, vars, pre_script, cx);
                return;
            }
            Protocol::WebSocket => {
                self.send_websocket(id, req_clone, vars, cx);
                return;
            }
            Protocol::Tcp => {
                self.send_tcp(id, req_clone, vars, cx);
                return;
            }
            Protocol::Grpc => {
                self.send_grpc(id, req_clone, vars, cx);
                return;
            }
            Protocol::SocketIo => {
                self.send_socketio(id, req_clone, vars, cx);
                return;
            }
            Protocol::Markdown | Protocol::Directory => {
                self.send_placeholder(id, req_clone.protocol, cx);
                return;
            }
            // HTTP and GraphQL execute via the HTTP path below.
            Protocol::Http | Protocol::Graphql => {}
        }

        // --- Pre-request script (PRD §5.2): runs before the request is sent.
        //     Variable mutations are merged into `vars` so they get substituted.
        let pre_result = crate::scripting::run_pre_request(&pre_script, &vars);
        let mut script_logs: Vec<String> = Vec::new();
        if !pre_script.trim().is_empty() {
            script_logs.push("— Pre-request Script —".to_string());
            script_logs.extend(pre_result.logs.clone());
            if let Some(e) = &pre_result.error {
                script_logs.push(format!("Pre-script error: {e}"));
            }
            // Apply SetVariable effects to the vars map (for substitution).
            for effect in &pre_result.effects {
                if let crate::scripting::SideEffect::SetVariable { name, value, .. } = effect {
                    vars.insert(name.clone(), value.clone());
                }
            }
        }

        let prepared = match http::prepare(
            req_clone.method,
            &req_clone.url,
            &req_clone.params,
            &req_clone.headers,
            &req_clone.path,
            &req_clone.cookies,
            &req_clone.auth,
            &req_clone.body,
            &vars,
            http::DEFAULT_TIMEOUT_SECS,
        ) {
            Ok(p) => p,
            Err(e) => {
                let err_resp = Response {
                    error: Some(format!("{e}")),
                    received_at: Some(Response::now_stamp()),
                    ..Default::default()
                };
                self.state.update(cx, |s, _cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id) {
                            r.last_response = Some(err_resp.clone());
                            apply_autosave_example(r, &err_resp);
                        }
                    }
                });
                // 必须在可变借用释放后再emit，避免双重借用panic。
                // spawn异步任务在当前同步流程结束后执行，借用已释放。
                let state = self.state.clone();
                cx.spawn(async move |_, cx| {
                    let _ = state.update(cx, |s, cx| {
                        let _ = s;
                        cx.emit(AppEvent::ResponseUpdated(id.clone()));
                    });
                })
                .detach();
                return;
            }
        };

        let client = cx.http_client();
        let id_clone = id.clone();
        // Snapshot the actually-sent request (post-substitution) before
        // `prepared` is moved into the task, so the "实际请求" tab can show
        // the real request regardless of the outcome.
        let actual_request = prepared.request_text();
        let actual_curl = prepared.to_curl();
        // Clear any previous response and mark "请求中" so the realtime panel
        // doesn't show stale content (body/status/time/size) while the request
        // is in flight. Mirrors the streaming branches' placeholder seeding;
        // `streaming` stays false because HTTP sends aren't user-cancellable.
        // The actual-request snapshot rides along so it is visible in-flight.
        self.state.update(cx, |s, cx| {
            s.sending = Some(id.clone());
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "请求中…".into(),
                        actual_request: Some(actual_request.clone()),
                        actual_curl: Some(actual_curl.clone()),
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });
        let env_id = self
            .state
            .read(cx)
            .active_project()
            .and_then(|p| p.active_environment.clone());
        cx.spawn(async move |this, cx| {
            let mut resp = http::execute(client.as_ref(), prepared, http::DEFAULT_TIMEOUT_SECS).await;
            resp.actual_request = Some(actual_request);
            resp.actual_curl = Some(actual_curl);

            // --- Post-request / Tests script (PRD §5.2): reads `response`,
            //     extracts data (e.g. token) into variables, runs assertions.
            if !tests_script.trim().is_empty() {
                let post_result = crate::scripting::run_post_request(&tests_script, &vars, &resp);
                script_logs.push("— Tests Script —".to_string());
                script_logs.extend(post_result.logs.clone());
                if let Some(e) = &post_result.error {
                    script_logs.push(format!("Tests error: {e}"));
                }
                // Persist SetVariable effects into the active environment.
                let env_effects: Vec<(String, String)> = post_result
                    .effects
                    .iter()
                    .filter_map(|e| match e {
                        crate::scripting::SideEffect::SetVariable {
                            scope: crate::scripting::VarScope::Environment,
                            name,
                            value,
                        } => Some((name.clone(), value.clone())),
                        _ => None,
                    })
                    .collect();
                if !env_effects.is_empty() {
                    let _ = this.update(cx, |this, cx| {
                        this.state.update(cx, |s, cx| {
                            if let Some(project) = s.active_project_mut() {
                                if let Some(env_id) = &env_id {
                                    if let Some(env) =
                                        project.environments.iter_mut().find(|e| &e.id == env_id)
                                    {
                                        for (k, v) in &env_effects {
                                            if let Some(kv) =
                                                env.variables.iter_mut().find(|kv| &kv.key == k)
                                            {
                                                kv.value = v.clone();
                                            } else {
                                                env.variables.push(
                                                    crate::state::models::KeyValue::new(k, v),
                                                );
                                            }
                                        }
                                    }
                                }
                            }
                            s.notify_edited(cx);
                        });
                    });
                }
                // Surface assertion summary on the response.
                if post_result.assertions_failed > 0 || post_result.assertions_passed > 0 {
                    let summary = format!(
                        "{} passed, {} failed",
                        post_result.assertions_passed, post_result.assertions_failed
                    );
                    if let Some(err) = &mut resp.error {
                        err.push_str(&format!(" [{summary}]"));
                    } else if post_result.assertions_failed > 0 {
                        resp.error = Some(format!("Assertions: {summary}"));
                    }
                }
            }

            // Attach script logs to the response body as a footer so they show
            // up in the response panel. (A dedicated script-console is a
            // future enhancement.)
            if !script_logs.is_empty() {
                let mut combined = resp.body.clone();
                if !combined.is_empty() {
                    combined.push_str("\n\n");
                }
                combined.push_str("// ── Script Output ──\n");
                combined.push_str(&script_logs.join("\n"));
                resp.body = combined;
            }

            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    // Look up the request once so we can both store last_response
                    // and build a rich history entry from its authored fields.
                    let (project_id, name, method, url, query_params, request_headers) = {
                        match s.active_project_mut() {
                            Some(project) => {
                                let pid = project.id.clone();
                                match project.find_request_mut(&id_clone) {
                                    Some((_, r)) => {
                                        r.last_response = Some(resp.clone());
                                        apply_autosave_example(r, &resp);
                                        let qp: Vec<(String, String)> = r
                                            .params
                                            .iter()
                                            .filter(|kv| kv.enabled && !kv.key.is_empty())
                                            .take(crate::state::HISTORY_KV_MAX)
                                            .map(|kv| (kv.key.clone(), kv.value.clone()))
                                            .collect();
                                        let rh: Vec<(String, String)> = r
                                            .headers
                                            .iter()
                                            .filter(|kv| kv.enabled && !kv.key.is_empty())
                                            .take(crate::state::HISTORY_KV_MAX)
                                            .map(|kv| (kv.key.clone(), kv.value.clone()))
                                            .collect();
                                        (pid, r.name.clone(), r.method, r.url.clone(), qp, rh)
                                    }
                                    None => (
                                        pid,
                                        String::new(),
                                        RequestMethod::Get,
                                        String::new(),
                                        Vec::new(),
                                        Vec::new(),
                                    ),
                                }
                            }
                            None => (
                                String::new(),
                                String::new(),
                                RequestMethod::Get,
                                String::new(),
                                Vec::new(),
                                Vec::new(),
                            ),
                        }
                    };

                    // Truncate response body at char boundary.
                    let (response_body, response_truncated) = truncate_history_body(&resp.body);

                    // Append to history.
                    let entry = HistoryEntry {
                        id: new_id(),
                        project_id,
                        request_id: Some(id_clone.clone()),
                        name,
                        method,
                        url,
                        status: resp.status,
                        status_text: resp.status_text.clone(),
                        time_ms: resp.time_ms,
                        size: resp.size,
                        at: chrono::Utc::now().to_rfc3339(),
                        error: resp.error.clone(),
                        query_params,
                        request_headers,
                        response_body,
                        response_truncated,
                    };
                    s.data.history.insert(0, entry);
                    if s.data.history.len() > 200 {
                        s.data.history.truncate(200);
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// SSE: stream events, appending each to the response body in real time.
    pub(super) fn send_sse(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        pre_script: String,
        cx: &mut Context<Self>,
    ) {
        // Legacy self-heal: SSE requests were created with GET and the method
        // selector was hidden, so POST-style SSE APIs (JSON body, LLM
        // chat/completions 等) could only go out as GET — a GET with a body
        // is broken HTTP semantics and servers answer with an empty body.
        // Upgrade to POST when a body is present; body-less SSE (classic
        // EventSource style) keeps GET. The "实际请求" snapshot below shows
        // the method that actually went on the wire.
        let mut req = req;
        if req.method == RequestMethod::Get && !req.body.is_empty() {
            req.method = RequestMethod::Post;
        }

        // Pre-request script (same as HTTP).
        let mut script_logs: Vec<String> = Vec::new();
        let mut vars = vars;
        if !pre_script.trim().is_empty() {
            let pre_result = crate::scripting::run_pre_request(&pre_script, &vars);
            script_logs.extend(pre_result.logs);
            for effect in &pre_result.effects {
                if let crate::scripting::SideEffect::SetVariable { name, value, .. } = effect {
                    vars.insert(name.clone(), value.clone());
                }
            }
        }

        let prepared = match http::prepare(
            req.method,
            &req.url,
            &req.params,
            &req.headers,
            &req.path,
            &req.cookies,
            &req.auth,
            &req.body,
            &vars,
            http::DEFAULT_TIMEOUT_SECS,
        ) {
            Ok(p) => p,
            Err(e) => {
                self.set_error_response(&id, format!("{e}"), cx);
                return;
            }
        };

        let client = cx.http_client();
        let stop = register_stop(&id);
        let id_clone = id.clone();
        // Snapshot the actually-sent request before `prepared` is moved into
        // the stream future (see the HTTP branch for rationale).
        let actual_request = prepared.request_text();
        let actual_curl = prepared.to_curl();
        self.state.update(cx, |s, _cx| s.sending = Some(id.clone()));

        cx.spawn(async move |this, cx| {
            // Seed an empty streaming response so the UI shows the "停止" state.
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            r.last_response = Some(Response {
                                status_text: "SSE 流式接收中…".into(),
                                streaming: true,
                                actual_request: Some(actual_request.clone()),
                                actual_curl: Some(actual_curl.clone()),
                                ..Default::default()
                            });
                        }
                    }
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
            });

            // Shared accumulator + last-seen length, polled by a timer task to
            // push live updates into the model (the stream future itself is Send
            // and can't capture the !Send GPUI entity handle).
            let acc = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
            let acc_for_stream = acc.clone();
            let acc_for_poll = acc.clone();
            let mut last_len = 0usize;

            let mut stream_fut = Box::pin(crate::http::sse::stream(
                client.clone(),
                prepared,
                http::DEFAULT_TIMEOUT_SECS,
                stop.clone(),
                acc_for_stream,
            ));

            // Poll loop: interleave the stream with periodic UI-sync ticks.
            let result = loop {
                let timer = cx
                    .background_executor()
                    .timer(std::time::Duration::from_millis(120));
                match futures::future::select(&mut stream_fut, Box::pin(timer)).await {
                    futures::future::Either::Left((res, _)) => break res,
                    futures::future::Either::Right(((), _)) => {
                        // UI-sync tick: mirror new accumulator content into the model.
                        let snapshot = acc_for_poll.lock().map(|s| s.clone()).unwrap_or_default();
                        if snapshot.len() != last_len {
                            last_len = snapshot.len();
                            let _ = this.update(cx, |this, cx| {
                                this.state.update(cx, |s, cx| {
                                    if let Some(project) = s.active_project_mut() {
                                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                                            if let Some(resp) = r.last_response.as_mut() {
                                                resp.body = snapshot.clone();
                                                resp.size = snapshot.len() as u64;
                                            }
                                        }
                                    }
                                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                                });
                            });
                        }
                    }
                }
            };

            let mut resp = match result {
                Ok(r) => r,
                Err(e) => crate::state::models::Response {
                    error: Some(format!("{e}")),
                    received_at: Some(Response::now_stamp()),
                    ..Default::default()
                },
            };
            resp.actual_request = Some(actual_request);
            resp.actual_curl = Some(actual_curl);
            // Append script logs.
            if !script_logs.is_empty() {
                resp.body.push_str("\n\n// ── 预执行脚本输出 ──\n");
                resp.body.push_str(&script_logs.join("\n"));
            }
            unregister_stop(&id_clone);
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            r.last_response = Some(resp.clone());
                            apply_autosave_example(r, &resp);
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// WebSocket: connect and drain incoming messages into the response body.
    pub(super) fn send_websocket(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let url = crate::http::variable::substitute(&req.url, &vars);
        let url = crate::http::normalize_url_with_default(&url, "ws");
        let stop = register_stop(&id);
        let id_clone = id.clone();
        self.state.update(cx, |s, _cx| s.sending = Some(id.clone()));

        // Seed a streaming placeholder response.
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "WebSocket 连接中…".into(),
                        streaming: true,
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });

        // Connect off the GPUI thread; the connection runs on its own tokio rt.
        let conn = crate::http::ws::connect(&url);

        cx.spawn(async move |this, cx| {
            let mut log = String::new();
            loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    log.push_str("\n[已断开]\n");
                    break;
                }
                match conn.frames.recv().await {
                    Ok(crate::http::ws::WsFrame::Status(s)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("• {s}"));
                    }
                    Ok(crate::http::ws::WsFrame::Message(m)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&crate::http::ws::format_message("收到", &m));
                    }
                    Ok(crate::http::ws::WsFrame::Done(err)) => {
                        if let Some(e) = err {
                            log.push_str(&format!("\n• {e}"));
                        }
                        break;
                    }
                    Err(_) => break,
                }
                // Push the current log into the model on each frame.
                let snapshot = log.clone();
                let _ = this.update(cx, |this, cx| {
                    this.state.update(cx, |s, cx| {
                        if let Some(project) = s.active_project_mut() {
                            if let Some((_, r)) = project.find_request_mut(&id_clone) {
                                if let Some(resp) = r.last_response.as_mut() {
                                    resp.body = snapshot.clone();
                                    resp.size = snapshot.len() as u64;
                                }
                            }
                        }
                        cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                    });
                });
            }
            unregister_stop(&id_clone);
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            if let Some(resp) = r.last_response.as_mut() {
                                resp.streaming = false;
                                resp.body = log.clone();
                                resp.size = log.len() as u64;
                                resp.received_at = Some(Response::now_stamp());
                            }
                            // Clone the response and apply autosave separately to avoid double-borrow.
                            if let Some(resp) = r.last_response.clone() {
                                apply_autosave_example(r, &resp);
                            }
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// TCP: connect and drain incoming data into the response body.
    pub(super) fn send_tcp(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let url = crate::http::variable::substitute(&req.url, &vars);
        let url = crate::http::normalize_url_with_default(&url, "tcp");
        let stop = register_stop(&id);
        let id_clone = id.clone();
        self.state.update(cx, |s, _cx| s.sending = Some(id.clone()));

        // Seed a streaming placeholder response.
        self.state.update(cx, |s, cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "TCP 连接中…".into(),
                        streaming: true,
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });

        let conn = crate::http::tcp::connect(&url);

        cx.spawn(async move |this, cx| {
            let mut log = String::new();
            loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    log.push_str("\n[已断开]");
                    break;
                }
                match conn.frames.recv().await {
                    Ok(crate::http::tcp::TcpFrame::Status(s)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("• {s}"));
                    }
                    Ok(crate::http::tcp::TcpFrame::Data(d)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("← {d}"));
                    }
                    Ok(crate::http::tcp::TcpFrame::Done(err)) => {
                        if let Some(e) = err {
                            log.push_str(&format!("\n• {e}"));
                        } else {
                            log.push_str("\n• 连接已关闭");
                        }
                        break;
                    }
                    Err(_) => break,
                }
                let snapshot = log.clone();
                let _ = this.update(cx, |this, cx| {
                    this.state.update(cx, |s, cx| {
                        if let Some(project) = s.active_project_mut() {
                            if let Some((_, r)) = project.find_request_mut(&id_clone) {
                                if let Some(resp) = r.last_response.as_mut() {
                                    resp.body = snapshot.clone();
                                    resp.size = snapshot.len() as u64;
                                }
                            }
                        }
                        cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                    });
                });
            }
            unregister_stop(&id_clone);
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            if let Some(resp) = r.last_response.as_mut() {
                                resp.streaming = false;
                                resp.body = log.clone();
                                resp.size = log.len() as u64;
                                resp.received_at = Some(Response::now_stamp());
                            }
                            if let Some(resp) = r.last_response.clone() {
                                apply_autosave_example(r, &resp);
                            }
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// gRPC: send via gRPC-Web (HTTP POST + framed protobuf/JSON).
    pub(super) fn send_grpc(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        let client = cx.http_client();
        let id_clone = id.clone();
        self.state.update(cx, |s, cx| {
            s.sending = Some(id.clone());
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "gRPC 调用中…".into(),
                        streaming: true,
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });

        let body_json = req.body.raw.clone();
        let headers = req.headers.clone();
        let url = crate::http::normalize_url_with_default(&req.url, "grpc");

        cx.spawn(async move |this, cx| {
            let resp = crate::http::grpc::execute_grpc_web(
                client.as_ref(),
                &url,
                &headers,
                &body_json,
                &vars,
                http::DEFAULT_TIMEOUT_SECS,
            )
            .await;

            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            r.last_response = Some(resp.clone());
                            apply_autosave_example(r, &resp);
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// Socket.IO: connect via engine.io WebSocket transport.
    pub(super) fn send_socketio(
        &mut self,
        id: String,
        req: ApiRequest,
        vars: BTreeMap<String, String>,
        cx: &mut Context<Self>,
    ) {
        // Socket.IO runs over WebSocket with engine.io protocol.
        // Convert socket:// to ws:// and connect.
        let ws_url = req
            .url
            .replacen("socket://", "ws://", 1)
            .replacen("socketio://", "ws://", 1);
        let ws_url = crate::http::variable::substitute(&ws_url, &vars);
        let stop = register_stop(&id);
        let id_clone = id.clone();
        self.state.update(cx, |s, cx| {
            s.sending = Some(id.clone());
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(Response {
                        status_text: "Socket.IO 连接中…".into(),
                        streaming: true,
                        ..Default::default()
                    });
                }
            }
            cx.emit(AppEvent::ResponseUpdated(id.clone()));
        });

        // Socket.IO uses engine.io: first message is "0" (open), then we
        // upgrade to WebSocket. For simplicity, connect directly via WS
        // and send/receive engine.io frames.
        let conn = crate::http::ws::connect(&format!(
            "{}/socket.io/?EIO=4&transport=websocket",
            ws_url.trim_end_matches('/')
        ));

        cx.spawn(async move |this, cx| {
            let mut log = String::new();
            // Send engine.io connect probe.
            let _ = conn.tx.send("40".into()).await; // Socket.IO CONNECT frame
            loop {
                if stop.load(std::sync::atomic::Ordering::SeqCst) {
                    log.push_str("\n[已断开]");
                    break;
                }
                match conn.frames.recv().await {
                    Ok(crate::http::ws::WsFrame::Status(s)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("• {s}"));
                    }
                    Ok(crate::http::ws::WsFrame::Message(m)) => {
                        if !log.is_empty() {
                            log.push('\n');
                        }
                        log.push_str(&format!("← {m}"));
                    }
                    Ok(crate::http::ws::WsFrame::Done(err)) => {
                        if let Some(e) = err {
                            log.push_str(&format!("\n• {e}"));
                        }
                        break;
                    }
                    Err(_) => break,
                }
                let snapshot = log.clone();
                let _ = this.update(cx, |this, cx| {
                    this.state.update(cx, |s, cx| {
                        if let Some(project) = s.active_project_mut() {
                            if let Some((_, r)) = project.find_request_mut(&id_clone) {
                                if let Some(resp) = r.last_response.as_mut() {
                                    resp.body = snapshot.clone();
                                    resp.size = snapshot.len() as u64;
                                }
                            }
                        }
                        cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                    });
                });
            }
            unregister_stop(&id_clone);
            let _ = this.update(cx, |this, cx| {
                this.state.update(cx, |s, cx| {
                    if let Some(project) = s.active_project_mut() {
                        if let Some((_, r)) = project.find_request_mut(&id_clone) {
                            if let Some(resp) = r.last_response.as_mut() {
                                resp.streaming = false;
                                resp.body = log.clone();
                                resp.size = log.len() as u64;
                                resp.received_at = Some(Response::now_stamp());
                            }
                            if let Some(resp) = r.last_response.clone() {
                                apply_autosave_example(r, &resp);
                            }
                        }
                    }
                    s.sending = None;
                    cx.emit(AppEvent::ResponseUpdated(id_clone.clone()));
                });
                cx.notify();
            });
        })
        .detach();
    }

    /// gRPC / Socket.IO placeholder (only for unimplemented edge cases).
    pub(super) fn send_placeholder(
        &mut self,
        id: String,
        protocol: Protocol,
        cx: &mut Context<Self>,
    ) {
        let name = protocol.to_string();
        let placeholder_resp = Response {
            status_text: name.clone(),
            error: Some(format!("{name} 协议支持开发中，敬请期待。")),
            received_at: Some(Response::now_stamp()),
            ..Default::default()
        };
        self.state.update(cx, |s, _cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(&id) {
                    r.last_response = Some(placeholder_resp.clone());
                    apply_autosave_example(r, &placeholder_resp);
                }
            }
        });
        // 必须在可变借用释放后再emit，避免双重借用panic。
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::ResponseUpdated(id));
            });
        })
        .detach();
        cx.notify();
    }

    /// Helper: write a plain error response onto the request.
    pub(super) fn set_error_response(&mut self, id: &str, msg: String, cx: &mut Context<Self>) {
        let err_resp = Response {
            error: Some(msg),
            received_at: Some(Response::now_stamp()),
            ..Default::default()
        };
        self.state.update(cx, |s, _cx| {
            if let Some(project) = s.active_project_mut() {
                if let Some((_, r)) = project.find_request_mut(id) {
                    r.last_response = Some(err_resp.clone());
                    apply_autosave_example(r, &err_resp);
                }
            }
        });
        // 必须在可变借用释放后再emit，避免双重借用panic。
        // spawn异步任务在当前同步流程结束后执行，借用已释放。
        let state = self.state.clone();
        let emit_id = id.to_string();
        cx.spawn(async move |_, cx| {
            let _ = state.update(cx, |s, cx| {
                let _ = s;
                cx.emit(AppEvent::ResponseUpdated(emit_id));
            });
        })
        .detach();
    }
}
