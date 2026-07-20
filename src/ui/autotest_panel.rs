//! Automated testing panel — the full-bleed "自动化测试" exclusive view.
//!
//! Lists suites/cases from the active project, lets the user add/remove steps
//! referencing existing ApiRequests *or* flow-control steps (if/loop/wait/script),
//! and runs cases through [`autotest::runner::run_case`]. Results (per-step
//! status, assertion counts, extracted variables) stream in via an
//! `mpsc::UnboundedReceiver` polled in the background.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

use gpui::prelude::FluentBuilder as _;
use gpui::*;
use gpui_component::{
    button::{Button, ButtonVariants as _},
    h_flex, v_flex, ActiveTheme, Disableable as _, IconName, Sizable as _,
};
use gpui_component::WindowExt as _;
use http_client::HttpClient;
use tokio::sync::mpsc;

use crate::autotest::models::{
    IfStep, LoopMode, LoopStep, RequestStep, ScriptStep, TestCase, TestCaseStep, TestSuite, WaitStep,
};
use crate::autotest::runner::{run_case, RunEvent};
use crate::state::models::{Folder, Project};
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
    /// Whether the "add step" request tree picker is open.
    show_add_picker: bool,
    /// Expanded folders in the add-picker request tree.
    picker_expanded: BTreeSet<String>,
    /// Expanded suites in the left suite/case tree.
    suite_expanded: BTreeSet<String>,
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
            show_add_picker: false,
            picker_expanded: BTreeSet::new(),
            suite_expanded: BTreeSet::new(),
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
        let _ = app;
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
            self.suite_expanded.insert(id.clone());
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

    /// Append a request step referencing an existing ApiRequest by id.
    fn add_step(&mut self, request_id: &str, cx: &mut Context<Self>) {
        let Some(case_id) = self.selected_case.clone() else { return };
        let rid = request_id.to_string();
        self.with_active_project(cx, |p| {
            'outer: for suite in p.test_suites.iter_mut() {
                for case in suite.test_cases.iter_mut() {
                    if case.id == case_id {
                        case.steps.push(TestCaseStep::new_request(&rid));
                        break 'outer;
                    }
                }
            }
        });
        self.show_add_picker = false;
        cx.notify();
    }

    /// Append a control-flow step.
    fn add_control_step(&mut self, step: TestCaseStep, cx: &mut Context<Self>) {
        let Some(case_id) = self.selected_case.clone() else { return };
        self.with_active_project(cx, |p| {
            'outer: for suite in p.test_suites.iter_mut() {
                for case in suite.test_cases.iter_mut() {
                    if case.id == case_id {
                        case.steps.push(step);
                        break 'outer;
                    }
                }
            }
        });
        cx.notify();
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

    fn toggle_suite(&mut self, suite_id: &str, cx: &mut Context<Self>) {
        if self.suite_expanded.contains(suite_id) {
            self.suite_expanded.remove(suite_id);
        } else {
            self.suite_expanded.insert(suite_id.to_string());
        }
        cx.notify();
    }

    fn select_case(&mut self, case_id: &str, suite_id: &str, cx: &mut Context<Self>) {
        self.selected_case = Some(case_id.to_string());
        self.selected_suite = Some(suite_id.to_string());
        cx.notify();
    }

    fn select_suite(&mut self, suite_id: &str, cx: &mut Context<Self>) {
        self.selected_suite = Some(suite_id.to_string());
        cx.notify();
    }

    fn toggle_picker_folder(&mut self, folder_id: &str, cx: &mut Context<Self>) {
        if self.picker_expanded.contains(folder_id) {
            self.picker_expanded.remove(folder_id);
        } else {
            self.picker_expanded.insert(folder_id.to_string());
        }
        cx.notify();
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

    /// Build a synthetic run result from the collected live steps.
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
                let r = live.iter().find(|r| r.step_id == s.id()).cloned();
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
            // Body: left tree (260px) | right detail (flex 1).
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
                                    .map(|suite| self::render_suite(suite, &self.selected_case, &self.selected_suite, &self.suite_expanded, cx))
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
                            .child(self::render_detail_header(selected_case, &finished_res, cx))
                            // Step toolbar
                            .child(
                                h_flex()
                                    .gap_2()
                                    .items_center()
                                    .flex_wrap()
                                    .child(
                                        Button::new("autotest-add-request-step")
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::Plus)
                                            .label("添加请求步骤")
                                            .disabled(self.selected_case.is_none())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.show_add_picker = true;
                                                cx.notify();
                                            })),
                                    )
                                    .child(
                                        Button::new("autotest-add-if")
                                            .ghost()
                                            .xsmall()
                                            .label("条件判断 (If)")
                                            .disabled(self.selected_case.is_none())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_control_step(TestCaseStep::new_if("response.status === 200"), cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("autotest-add-repeat")
                                            .ghost()
                                            .xsmall()
                                            .label("重复循环")
                                            .disabled(self.selected_case.is_none())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_control_step(TestCaseStep::new_repeat(3), cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("autotest-add-for-each")
                                            .ghost()
                                            .xsmall()
                                            .label("遍历循环")
                                            .disabled(self.selected_case.is_none())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_control_step(TestCaseStep::new_for_each("item", "response.json.data.items"), cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("autotest-add-while")
                                            .ghost()
                                            .xsmall()
                                            .label("条件循环")
                                            .disabled(self.selected_case.is_none())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_control_step(TestCaseStep::new_while("retry < 3"), cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("autotest-add-wait")
                                            .ghost()
                                            .xsmall()
                                            .label("等待")
                                            .disabled(self.selected_case.is_none())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_control_step(TestCaseStep::new_wait(1000), cx);
                                            })),
                                    )
                                    .child(
                                        Button::new("autotest-add-script")
                                            .ghost()
                                            .xsmall()
                                            .label("脚本")
                                            .disabled(self.selected_case.is_none())
                                            .on_click(cx.listener(|this, _, _, cx| {
                                                this.add_control_step(TestCaseStep::new_script("// 在此编写脚本\napt.log('hello');"), cx);
                                            })),
                                    ),
                            )
                            // Add-step request tree picker
                            .when(self.show_add_picker, |c| {
                                c.child(
                                    v_flex()
                                        .gap_1()
                                        .p_2()
                                        .border_1()
                                        .border_color(border)
                                        .rounded_md()
                                        .child(
                                            h_flex()
                                                .items_center()
                                                .gap_2()
                                                .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child("选择请求添加为步骤"))
                                                .child(div().flex_1())
                                                .child(
                                                    Button::new("autotest-picker-close")
                                                        .ghost()
                                                        .xsmall()
                                                        .icon(IconName::Close)
                                                        .on_click(cx.listener(|this, _, _, cx| {
                                                            this.show_add_picker = false;
                                                            cx.notify();
                                                        })),
                                                ),
                                        )
                                        .child(
                                            div()
                                                .id("autotest-picker-tree")
                                                .max_h(px(240.))
                                                .overflow_y_scroll()
                                                .children(
                                                    project.as_ref().map(|p| render_picker_tree(p, &self.picker_expanded, cx)).unwrap_or_default()
                                                ),
                                        ),
                                )
                            })
                            .child(self::render_steps(&steps_to_show, &req_name_map, cx))
                            .child(self::render_results(&live, &finished_res, muted)),
                    ),
            )
    }
}

