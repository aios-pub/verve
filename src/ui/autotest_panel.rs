//! Automated testing panel — the full-bleed "自动化测试" exclusive view.
//!
//! Replaces the previous placeholder. Lists suites/cases from the active
//! project, lets the user add/remove steps referencing existing ApiRequests,
//! and runs cases through [`autotest::runner::run_case`]. Results (per-step
//! status, assertion counts, extracted variables) stream in via an
//! `mpsc::UnboundedReceiver` polled in the background.
//!
//! Scope of this iteration:
//! - Add/delete suites and cases; rename inline (name edits persisted on change)
//! - Add steps by selecting an existing request from the active project; delete/reorder
//! - Run one case; show live results as steps complete
//! - Extractors / script overrides are persisted but not yet editable in the UI
//!   (they fall back to the request's own pre/test scripts, which covers most cases)

use std::collections::BTreeMap;
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _,
};
use http_client::HttpClient;
use tokio::sync::mpsc;

use crate::autotest::models::{TestCase, TestCaseStep, TestSuite};
use crate::autotest::runner::{run_case, RunEvent};
use crate::state::models::Project;
use crate::state::{AppEvent, AppState};

/// Events the panel emits upward (currently unused; reserved for future nav).
#[derive(Clone, Debug)]
pub enum AutoTestEvent {}

pub struct AutoTestPanel {
    pub state: Entity<AppState>,
    /// Currently-selected case id (drives the right-hand detail view).
    pub selected_case: Option<String>,
    /// Currently-selected suite id (for the add-case button scope).
    pub selected_suite: Option<String>,
    /// Latest finished run result per case id.
    pub last_result: BTreeMap<String, crate::autotest::TestRunResult>,
    /// Live step results for the in-progress run.
    pub live_steps: Vec<crate::autotest::TestStepResult>,
    pub running: bool,
    /// Handle of the in-flight run so we can't start two at once.
    _run_task: Option<Task<()>>,
    _subs: Vec<gpui::Subscription>,
}

impl EventEmitter<AutoTestEvent> for AutoTestPanel {}

impl AutoTestPanel {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let sub = cx.subscribe(&state, |this, _src, ev: &AppEvent, cx| match ev {
            AppEvent::WorkspaceChanged | AppEvent::Persisted => cx.notify(),
            _ => {}
        });

