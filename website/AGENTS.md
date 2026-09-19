# Verve Website — 构建与发布指南

本文件固化网站的开发与发布流程，供下次直接照做。**域名/部署仓库均保持现状，勿擅自改动。**

---

## 1. 仓库拓扑

| 角色 | 仓库 | 分支 | 说明 |
|---|---|---|---|
| **工作仓库**（website/ 源码） | `lijingrs/verve` | `dev` | 开发在这里进行 |
| **部署目标** | `aios-pub/verve` | `gh-pages` | 独立仓库（**非 fork**），Pages 从此分支提供 |

- 两仓库相互独立，没有 fork 关系。
- 站点线上地址：`https://aios-pub.github.io/verve/`
  - 经组织自定义域 `aios.pub` 重定向，实际访问到 `https://aios.pub/verve/`。
  - 部署仓库 `aios-pub/verve` 的**默认分支是 `main`**（项目源码）；`gh-pages` 是部署专用的 orphan 分支，只含构建产物。

---

## 2. 技术栈

`website/` 是**独立 crate**（不在根 workspace）。依赖：

- **Leptos 0.8**（CSR 模式）+ `leptos_meta` + `leptos_router`
- **Trunk 0.21** 作为构建/打包器（**不是** cargo-leptos —— cargo-leptos 无 SSG 参数）
- **Tailwind CSS v4**（`@tailwindcss/cli`，经 Trunk hook 在构建前编译）
- **leptos_i18n 0.6**：`build.rs` + `leptos_i18n_build`，YAML 在 `i18n/`（`zh-CN` 默认 + `en`）
- **leptos-use 0.18**（鼠标聚光、滚动显现等）
- 目标：`wasm32-unknown-unknown`

i18n 代码生成：`build.rs` 用 `Config::new("zh-CN")?.add_locale("en")?`，源码通过
`include!(concat!(env!("OUT_DIR"), "/i18n/mod.rs"))` 引入（**勿用** `load_locales!()` 宏，会报 metadata 错误）。

---

## 3. 本地开发

```sh
cd website
trunk serve          # 127.0.0.1:8080，热重载
```

前置：已安装 `wasm32-unknown-unknown` target、Trunk、Node/npx（Tailwind CLI 经 npx 调用）。

---

## 4. 发布流程（预构建 dist → 推 aios-pub/verve:gh-pages）

采用**预构建产物直接推送**到部署仓库的 orphan `gh-pages` 分支。每次更新站点都需手动重建 + 推送（无 CI）。

### 4.1 构建

```sh
cd website
trunk build --release          # 产物输出到 dist/
```

### 4.2 准备 dist

- `dist/` **必须含 `.nojekyll`**（空文件即可）——否则 Jekyll 处理会丢掉带 `_` 的 wasm 文件名（如 `website-<hash>_bg.wasm`）。
- 首次发布/缺失时创建：`touch dist/.nojekyll`

### 4.3 推送到 gh-pages

部署仓库 `aios-pub/verve` 与工作仓库独立，用**临时 git clone 方式**推送（不污染工作仓库）：

```sh
TMP="$(mktemp -d)"
cd "$TMP"
git init -q
git config user.name  "Jing.Li"
git config user.email "lijingrs@users.noreply.github.com"
git checkout -q -b gh-pages
cp -R "/Users/lijing/CodeProjects/verve/website/dist/." .
TOKEN="$(gh auth token)"
git remote add deploy "https://x-access-token:${TOKEN}@github.com/aios-pub/verve.git"
git add -A
git commit -q -m "Deploy website: <简述>"
git push -u deploy gh-pages      # 首次会创建该分支；后续为 fast-forward 更新
cd - && rm -rf "$TMP"
```

### 4.4 确认 Pages 配置

```sh
gh api repos/aios-pub/verve/pages \
  --jq '"status=\(.status) branch=\(.source.branch)/\(.source.path)"'
# 期望：status=built  branch=gh-pages/
```

若未启用 Pages（新环境）：
```sh
gh api -X POST repos/aios-pub/verve/pages \
  -f "source[branch]=gh-pages" -f "source[path]=/"
```

### 4.5 验证上线

```sh
# HTML 与资源都应 200
curl -sL -o /dev/null -w "%{http_code}\n" https://aios-pub.github.io/verve/
curl -sL -o /dev/null -w "%{http_code}\n" "https://aios.pub/verve/$( \
  curl -sL https://aios.pub/verve/ | grep -oE 'website-[a-f0-9]+\.js' | head -1)"
```

> ⚠️ **常见坑**：`aios-pub/verve` 仓库**大小写**决定 Pages 路径。曾因仓库大小写（`Verve` vs `verve`）与构建 base path 不一致导致全站资源 404、白屏。重命名为小写 `verve` 后修复。**改 base path 或动仓库名时务必保持两者一致。**

---

## 5. 路径架构（改 base path 只动这几处）

站点从 `/verve/` 子路径提供，路径逻辑**集中在少数几处**：

| 文件 | 位置 | 当前值 |
|---|---|---|
| `src/consts.rs` | `BASE_PATH` | `"/verve"` |
| `src/lib.rs` | `<Router base="...">` | `"/verve"` |
| `Trunk.toml` | `public_url` | `"/verve/"` |
| `src/pages/docs/cloud.rs` | 两处 `href` | `/verve/docs`、`/verve/docs/verve-server` |
| `src/pages/docs/verve_server.rs` | 一处 `href` | `/verve/docs` |

`src/utils.rs` 的 `asset()` / `route()` 自动跟随 `BASE_PATH`，**无需改动**；`rel()` 用于 `<Router>` 内部链接（不带 base）。

切换到根路径/子域名部署时，把上面 5 处的 `/verve` 前缀清空即可。

---

## 6. 注意事项

- `dist/` 已在工作仓库 `.gitignore` —— **勿把构建产物提交到工作仓库**。
- 发布后检查工作仓库 `dev` 分支是否领先 `origin/dev` 未推送（`git log --oneline origin/dev..dev`）；发布用临时 clone 推到部署仓库，不影响工作仓库的推送状态。
- 部署仓库 `aios-pub/verve` 的 `main` 是项目源码，**勿向其推网站产物**；网站只在 `gh-pages`。
- 主题：默认亮色，`<html class="dark">` 切暗色；防闪烁脚本在 `index.html` `<head>` 最前（读 localStorage `leptos-use-color-scheme`）。