fn collect_req_names(folder: &Folder, m: &mut BTreeMap<String, String>) {
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
    expanded: &BTreeSet<String>,
    cx: &Context<AutoTestPanel>,
) -> AnyElement {
    let is_suite_selected = selected_suite.as_deref() == Some(&suite.id);
    let is_expanded = expanded.contains(&suite.id);
    let panel = cx.entity();
    let suite_id_for_header = suite.id.clone();
    let suite_id_for_case = suite.id.clone();
    let suite_name = suite.name.clone();
    let sid = suite.id.clone();

    v_flex()
        .gap_1()
        .child(
            h_flex()
                .gap_1()
                .px_2()
                .py_1()
                .rounded_md()
                .items_center()
                .child(
                    div()
                        .id(format!("autotest-suite-chev-{}", suite.id.clone()))
                        .child(if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                        .text_size(px(10.))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_suite(&sid, cx))),
                )
                .child(
                    div()
                        .id(format!("autotest-suite-name-{}", suite.id.clone()))
                        .text_size(px(12.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .flex_1()
                        .text_color(if is_suite_selected {
                            cx.theme().accent
                        } else {
                            cx.theme().foreground
                        })
                        .child(suite_name)
                        .on_click(cx.listener(move |this, _, _, cx| this.select_suite(&suite_id_for_header, cx))),
                ),
        )
        .when(is_expanded, |v| {
            v.children(suite.test_cases.iter().map(|case| {
                let selected = selected_case.as_deref() == Some(&case.id);
                let cid = case.id.clone();
                let sid = suite_id_for_case.clone();
                div()
                    .id(format!("autotest-case-{cid}"))
                    .px_2()
                    .py_1()
                    .pl(px(22.))
                    .rounded_md()
                    .text_size(px(12.))
                    .bg(if selected {
                        cx.theme().accent
                    } else {
                        Hsla::transparent_black()
                    })
                    .child(case.name.clone())
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.select_case(&cid, &sid, cx);
                    }))
            }))
        })
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
    let pass_count = res.as_ref().map(|r| count_passed(&r.steps)).unwrap_or(0);
    let fail_count = res.as_ref().map(|r| count_failed(&r.steps)).unwrap_or(0);
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