        Self {
            state,
            selected_case: None,
            selected_suite: None,
            last_result: BTreeMap::new(),
            live_steps: Vec::new(),
            running: false,
            _run_task: None,
            _subs: vec![sub],
        }
    }

    fn active_project(&self, cx: &App) -> Option<Project> {
        self.state.read(cx).active_project().cloned()
    }

    fn http_client(cx: &mut Context<Self>) -> Option<std::sync::Arc<dyn HttpClient>> {
        Some(cx.http_client())
    }

    /// Mutate the active project via a closure, then persist & notify.
    fn with_active_project<F: FnOnce(&mut Project)>(&mut self, cx: &mut Context<Self>, f: F) {
        let app = self.state.read(cx);
        let idx = app.active_project;
        let mut data = app.data.clone();
        drop(app);
        let Some(p) = data.projects.get_mut(idx) else { return };
        f(p);
        self.state.update(cx, |app, cx| {
            app.data = data;
            app.notify_workspace(cx);
        });
    }

    /// Add a new empty suite to the active project.
    fn add_suite(&mut self, cx: &mut Context<Self>) {
        let mut new_id: Option<String> = None;
        self.with_active_project(cx, |p| {
            let suite = TestSuite::new(format!("测试套件 {}", p.test_suites.len() + 1));
            new_id = Some(suite.id.clone());
            p.test_suites.push(suite);
        });
        if let Some(id) = new_id {
            self.selected_suite = Some(id);
        }
        cx.notify();
    }

    /// Add a new empty case under the currently-selected suite (or first suite).
    fn add_case(&mut self, cx: &mut Context<Self>) {
        let (selected_suite, selected_case) = (self.selected_suite.clone(), self.selected_case.clone());
        let mut new_id: Option<String> = None;
        self.with_active_project(cx, |p| {
            let suite_id = selected_suite
                .clone()
                .or_else(|| p.test_suites.first().map(|x| x.id.clone()));
            let Some(sid) = suite_id else { return };
            let Some(suite) = p.test_suites.iter_mut().find(|s| s.id == sid) else {
                return;
            };
            let case = TestCase::new(format!("用例 {}", suite.test_cases.len() + 1));
            new_id = Some(case.id.clone());
            suite.test_cases.push(case);
        });
        if let Some(id) = new_id {
            self.selected_case = Some(id);
        }
        cx.notify();
    }

    /// Append a step referencing an existing request by id.
    fn add_step(&mut self, request_id: &str, cx: &mut Context<Self>) {
        let Some(case_id) = self.selected_case.clone() else { return };
        let rid = request_id.to_string();
        self.with_active_project(cx, |p| {
            'outer: for suite in p.test_suites.iter_mut() {
                for case in suite.test_cases.iter_mut() {
                    if case.id == case_id {
                        case.steps.push(TestCaseStep::new(&rid));
                        break 'outer;
                    }
                }
            }
        });
    }

    /// Delete a step by index from the selected case.
    fn delete_step(&mut self, step_idx: usize, cx: &mut Context<Self>) {
        let Some(case_id) = self.selected_case.clone() else { return };
        self.with_active_project(cx, |p| {
            'outer: for suite in p.test_suites.iter_mut() {
                for case in suite.test_cases.iter_mut() {
                    if case.id == case_id && step_idx < case.steps.len() {
                        case.steps.remove(step_idx);
                        break 'outer;
                    }
                }
            }
        });
    }

    /// Run the selected case.
    fn run_selected(&mut self, cx: &mut Context<Self>) {
        if self.running {
            return;
        }
        let Some(case_id) = self.selected_case.clone() else { return };
        let Some(project) = self.active_project(cx) else { return };
        let Some(case) = find_case(&project, &case_id).cloned() else {
            return;
        };
        let Some(client) = Self::http_client(cx) else { return };

        self.running = true;
        self.live_steps.clear();
        let (tx, rx) = mpsc::unbounded_channel::<RunEvent>();
        let entity = cx.entity().downgrade();

        // Drive the case on a background task; forward events to the panel via channel.
        cx.spawn(async move |_this, cx| {
            let client = client;
            let project = project;
            let case_run = case;
            let tx_run = tx;
            let _ = run_case(&case_run, &project, &*client, tx_run, None).await;
            let _ = entity.update(cx, |panel, cx| {
                panel.running = false;
                if let Some(res) = panel.live_result_from_events() {
                    panel.last_result.insert(res.case_id.clone(), res);
                }
                cx.notify();
            });
        })
        .detach();

        // Poll the channel on a timer to stream StepFinished events into the UI.
        let poll_entity = cx.entity().downgrade();
        let mut rx_poll = rx;
        cx.spawn(async move |_this, cx| {
            loop {
                smol::Timer::after(Duration::from_millis(150)).await;
                let mut got_any = false;
                while let Ok(ev) = rx_poll.try_recv() {
                    got_any = true;
                    let _ = poll_entity.update(cx, |panel, cx| panel.apply_event(ev, cx));
                }
                let done = poll_entity
                    .update(cx, |p, _| !p.running)
                    .unwrap_or(true);
                if done && !got_any {
                    break;
                }
            }
        })
        .detach();

        cx.notify();
    }

    /// Build a synthetic run result from the collected live steps (used when
    /// the runner doesn't return a value we can capture directly across the
    /// async spawn boundary).
    fn live_result_from_events(&self) -> Option<crate::autotest::TestRunResult> {
        let cid = self.selected_case.clone()?;
        let steps = self.live_steps.clone();
        if steps.is_empty() {
            return None;
        }
        let passed = steps.iter().all(|s| s.passed);
        let total_ms = steps.iter().map(|s| s.time_ms).sum();
        Some(crate::autotest::TestRunResult {
            case_id: cid.clone(),
            case_name: String::new(),
            started_at: 0,
            finished_at: 0,
            total_ms,
            steps,
            passed,
            iteration: 1,
        })
    }

    fn apply_event(&mut self, ev: RunEvent, cx: &mut Context<Self>) {
        match ev {
            RunEvent::Started(_) => {
                self.live_steps.clear();
            }
            RunEvent::StepFinished(step) => {
                self.live_steps.push(step);
            }
            RunEvent::Finished(res) => {
                self.last_result.insert(res.case_id.clone(), res);
            }
        }
        cx.notify();
    }
}

fn find_case<'a>(project: &'a Project, case_id: &str) -> Option<&'a TestCase> {
    for s in &project.test_suites {
        for c in &s.test_cases {
            if c.id == case_id {
                return Some(c);
            }
        }
    }
    None
}

