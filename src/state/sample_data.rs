//! Sample/demo project so the first launch is not an empty workspace.
//! Mirrors the PRD's "用户中心" example.

use super::models::*;

pub fn demo_workspace() -> WorkspaceData {
    let mut project = Project::new("用户中心");
    project.description = "示例项目：演示前后端协作开发的认证模块。".to_string();

    // Environments
    let mut dev = Environment::new("开发环境");
    dev.variables = vec![
        KeyValue::new("baseUrl", "https://httpbin.org"),
        KeyValue::new("username", "demo"),
        KeyValue::new("password", "p@ssw0rd"),
    ];
    let mut test = Environment::new("测试环境");
    test.variables = vec![KeyValue::new("baseUrl", "https://httpbin.org")];
    let prod = Environment::new("生产环境");
    project.environments = vec![dev.clone(), test, prod];
    project.active_environment = Some(dev.id);

    // Global variables
    project.global_variables = vec![KeyValue::new("token", "")];

    // 认证模块 folder
    let mut auth = Folder::new("认证模块");

    let mut login = ApiRequest::new("用户登录", RequestMethod::Post, "{{baseUrl}}/post");
    login.body.body_type = BodyType::Raw;
    login.body.raw_language = RawLanguage::Json;
    login.body.raw = serde_json::to_string_pretty(&serde_json::json!({
        "username": "{{username}}",
        "password": "{{password}}"
    }))
    .unwrap();
    login.description =
        "用户登录接口。\n\n请求成功后将 `token` 写入全局变量 `token` 供后续接口使用。".to_string();
    login.mock = Some(MockRule {
        // Note: match_path gets backfilled by crate::mock::backfill_path_patterns
        // at serve time — empty Exact means "use the request's own URL path".
        enabled: true,
        status: 200,
        headers: vec![KeyValue::new("Content-Type", "application/json")],
        body: serde_json::to_string_pretty(&serde_json::json!({
            "code": 0,
            "message": "ok",
            "data": { "token": "mock-token-123456", "expires_in": 3600 }
        }))
        .unwrap(),
        delay_ms: 0,
        match_method: None,
        match_path: PathPattern::Exact(String::new()),
        match_query: Vec::new(),
        match_headers: Vec::new(),
        enable_templates: false,
    });
    // Tests script: extract the token from the response and store it in the
    // `token` environment variable, then assert (PRD §7.5 workflow).
    login.tests_script = "// Extract token from the JSON response and store it\n\
        // in the `token` variable for subsequent requests.\n\
        if (response.json && response.json.data) {\n\
        \x20\x20apt.setVariable('token', response.json.data.token);\n\
        \x20\x20apt.echo('saved token: ' + response.json.data.token);\n\
        }\n\
        apt.assert(response.status === 200, 'status is 200');\n\
        apt.assert(response.json.code === 0, 'code is 0');\n"
        .to_string();

    let get_info = ApiRequest::new("查询个人信息", RequestMethod::Get, "{{baseUrl}}/get");
    let mut get_info = get_info;
    get_info.headers = vec![KeyValue::new("Authorization", "Bearer {{token}}")];
    get_info.params = vec![KeyValue::new("detail", "1")];
    // Demo the Cookie + path-variable tabs.
    get_info.cookies = vec![KeyValue::new("session", "abc123")];
    get_info.path = vec![KeyValue::new("userId", "42")];
    // Demo the Auth tab (Bearer) + a protocol (HTTP).
    get_info.auth = crate::state::models::AuthConfig {
        auth_type: crate::state::models::AuthType::Bearer,
        token: "{{token}}".into(),
        ..Default::default()
    };
    get_info.status = "已发布".to_string();
    get_info.tags = vec!["用户".to_string(), "只读".to_string()];

    // A few more requests so the interface list shows multiple rows with
    // varied methods, statuses, and timestamps.
    let mut logout = ApiRequest::new("退出登录", RequestMethod::Post, "{{baseUrl}}/auth/logout");
    logout.status = "开发中".to_string();
    logout.tags = vec!["用户".to_string()];

    let mut refresh = ApiRequest::new("刷新令牌", RequestMethod::Post, "{{baseUrl}}/auth/refresh");
    refresh.status = "已发布".to_string();
    refresh.tags = vec!["鉴权".to_string()];

    let mut reset = ApiRequest::new("重置密码", RequestMethod::Post, "{{baseUrl}}/auth/reset");
    reset.status = "废弃".to_string();

    let mut send_code = ApiRequest::new("发送验证码", RequestMethod::Post, "{{baseUrl}}/auth/code");
    send_code.status = "已发布".to_string();
    send_code.tags = vec!["鉴权".to_string(), "短信".to_string()];

    let mut verify = ApiRequest::new("校验验证码", RequestMethod::Get, "{{baseUrl}}/auth/verify");
    verify.status = "开发中".to_string();
    verify.tags = vec!["鉴权".to_string()];

    auth.requests = vec![login, get_info, logout, refresh, reset, send_code, verify];
    // Folder-level metadata: a description, a shared query param, a header,
    // and a folder variable — all surfaced in the folder detail view.
    auth.description = "认证相关接口集合：登录、获取用户信息等。统一使用 Bearer 鉴权。".to_string();
    auth.params = vec![KeyValue::new("version", "v2")];
    auth.headers = vec![KeyValue::new("X-Client", "verve")];
    auth.variables = vec![
        KeyValue::new("clientId", "verve-demo"),
        KeyValue::new("tenant", "default"),
    ];

    // Other APIs at project root
    let list = ApiRequest::new("用户列表", RequestMethod::Get, "{{baseUrl}}/get");
    let create = ApiRequest::new("创建用户", RequestMethod::Post, "{{baseUrl}}/post");

    project.folders = vec![auth];
    project.requests = vec![list, create];

    WorkspaceData {
        projects: vec![project],
        history: Vec::new(),
        active_project_id: None,
    }
}