fn count_passed(steps: &[crate::autotest::TestStepResult]) -> usize {
    let mut n = 0;
    for s in steps {
        if s.passed { n += 1; }
        n += count_passed(&s.children);
    }
    n
}

fn count_failed(steps: &[crate::autotest::TestStepResult]) -> usize {
    let mut n = 0;
    for s in steps {
        if !s.passed { n += 1; }
        n += count_failed(&s.children);
    }
    n
}

fn step_display_name(step: &TestCaseStep, req_names: &BTreeMap<String, String>) -> String {
    match step {
        TestCaseStep::Request(r) => req_names
            .get(&r.request_id)
            .cloned()
            .or(r.name_override.clone())
            .unwrap_or_else(|| format!("<未找到请求>", )),
        other => other.display_label(),
    }
}

fn step_kind_icon(step: &TestCaseStep) -> &'static str {
    match step {
        TestCaseStep::Request(_) => "🌐",
        TestCaseStep::If(_) => "🔀",
        TestCaseStep::Loop(_) => "🔁",
        TestCaseStep::Wait(_) => "⏱",
        TestCaseStep::Script(_) => "📜",
    }
}

fn render_steps(
    steps: &[(usize, TestCaseStep, Option<crate::autotest::TestStepResult>)],
    req_names: &BTreeMap<String, String>,
    cx: &Context<AutoTestPanel>,
) -> AnyElement {
    if steps.is_empty() {
        return div()
            .text_size(px(12.))
            .text_color(cx.theme().muted_foreground)
            .p_2()
            .child("暂无步骤。点上方按钮添加请求或流程控制步骤。")
            .into_any_element();
    }
    v_flex()
        .gap_2()
        .children(steps.iter().map(|(i, step, res)| {
            let name = self::step_display_name(step, req_names);
            let icon = self::step_kind_icon(step);
            let (status_color, status_text) = match res {
                Some(r) if r.passed => (cx.theme().success, format!("✓ {}ms", r.time_ms)),
                Some(r) => (cx.theme().danger, format!("✗ {}", r.error.clone().unwrap_or_default())),
                None => (cx.theme().muted_foreground, "待执行".into()),
            };
            let idx = *i;

            // Show method badge for request steps
            let method_badge = match step {
                TestCaseStep::Request(r) => req_names.get(&r.request_id).map(|_| {
                    // Look up the request id to show method color is complex; keep it simple
                    div().text_size(px(11.)).text_color(cx.theme().muted_foreground).child("REQ").into_any_element()
                }),
                _ => None,
            };

            let mut row = h_flex()
                .gap_2()
                .items_center()
                .p_2()
                .border_1()
                .border_color(cx.theme().border)
                .rounded_md()
                .child(div().text_size(px(12.)).text_color(cx.theme().muted_foreground).child(icon))
                .child(div().text_size(px(12.)).text_color(cx.theme().muted_foreground).child(format!("#{}", idx + 1)));

            if let Some(badge) = method_badge {
                row = row.child(badge);
            }

            row = row
                .child(div().flex_1().text_size(px(13.)).child(name))
                .child(div().text_size(px(11.)).text_color(status_color).child(status_text))
                .child(
                    Button::new(("autotest-step-del", idx))
                        .ghost()
                        .xsmall()
                        .icon(IconName::Delete)
                        .on_click(cx.listener(move |this, _, _, cx| this.delete_step(idx, cx))),
                );

            // Render nested children (if any results exist with children)
            let mut elem = row.into_any_element();
            if let Some(r) = res {
                if !r.children.is_empty() {
                    let mut children_elems = vec![elem];
                    children_elems.push(
                        div()
                            .pl(px(28.))
                            .gap_1()
                            .child(render_child_results(&r.children, cx))
                            .into_any_element()
                    );
                    elem = v_flex().gap_1().children(children_elems).into_any_element();
                }
            }
            elem
        }))
        .into_any_element()
}