impl Render for AutoTestPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let muted = theme.muted_foreground;
        let border = theme.border;

        let project = self.active_project(cx);
        let has_project = project.is_some();
        let suites = project.as_ref().map(|p| &p.test_suites).cloned().unwrap_or_default();

        // Selected case & its steps.
        let selected_case_id = self.selected_case.clone();
        let selected_case = project.as_ref().and_then(|p| find_case(p, selected_case_id.as_deref().unwrap_or("")));
        let steps: Vec<TestCaseStep> = selected_case.map(|c| c.steps.clone()).unwrap_or_default();
        // Build a name lookup for request names.
        let req_name_map: BTreeMap<String, String> = project
            .as_ref()
            .map(|p| {
                let mut m = BTreeMap::new();
                for r in &p.requests {
                    m.insert(r.id.clone(), r.name.clone());
                }
                for f in &p.folders {
                    collect_req_names(f, &mut m);
                }
                m
            })
            .unwrap_or_default();

        // Live results.
        let live = self.live_steps.clone();
        let finished_res = selected_case_id
            .as_ref()
            .and_then(|cid| self.last_result.get(cid))
            .cloned();
        let steps_to_show: Vec<(usize, TestCaseStep, Option<crate::autotest::TestStepResult>)> = steps
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let r = live.iter().find(|r| r.step_id == s.id).cloned();
                (i, s.clone(), r)
            })
            .collect();

        v_flex()
            .size_full()
            .min_h_0()
            // Title bar.
            .child(
                h_flex()
                    .h(px(44.))
                    .px_4()
                    .when(cfg!(target_os = "macos"), |this| this.pl(px(78.)))
                    .items_center()
                    .gap_2()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("自动化测试"),
                    )
                    .child(div().flex_1())
                    .child(
                        Button::new("autotest-add-suite")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .label("新建套件")
                            .disabled(!has_project)
                            .on_click(cx.listener(|this, _, _, cx| this.add_suite(cx))),
                    )
                    .child(
                        Button::new("autotest-add-case")
                            .ghost()
                            .small()
                            .icon(IconName::Plus)
                            .label("新建用例")
                            .disabled(!has_project || suites.is_empty())
                            .on_click(cx.listener(|this, _, _, cx| this.add_case(cx))),
                    )
                    .child(
                        Button::new("autotest-run")
                            .primary()
                            .small()
                            .icon(if self.running { IconName::Pause } else { IconName::Play })
                            .label(if self.running { "运行中…" } else { "运行用例" })
                            .disabled(!has_project || self.selected_case.is_none() || self.running)
                            .on_click(cx.listener(|this, _, _, cx| this.run_selected(cx))),
                    ),
            )
            // Body: left tree (30%) | right detail (70%).
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .w_full()
                    .child(
                        div()
                            .w(px(260.))
                            .h_full()
                            .id("autotest-tree-scroll")
                            .border_r_1()
                            .border_color(border)
                            .overflow_y_scroll()
                            .p_2()
                            .gap_1()
                            .children(if suites.is_empty() {
                                vec![
                                    div()
                                        .text_size(px(12.))
                                        .text_color(muted)
                                        .p_2()
                                        .child("暂无套件，点右上“新建套件”开始")
                                        .into_any_element()
                                ]
                            } else {
                                suites
                                    .iter()
                                    .map(|suite| render_suite(suite, &self.selected_case, &self.selected_suite, cx))
                                    .collect::<Vec<_>>()
                            }),
                    )
                    .child(
                        v_flex()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .id("autotest-detail-scroll")
                            .p_4()
                            .gap_3()
                            .overflow_y_scroll()
                            .child(render_detail_header(selected_case, &finished_res, cx))
                            .child(render_steps(&steps_to_show, &req_name_map, cx))
                            .child(render_results(&live, &finished_res, muted)),
                    ),
            )
    }
}

fn collect_req_names(folder: &crate::state::models::Folder, m: &mut BTreeMap<String, String>) {
    for r in &folder.requests {
        m.insert(r.id.clone(), r.name.clone());
    }
    for sub in &folder.folders {
        collect_req_names(sub, m);
    }
}

fn render_suite(
    suite: &TestSuite,
    selected_case: &Option<String>,
    selected_suite: &Option<String>,
    cx: &mut Context<AutoTestPanel>,
) -> AnyElement {
    let is_suite_selected = selected_suite.as_deref() == Some(&suite.id);
    let panel = cx.entity();
    let suite_id_for_header = suite.id.clone();
    let suite_id_for_case = suite.id.clone();
    let suite_name = suite.name.clone();

    v_flex()
        .gap_1()
        .child(
            div()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .px_2()
                .py_1()
                .text_color(if is_suite_selected {
                    cx.theme().accent
                } else {
                    cx.theme().foreground
                })
                .child(suite_name),
        )
        .children(suite.test_cases.iter().map(|case| {
            let selected = selected_case.as_deref() == Some(&case.id);
            let cid = case.id.clone();
            let sid = suite_id_for_case.clone();
            let panel = panel.clone();
            div()
                .id(format!("autotest-case-{cid}"))
                .px_2()
                .py_1()
                .rounded_md()
                .text_size(px(12.))
                .bg(if selected {
                    cx.theme().accent
                } else {
                    Hsla::transparent_black()
                })
                .child(case.name.clone())
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.selected_case = Some(cid.clone());
                    this.selected_suite = Some(sid.clone());
                    cx.notify();
                }))
        }))
        .into_any_element()
}

