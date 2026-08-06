# Verve Website — 构建与发布指南

本文件供 Claude Code 会话遵循，固化网站的开发与发布流程。**域名/部署仓库均保持现状，勿擅自改动。**
完整背景见 `AGENTS.md`；本文件是它在 Claude Code 工作流下的可执行摘要。

---

## 1. 仓库拓扑

| 角色 | 仓库 | 分支 | 说明 |
|---|---|---|---|
| **工作仓库**（website/ 源码） | `lijingrs/verve` | `dev` | 开发在这里进行 |
| **部署目标** | `aios-rs/verve` | `gh-pages` | 独立仓库（**非 fork**），Pages 从此分支提供 |

- 两仓库相互独立，没有 fork 关系。
- 线上地址：`https://aios.pub/verve/`（经自定义域重定向到 `https://aios-rs.github.io/verve/`）。
- 部署仓库默认分支是 `main`（项目源码）；`gh-pages` 是 orphan 部署分支，只含构建产物。

---

## 2. 技术栈

`website/` 是**独立 crate**（不在根 workspace）。依赖：

- **Leptos 0.8**（CSR）+ `leptos_meta` + `leptos_router`
- **Trunk 0.21** 构建/打包（**不是** cargo-leptos）
- **Tailwind CSS v4**（`@tailwindcss/cli`，经 Trunk `pre_build` hook 在资源扫描前编译）
- **leptos_i18n 0.6**：`build.rs` + `leptos_i18n_build`，YAML 在 `i18n/`（`zh-CN` 默认 + `en`）
- **leptos-use 0.18**；目标：`wasm32-unknown-unknown`

i18n 用 `include!(concat!(env!("OUT_DIR"), "/i18n/mod.rs"))` 引入，**勿用** `load_locales!()` 宏。

---

## 3. 本地开发（改完必须这样验证）

```sh
cd website
trunk serve          # http://127.0.0.1:8080，热重载
```

前置：`wasm32-unknown-unknown` target、Trunk、Node/npx（Tailwind CLI 经 npx）。

### 关于 wasm-opt

`index.html` 的 `<link data-trunk rel="rust" data-wasm-opt="z">` 会让 Trunk 下载 binaryen 做 wasm 优化。
若网络无法访问 GitHub release 资源（下载 `binaryen-...tar.gz` 超时），用 Homebrew 安装本地 binaryen：

```sh
brew install binaryen     # 提供 wasm-opt
```

### 验证改动

- **不要只靠 `cargo check`**——它不跑 Trunk 的资源管线，发现不了 `index.html`/CSS/hook 层面的问题。
- 改完样式或页面结构后，必须 `trunk serve` 起得来，并在浏览器里打开受影响页面目视确认。
- `public/tailwind.css` 是构建产物（`.gitignore` 忽略），由 Trunk hook 生成，**不要手改、不要提交**。

---

## 4. 发布流程（预构建 dist → 推 aios-rs/verve:gh-pages）

无 CI，每次手动重建 + 推送。

### 4.1 构建

```sh
cd website
trunk build --release          # 产物输出到 dist/
```

### 4.2 dist 必须含 `.nojekyll`

否则 Jekyll 会丢掉带 `_` 的 wasm 文件名（如 `website-<hash>_bg.wasm`）。缺失时：`touch dist/.nojekyll`。

### 4.3 推送到 gh-pages（临时 clone，不污染工作仓库）

```sh
TMP="$(mktemp -d)"; cd "$TMP"
git init -q
git config user.name  "Jing.Li"
git config user.email "lijingrs@users.noreply.github.com"
git checkout -q -b gh-pages
cp -R "/Users/lijing/CodeProjects/verve/website/dist/." .
TOKEN="$(gh auth token)"
git remote add deploy "https://x-access-token:${TOKEN}@github.com/aios-rs/verve.git"
git add -A
git commit -q -m "Deploy website: <简述>"
git push -u deploy gh-pages
cd - && rm -rf "$TMP"
```

> 注：本地实际工作路径可能是 `/Volumes/PSSD/CodeProjects/verve/website`；`cp` 源路径按当前机器调整。

### 4.4 确认 Pages 与上线

```sh
gh api repos/aios-rs/verve/pages \
  --jq '"status=\(.status) branch=\(.source.branch)/\(.source.path)"'
# 期望：status=built  branch=gh-pages/

curl -sL -o /dev/null -w "%{http_code}\n" https://aios-rs.github.io/verve/
curl -sL -o /dev/null -w "%{http_code}\n" "https://aios.pub/verve/$( \
  curl -sL https://aios.pub/verve/ | grep -oE 'website-[a-f0-9]+\.js' | head -1)"
```

> ⚠️ **常见坑**：`aios-rs/verve` 仓库名**小写** `verve` 决定 Pages 路径；改 base path 或动仓库名时必须与构建 base path 一致，否则全站资源 404、白屏。

---

## 5. 路径架构（改 base path 只动这几处）

站点从 `/verve/` 子路径提供：

| 文件 | 位置 | 当前值 |
|---|---|---|
| `src/consts.rs` | `BASE_PATH` | `"/verve"` |
| `src/lib.rs` | `<Router base="...">` | `"/verve"` |
| `Trunk.toml` | `public_url` | `"/verve/"` |
| `src/pages/docs/cloud.rs` | 两处 `href` | `/verve/docs`、`/verve/docs/verve-server` |
| `src/pages/docs/verve_server.rs` | 一处 `href` | `/verve/docs` |

`src/utils.rs` 的 `asset()` / `route()` 自动跟随 `BASE_PATH`，**无需改动**；`rel()` 用于 Router 内部链接。

---

## 6. 样式约定（避免再犯的坑）

- **`.bg-grid` 带径向 `mask-image`**，遮罩会裁掉它所在元素的全部子内容。因此网格背景**必须**放在独立的绝对定位层上（`class="bg-grid pointer-events-none absolute inset-0"`），**不要**直接把 `bg-grid` 加在含文字的 `<section>` 上——否则标题会被一起淡出裁掉。
- ink 色阶只定义了 `100`–`950`（见 `style/input.css`）。**没有 `ink-50`**，写 `text-ink-50` 不会生成任何颜色；标题用 `text-ink-100`。
- 主题：默认亮色，`<html class="dark">` 切暗色；防闪烁脚本在 `index.html` `<head>` 最前（读 `localStorage` 的 `leptos-use-color-scheme`）。
- `btn-bolt` 是固定亮黄底 + 深字，两套主题都不变；不要用 `text-bolt-*` 去配它。

---

## 7. 注意事项

- `dist/`、`public/tailwind.css` 都在 `.gitignore`——**勿提交构建产物到工作仓库**。
- 发布只推 `aios-rs/verve` 的 `gh-pages`；**勿向其 `main` 推网站产物**，也勿把产物推到 `lijingrs/verve`。
- 发布后检查工作仓库 `dev` 是否领先 `origin/dev`（`git log --oneline origin/dev..dev`）；部署用临时 clone 不影响工作仓库。