fn render_child_results(children: &[crate::autotest::TestStepResult], cx: &Context<AutoTestPanel>) -> AnyElement {
    v_flex()
        .gap_1()
        .children(children.iter().map(|c| {
            let color = if c.passed { cx.theme().success } else { cx.theme().danger };
            let icon = if c.passed { "✓" } else { "✗" };
            let mut row = h_flex()
                .gap_2()
                .items_center()
                .child(div().text_size(px(11.)).text_color(color).child(icon))
                .child(div().text_size(px(12.)).flex_1().child(c.request_name.clone()))
                .child(div().text_size(px(10.)).text_color(cx.theme().muted_foreground).child(format!("{}ms", c.time_ms)));

            let mut base: AnyElement = row.into_any_element();
            if !c.children.is_empty() {
                let parent = v_flex()
                    .gap_1()
                    .child(base)
                    .child(div().pl(px(16.)).child(render_child_results(&c.children, cx)));
                base = parent.into_any_element();
            }
            base
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

// ---------------------------------------------------------------------------
// Add-step request tree picker
// ---------------------------------------------------------------------------

fn render_picker_tree(project: &Project, expanded: &BTreeSet<String>, cx: &Context<AutoTestPanel>) -> Vec<AnyElement> {
    let mut items = Vec::new();
    for folder in &project.folders {
        render_picker_folder(folder, 0, expanded, cx, &mut items);
    }
    for req in &project.requests {
        let rid = req.id.clone();
        let method = req.method;
        items.push(
            h_flex()
                .id(format!("autotest-picker-top-req-{}", req.id.clone()))
                .gap_2()
                .px_2()
                .py_1()
                .rounded_md()
                .hover(|this| this.bg(cx.theme().accent.opacity(0.3)))
                .child(div().w(px(16.)))
                .child(
                    div()
                        .text_size(px(11.))
                        .w(px(40.))
                        .text_color(crate::ui::method_colors::badge_color(method, cx))
                        .child(method.badge_label()),
                )
                .child(div().text_size(px(12.)).flex_1().child(req.name.clone()))
                .on_click(cx.listener(move |this, _, _, cx| this.add_step(&rid, cx)))
                .into_any_element(),
        );
    }
    if items.is_empty() {
        items.push(
            div()
                .text_size(px(12.))
                .text_color(cx.theme().muted_foreground)
                .p_2()
                .child("当前项目暂无请求")
                .into_any_element(),
        );
    }
    items
}

fn render_picker_folder(
    folder: &Folder,
    depth: usize,
    expanded: &BTreeSet<String>,
    cx: &Context<AutoTestPanel>,
    items: &mut Vec<AnyElement>,
) {
    let is_expanded = expanded.contains(&folder.id);
    let fid = folder.id.clone();
    let indent = depth as f32 * 14.;

    items.push(
        h_flex()
            .id(format!("autotest-picker-folder-{}", folder.id.clone()))
            .gap_1()
            .px_2()
            .py_1()
            .rounded_md()
            .pl(px(indent + 8.))
            .items_center()
            .child(
                div()
                    .id(format!("autotest-picker-folder-chev-{}", folder.id.clone()))
                    .child(if is_expanded { IconName::ChevronDown } else { IconName::ChevronRight })
                    .text_size(px(10.))
                    .on_click(cx.listener(move |this, _, _, cx| this.toggle_picker_folder(&fid, cx))),
            )
            .child(div().text_size(px(12.)).font_weight(FontWeight::MEDIUM).child(folder.name.clone()))
            .into_any_element(),
    );

    if is_expanded {
        for sub in &folder.folders {
            render_picker_folder(sub, depth + 1, expanded, cx, items);
        }
        for req in &folder.requests {
            let rid = req.id.clone();
            let method = req.method;
            items.push(
                h_flex()
                    .id(format!("autotest-picker-nested-req-{}", req.id.clone()))
                    .gap_2()
                    .px_2()
                    .py_1()
                    .rounded_md()
                    .hover(|this| this.bg(cx.theme().accent.opacity(0.3)))
                    .pl(px(indent + 8. + 16.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .w(px(40.))
                            .text_color(crate::ui::method_colors::badge_color(method, cx))
                            .child(method.badge_label()),
                    )
                    .child(div().text_size(px(12.)).flex_1().child(req.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| this.add_step(&rid, cx)))
                    .into_any_element(),
            );
        }
    }
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