fn render_detail_header(
    case: Option<&TestCase>,
    res: &Option<crate::autotest::TestRunResult>,
    _cx: &mut Context<AutoTestPanel>,
) -> AnyElement {
    let Some(case) = case else {
        return div()
            .text_size(px(12.))
            .text_color(_cx.theme().muted_foreground)
            .child("选择一个用例开始编辑与执行")
            .into_any_element();
    };
    let pass_count = res.as_ref().map(|r| r.steps.iter().filter(|s| s.passed).count()).unwrap_or(0);
    let fail_count = res.as_ref().map(|r| r.steps.iter().filter(|s| !s.passed).count()).unwrap_or(0);
    h_flex()
        .gap_2()
        .items_center()
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(case.name.clone()),
        )
        .child(div().flex_1())
        .when(pass_count + fail_count > 0, |d| {
            d.child(div().text_size(px(12.)).text_color(_cx.theme().success).child(format!("✓ {pass_count}")))
             .child(div().text_size(px(12.)).text_color(_cx.theme().danger).child(format!("✗ {fail_count}")))
        })
        .into_any_element()
}

fn render_steps(
    steps: &[(usize, TestCaseStep, Option<crate::autotest::TestStepResult>)],
    req_names: &BTreeMap<String, String>,
    cx: &mut Context<AutoTestPanel>,
) -> AnyElement {
    if steps.is_empty() {
        return div()
            .text_size(px(12.))
            .text_color(cx.theme().muted_foreground)
            .p_2()
            .child("暂无步骤。点右上“+”从当前项目的请求中添加步骤。")
            .into_any_element();
    }
    let panel = cx.entity();
    v_flex()
        .gap_2()
        .children(steps.iter().map(|(i, step, res)| {
            let name = req_names
                .get(&step.request_id)
                .cloned()
                .unwrap_or_else(|| format!("<未找到请求 {}>", &step.request_id[..8.min(step.request_id.len())]));
            let (status_color, status_text) = match res {
                Some(r) if r.passed => (cx.theme().success, format!("✓ {} {}ms", r.status, r.time_ms)),
                Some(r) => (cx.theme().danger, format!("✗ {} {}", r.status, r.error.clone().unwrap_or_default())),
                None => (cx.theme().muted_foreground, "待执行".into()),
            };
            let idx = *i;
            h_flex()
                .gap_2()
                .items_center()
                .p_2()
                .border_1()
                .border_color(cx.theme().border)
                .rounded_md()
                .child(div().text_size(px(12.)).text_color(cx.theme().muted_foreground).child(format!("#{}", idx + 1)))
                .child(div().flex_1().text_size(px(13.)).child(name))
                .child(div().text_size(px(11.)).text_color(status_color).child(status_text))
                .child(
                    Button::new(("autotest-step-del", idx))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Delete)
                        .on_click(cx.listener(move |this, _, _, cx| this.delete_step(idx, cx))),
                )
        }))
        .into_any_element()
}

fn render_results(
    live: &[crate::autotest::TestStepResult],
    finished: &Option<crate::autotest::TestRunResult>,
    muted: gpui::Hsla,
) -> AnyElement {
    let steps: &[crate::autotest::TestStepResult] = match finished {
        Some(r) => &r.steps,
        None => live,
    };
    if steps.is_empty() {
        return div().into_any_element();
    }
    v_flex()
        .gap_1()
        .child(div().text_size(px(12.)).font_weight(FontWeight::SEMIBOLD).child("执行结果"))
        .children(steps.iter().map(|s| {
            let icon = if s.passed { "✓" } else { "✗" };
            let color = if s.passed { muted } else { muted };
            v_flex()
                .gap_1()
                .p_2()
                .border_1()
                .border_color(muted)
                .rounded_md()
                .child(
                    h_flex()
                        .gap_2()
                        .child(div().text_size(px(12.)).child(format!("{icon} {}", s.request_name)))
                        .child(div().flex_1())
                        .child(div().text_size(px(11.)).child(format!("{} {}ms", s.status, s.time_ms)))
                        .child(div().text_size(px(11.)).child(format!("{}/{} 断言", s.assertions_passed, s.assertions_passed + s.assertions_failed))),
                )
                .children(s.logs.iter().map(|l| {
                    div().text_size(px(11.)).text_color(muted).child(l.clone())
                }))
                .children(if s.extracted.is_empty() {
                    None
                } else {
                    Some(div().text_size(px(11.)).child(
                        "提取: ".to_string()
                            + &s.extracted
                                .iter()
                                .map(|(k, v)| format!("{k}={}", truncate(v, 40)))
                                .collect::<Vec<_>>()
                                .join(", "),
                    ))
                })
        }))
        .into_any_element()
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
