<div align="center">

# ⚡ Verve

**基于 Rust 构建的离线优先一站式研发工作台，在一个原生窗口内集成接口调试、SSH 排查、Docker/K8s 日志查看、压测、Mock 与抓包，以及笔记、Hosts、JSON 等日常开发工具。**

### 开源社区版 + 商业 Pro 版

Verve 提供**两个版本**：免费开源的**社区版**，满足个人开发者日常接口工作流；以及一次性买断的 **Pro 版**，在社区版基础上叠加运维、知识管理与云端协作能力。

`接口与测试` · `终端与排查` · `Mock 与抓包` · `文档与 Git` · `笔记` · `Hosts` · `JSON` · `PDF`

<br/>

> 🚫 **告别 Postman + iTerm2 + Docker Desktop + Swagger + 笔记软件 + JSON 格式化器 + Hosts 编辑器……的来回切换。**
> Verve 是一个把日常开发高频操作融为一体的原生桌面应用——基于 Rust 构建，专为拒绝 Electron 臃肿的开发者而生。

<br/>

<table>
  <tr>
    <td align="center"><b>🦀 Rust 原生</b><br/><sub>非 Electron，无 Chromium</sub></td>
    <td align="center"><b>⚡ <1 秒启动</b><br/><sub>即开即用</sub></td>
    <td align="center"><b>💾 <100MB 内存</b><br/><sub>轻量 5 倍</sub></td>
    <td align="center"><b>🔒 离线优先</b><br/><sub>数据本地留存</sub></td>
    <td align="center"><b>🖥️ 跨平台</b><br/><sub>macOS · Linux · Windows</sub></td>
  </tr>
</table>

<br/>

[功能特性](#-功能特性) · [社区版 vs Pro 版](#-社区版-vs-pro-版) · [快速开始](#-快速开始) · [Pro 版购买](#-pro-版--一次性买断)

</div>

---

## 🦀 基于 Rust 构建 —— 为什么重要

Verve 从底层起完全使用 **Rust** 构建。这是与 Electron 类工具最直观的差距：

|             | Verve（Rust）   | Electron 类工具  |
| ----------- | ------------- | ------------- |
| ⚡ **启动速度**  | < 1 秒         | 3–5 秒         |
| 💾 **内存占用** | < 100 MB      | 500 MB+       |
| 🎨 **渲染方式** | 原生 GPU，~60fps | Chromium 软件合成 |
| 🛡️ **安全性** | 内存安全，零成本抽象    | GC 卡顿，V8 开销   |

这意味着：调试接口时响应即时，高负载下终端依然流畅，且耗电极低，全天候保持轻量。

---

## 🆍 两个版本

### 🆍 社区版 —— 免费开源

**社区版**免费，计划采用 **Apache-2.0** 开源许可发布。覆盖个人开发者日常接口工作所需的全部能力：HTTP 调试、抓包、JSON 格式化、Hosts 管理、本地 Mock 服务、本地文档分享——集成在一个快速、轻量的原生应用中。

### 💎 Pro 版 —— 一次性买断

**Pro 版**是在社区版基础上的付费升级（早鸟 **¥99**，原价 **¥199**，一次性买断），新增全部进阶能力：完整的 **SSH / Docker / Kubernetes** 运维、**压力测试**、**自动化测试套件**、**Markdown 笔记与文件编辑器**、**Git 多工作区同步**，以及**云端协作**（自托管 `verve-server`、云端 Mock、云端文档分享）。

→ 完整功能对比：[社区版 vs Pro 版](#-社区版-vs-pro-版)
→ 购买：[Pro 版购买](#-pro-版--一次性买断)

---

## 🆚 社区版 vs Pro 版

**版本划分原则**：本地能力 → 社区版（开源、免费）；云端部署与专业运维能力 → Pro 版（商业、一次性买断）。

> ✅ 两版均含 · ❌ 该版不含 · 💎 Pro 版专属

### 总览

| | 🆍 社区版 | 💎 Pro 版 |
|---|---|---|
| **定位** | 个人开发者本地接口工具箱 | 一站式运维 + 知识管理 + 云端协作平台 |
| **授权** | Apache-2.0（计划开源） | 专有商业授权（Verve Pro License） |
| **源代码** | 公开可审计 | 不公开 |
| **价格** | 免费 | ¥99 早鸟 / ¥199 原价（一次性买断） |
| **目标用户** | 个人开发者、学生、开源社区 | 专业开发者、运维工程师、小团队 |
| **跨平台** | macOS · Linux · Windows | macOS · Linux · Windows |

### 🧪 接口与测试

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| HTTP API 调试（GET/POST/PUT/DELETE/PATCH/HEAD/OPTIONS 全方法） | ✅ | ✅ |
| 请求体类型（none / form-data / x-www-form-urlencoded / raw JSON·XML·Text·HTML·JS） | ✅ | ✅ |
| `{{变量}}` 占位符（request > folder > env > global 多作用域优先级） | ✅ | ✅ |
| 前置脚本 & 后置测试（JavaScript，boa_engine） | ✅ | ✅ |
| 多协议客户端（gRPC / TCP / SSE / WebSocket） | ✅ | ✅ |
| 响应面板（状态码 / 耗时 / 大小 / 响应头 / 响应体，JSON 美化） | ✅ | ✅ |
| 项目树（文件夹、多级嵌套、拖拽排序） | ✅ | ✅ |
| 请求/响应历史 | ✅ | ✅ |
| 本地 Mock 服务（精确 → 前缀 → 正则匹配，按优先级） | ✅ | ✅ |
| **HTTP 抓包代理**（`127.0.0.1:<port>`，环形缓冲） | ✅ | ✅ |
| **压力测试**（压测引擎 + 并发/时长 + 实时图表） | ❌ | 💎 |
| 压测场景模式（多步骤循环，延迟分位追踪） | ❌ | 💎 |
| **自动化测试套件**（suite/case/step + if/loop/wait/script + 断言） | ❌ | 💎 |

### 🛠️ 开发小工具

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| JSON 格式化器（可折叠树形） | ✅ | ✅ |
| Hosts 管理（读取 `/etc/hosts`、profile 化、环境绑定） | ✅ | ✅ |
| 22 套内置主题（Catppuccin / Gruvbox / Tokyo Night / Solarized 等） | ✅ | ✅ |
| 国际化 i18n（简体中文默认 / English） | ✅ | ✅ |
| 导入（Postman v2.1 / OpenAPI 3 / Postman 7+） | ✅ | ✅ |
| 导出（Markdown / JSON / Postman，双向兼容） | ✅ | ✅ |

### 🖥️ 终端与排查（Pro 专属）

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| **SSH 终端**（密码 / 私钥 / Agent 认证，多标签） | ❌ | 💎 |
| 终端模拟（完整 ANSI：16 色 + 256 色 + 真彩色） | ❌ | 💎 |
| Tab 补全 · 粘贴 · 文本复制 | ❌ | 💎 |
| 跳板机（ProxyJump） | ❌ | 💎 |
| 自动重连（指数退避） | ❌ | 💎 |
| **SFTP 文件传输**（上传 / 下载，原生对话框） | ❌ | 💎 |
| **Zmodem**（终端内 `rz` / `sz` 直传） | ❌ | 💎 |
| **SSH 端口转发**（`-L`，一键暴露内网服务） | ❌ | 💎 |
| **凭据安全存储**（OS 钥匙链 + AES-256-GCM 加密保险库） | ❌ | 💎 |
| **Docker 管理**（本地/远程 daemon，启停/重启/删除） | ❌ | 💎 |
| Docker 镜像（列表 / 清理） | ❌ | 💎 |
| Docker 日志流 | ❌ | 💎 |
| Docker exec（真实 PTY，多标签 shell） | ❌ | 💎 |
| Docker SSH 隧道（远程守护进程） | ❌ | 💎 |
| **Kubernetes 管理**（kubeconfig + 上下文切换） | ❌ | 💎 |
| K8s Pod（列表 / 日志 / exec / 端口转发） | ❌ | 💎 |
| K8s 连接模式（直连 API Server 或经跳板机 SSH 隧道） | ❌ | 💎 |

### 📝 文档与知识（分界）

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| 文档分享 —— 本地生成（自包含 HTML） | ✅ | ✅ |
| 文档分享 —— 查看（二维码 + 链接 + HTML 导出） | ✅ | ✅ |
| 文档分享 —— 访问控制（有效期 + 密码，本地 server 强制） | ✅ | ✅ |
| **Markdown 笔记**（块编辑器、笔记树、实时预览画布） | ❌ | 💎 |
| **笔记导出 PDF**（内置字体，标题/代码块/列表/链接） | ❌ | 💎 |
| **PDF 查看 / 编辑**（Pdfium 原生，文字/图片/擦除/页面操作） | ❌ | 💎 |
| **独立 Markdown 文件编辑器**（多标签，Finder 双击 / `verve file.md`） | ❌ | 💎 |

### 🌍 云端与协作（Pro 专属）

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| **Git 多工作区同步**（每工作区 = 一个 git 分支，每 30 分钟自动 commit + sync） | ❌ | 💎 |
| Git 分支管理（用户名/密码/Token 认证、分支管理 UI） | ❌ | 💎 |
| Git 工作区隔离（按工作空间隔离 `workspace.json`） | ❌ | 💎 |
| **自托管 `verve-server`**（独立服务端二进制，云端托管文档与 Mock） | ❌ | 💎 |
| **云端文档分享**（推送到远程 verve-server，获取公网链接） | ❌ | 💎 |
| **云端 Mock**（云端托管 Mock 规则，公网可访问） | ❌ | 💎 |
| 管理 Web UI（上传 / 创建 / 浏览 / 删除分享） | ❌ | 💎 |

### 📄 授权与使用权利

| | 社区版 | Pro 版 |
|---|---|---|
| **价格** | 免费 | ¥99 早鸟 / ¥199 原价（一次性买断） |
| **授权类型** | Apache-2.0（计划） | 专有商业授权 |
| **源代码可见** | ✅ 公开 | ❌ 不公开 |
| **个人使用** | ✅ | ✅ |
| **商业使用** | ✅ | ✅（购买授权后） |
| **修改 / 二次开发** | ✅ | ❌ |
| **再分发** | ✅（遵守 Apache-2.0） | ❌ |
| **商用托管 / SaaS** | ✅ | ❌（需联系作者） |
| **逆向工程** | ✅ | ❌ |
| **更新方式** | 社区维护 / 自行编译 | 官方预编译二进制 + 持续更新 |
| **技术支持** | 社区 Issue | 优先技术支持 + 新功能优先体验 |

### 我该选哪个？

- **社区版** —— 你是个人开发者 / 学生，日常就是调试接口、抓包、格式化 JSON、管 Hosts、做本地 Mock，想要一个快速、开源、免费的本地工具箱。
- **Pro 版** —— 你需要 SSH 进服务器排障、看 Docker/K8s 日志、跑压测和自动化测试、用 Markdown 记笔记/编辑文件，或想把文档和 Mock 托管到云端供团队访问。

---

## ✨ 功能特性

### 🆍 社区版功能

### 🔌 API 接口调试

<div align="center">
  <img src="./assets/verve_demo/api.png" width="850" alt="API 接口调试" />
  <br/>
  <em>请求编辑器与响应面板</em>
</div>

<div align="center">
  <img src="./assets/verve_demo/project.png" width="850" alt="项目树" />
  <br/>
  <em>项目树（多级嵌套）</em>
</div>

<div align="center">
  <img src="./assets/verve_demo/drag_order.png" width="850" alt="拖拽排序" />
  <br/>
  <em>拖拽调整树结构顺序</em>
</div>

- 项目 → 文件夹 → 接口树，支持多级嵌套
- 完整 HTTP 方法：`GET POST PUT DELETE PATCH HEAD OPTIONS`
- 请求体：none / form-data / x-www-form-urlencoded / raw（JSON / XML / Text / HTML / JS）
- `{{变量}}` 占位符替换，多作用域优先级（接口 > 文件夹 > 环境 > 全局）
- 前置脚本 & 后置测试（JavaScript）
- 响应面板：状态码 / 耗时 / 大小 / 响应头 / 响应体（JSON 美化）
- 接口克隆 · JSON 格式化与校验 · 历史记录隐藏

### 🌐 HTTP 抓包代理

<div align="center">
  <img src="./assets/verve_demo/http-captrue.png" width="850" alt="HTTP 抓包代理" />
</div>

- 本地 HTTP 正向代理（`127.0.0.1:<端口>`）
- 请求 + 响应成对记录到环形缓冲区，应用内查看

### 🎭 本地 Mock 服务

<div align="center">
  <img src="./assets/verve_demo/mock.png" width="850" alt="Mock 服务" />
</div>

- 基于规则的 Mock 响应，由统一的 share server 提供服务（端口 3097）
- 按方法 + 路径（精确 → 前缀 → 正则）+ 查询参数 + 请求头匹配，按优先级排序

### 📄 文档分享（本地）

<div align="center">
  <img src="./assets/verve_demo/doc.png" width="850" alt="文档分享" />
</div>

- 从项目 / 文件夹 / 单个接口生成自包含 HTML 文档
- 模块化布局：字段展示控制，带必填标记的彩色参数表格
- 二维码分享 + 链接分享 + HTML 导出
- 严格访问控制：有效期 + 密码保护（服务端强制）
- **推送到远程 `verve-server` 获取公网链接属于 Pro 版功能** —— 见[云端与协作](#-云端与协作pro-专属)

### 🛠️ 更多工具

<div align="center">
  <img src="./assets/verve_demo/hosts.png" width="420" alt="Hosts 管理" />
  <img src="./assets/verve_demo/json-format.png" width="420" alt="JSON 格式化" />
  <br/>
  <em>Hosts 管理  ·  JSON 格式化</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/theme.png" width="850" alt="主题" />
  <br/>
  <em>22 个内置主题</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/settings.png" width="850" alt="设置" />
  <br/>
  <em>设置（国际化、主题、首页指向、自动更新）</em>
</div>

- **Hosts 文件管理** —— 读取 `/etc/hosts`，管理多套配置
- 国际化：简体中文（默认）/ English
- 22 个内置主题（Catppuccin、Gruvbox、Tokyo Night、Solarized、Everforest、Flexoki 等）
- 导入：Postman v2.1 / OpenAPI 3 / Postman 7+（完全格式兼容）
- 导出：Markdown / JSON / Postman 格式（双向兼容）
- 可配置首页指向 · 自动更新检测 · 跨平台打包（macOS / Linux / Windows）

---

### 💎 Pro 版功能

### 🔐 SSH 终端管理

<div align="center">
  <img src="./assets/verve_demo/ssh.png" width="850" alt="SSH 主机卡片" />
  <br/>
  <em>主机卡片管理（凭据安全存储）</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/ssh-terminal.png" width="850" alt="SSH 终端" />
  <br/>
  <em>多标签终端会话</em>
</div>

- 通过密码 / 私钥 / Agent 认证连接 Linux 服务器
- 多标签终端会话（切换 / 关闭 / 持久保持），支持多终端
- 内置终端模拟器，完整 ANSI 颜色支持（16 色 + 256 色 + 真彩色）
- Tab 键自动补全 · 粘贴支持（Cmd/Ctrl+V）· 终端文本复制
- 跳板机支持（ProxyJump）
- 自动重连（指数退避策略）
- 主机卡片管理，凭据安全存储（OS 钥匙链 + 加密保险库）

### 📁 SFTP 与文件传输

<div align="center">
  <img src="./assets/verve_demo/ssh-file.png" width="850" alt="SFTP 文件浏览" />
</div>

- 基于 SSH 的 SFTP 文件浏览（上传 / 下载，原生文件对话框）
- **Zmodem `rz` / `sz`** 终端内直传文件，无需额外配置

### 🔀 SSH 端口转发

- 本地端口转发（`-L`）
- 一键把内网服务暴露给接口调试器

### 🐳 Docker 管理

<div align="center">
  <img src="./assets/verve_demo/docker.png" width="850" alt="Docker 管理" />
  <br/>
  <em>容器列表</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/docker-images.png" width="420" alt="Docker 镜像" />
  <img src="./assets/verve_demo/docker-log.png" width="420" alt="Docker 日志" />
  <br/>
  <em>镜像  ·  日志</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/docker-shell.png" width="850" alt="Docker Exec Shell" />
  <br/>
  <em>进入容器执行（多标签）</em>
</div>

- 连接本地或远程 Docker 守护进程（Docker 主机卡片）
- 容器列表 / 启动 / 停止 / 重启 / 删除，镜像列表与清理
- 容器日志流、进入容器执行命令（真实 PTY，多标签 Shell）
- Shell 自动补全 · 容器过滤 · 远程守护进程 SSH 隧道

### ☸️ Kubernetes 管理

<div align="center">
  <img src="./assets/verve_demo/k8s.png" width="850" alt="Kubernetes 管理" />
  <br/>
  <em>Pod 列表</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/k8s-log.png" width="420" alt="K8s 日志" />
  <img src="./assets/verve_demo/k8s-shell.png" width="420" alt="K8s Exec Shell" />
  <br/>
  <em>日志  ·  进入容器执行</em>
</div>

- 解析 `~/.kube/config` + Verve 管理的 kubeconfig，切换上下文
- Pod 列表、日志流、进入 Pod/容器执行、端口转发
- 两种连接模式：**直连**（API Server）或 **SSH 隧道**（经跳板机）
- kubectl Shell 自动补全 · 多标签 · Shell 备份脚本

### ⚡ 压力测试

<div align="center">
  <img src="./assets/verve_demo/stress.png" width="850" alt="压力测试" />
</div>

- 内置压测引擎 —— 并发 + 时长，**实时图表** 流式输出
- 场景压测：多步骤测试用例循环执行，延迟分位追踪
- 结果指标带标签 · 国际化

### 🧪 自动化测试

<div align="center">
  <img src="./assets/verve_demo/auto_test.png" width="850" alt="自动化测试" />
</div>

- 测试套件 / 用例 / 步骤，顺序执行器
- 复用脚本与 HTTP 基础设施进行断言

### 🗒️ Markdown 笔记 & PDF 编辑器

<div align="center">
  <img src="./assets/verve_demo/note.png" width="850" alt="Markdown 笔记" />
  <br/>
  <em>笔记 + 实时预览</em>
</div>
<div align="center">
  <img src="./assets/verve_demo/pdf.png" width="850" alt="PDF 查看 / 导出" />
  <br/>
  <em>PDF 查看与导出</em>
</div>

- 笔记树（文件夹、置顶、标签），**实时预览画布**
- 笔记导出为 **PDF**（内置字体，标题、代码块、列表、链接）
- PDF 查看器（支持缩放）· PDF 编辑器

### 🌍 Git 版本同步 & 云端

<div align="center">
  <img src="./assets/verve_demo/git_sync_time.png" width="850" alt="Git 版本同步" />
</div>

- 多工作空间支持（每个工作空间 = 一个 git 分支）
- 每 30 分钟自动提交 + 自动同步
- 按工作空间隔离 `workspace.json`
- 用户名 / 密码 / Token 认证 · 分支管理界面

### 📦 自托管 `verve-server`（云端部署）

Pro 版内置独立的服务端，可在云端托管文档、Mock 接口与云端分享，并提供管理后台用于上传项目、创建/管理分享链接。

完整部署指南见 [`docs/verve-server.md`](./docs/verve-server.md)。

---

## 🚀 快速开始

### 🆍 社区版

- **预编译包**：从 [Releases](../../releases) 下载对应平台（macOS / Linux / Windows）的最新社区版安装包。
- **源码构建**：`git clone` 本仓库，然后 `cargo build --release`。
- 首次启动会自动创建数据目录并附带演示项目，可立即上手体验。

### 💎 Pro 版

Pro 版（预编译二进制 + 持续更新 + 优先技术支持）通过一次性买断获取。请查看下方 [Pro 版购买](#-pro-版--一次性买断) 章节。

---

## 📋 快捷键

| 快捷键              | 功能        |
| ---------------- | --------- |
| `Cmd/Ctrl+Enter` | 发送请求      |
| `Cmd/Ctrl+S`     | 保存工作空间    |
| `Cmd/Ctrl+N`     | 新建接口      |
| `Cmd/Ctrl+V`     | 粘贴（终端中）   |
| `Tab`            | 自动补全（终端中） |

---

## 💎 Pro 版 —— 一次性买断

> ⚠️ **限时首发价 —— 即将结束！**

**Pro 版**解锁社区版之外的全部能力——SSH / Docker / Kubernetes 运维、压测与自动化测试、Markdown 笔记与文件编辑器、Git 多工作区同步，以及自托管云端协作（`verve-server`、云端 Mock、云端文档分享）。通过 **一次性买断** 提供，以下价格为 **限时早鸟价**。

### 🔥 早鸟特惠 —— 仅需 **99 元**

当前 **99 元** 早鸟价为限时首发优惠，**活动结束后将恢复原价 199 元。** 现在锁定最低价：

> **今日支付 99 元起**（一次性买断）即可解锁 Pro 版，包含：
>
> - ✅ 全部 Pro 能力（SSH / Docker / K8s / 压测 / 自动化测试 / Markdown 笔记 / Git 同步 / 云端协作）
> - ✅ 所有正式版功能更新（每个未来版本，免费）
> - ✅ 优先技术支持
> - ✅ 新功能优先体验
> - ✅ macOS / Linux / Windows 预编译二进制，零构建烦恼

<table>
  <tr>
    <td align="center">
      <img src="./assets/wechat_official.jpg" width="200" alt="微信公众号（支付入口）" />
      <br />
      <b>① 支付 —— 扫码进入公众号文章支付</b>
    </td>
    <td align="center">
      <img src="./assets/wechat.jpg" width="200" alt="作者个人微信" />
      <br />
      <b>② 添加我的个人微信</b>
    </td>
  </tr>
</table>

> 📩 **如何获取 Pro 版：**
>
> 1. **支付** —— 扫描左侧二维码进入**微信公众号文章页**完成支付（99 元起）。
> 2. **加我** —— 扫描右侧二维码添加我的**个人微信**为好友。
> 3. 发送支付截图，我会为你发送最新 Pro 版下载链接与激活指引。

⏰ **别错过 —— 首发期结束后即将涨价。**

---

## 💬 问题反馈

遇到 Bug 或有功能建议？欢迎 [提交 Issue](../../issues/new)。

---

## 📄 License / 授权

Verve 提供两个版本，分别采用不同授权：

- **社区版** —— 计划采用 **Apache-2.0** 开源许可发布。源代码将公开；允许个人与商业使用、修改、分发（遵守 Apache-2.0 条款）。
- **Pro 版** —— 专有软件，采用 **Verve Pro License**，**不开放源代码**。未经作者书面许可，禁止以下行为：反向工程 / 反编译 / 反汇编；复制 / 修改 / 二次分发软件或衍生品；用于商业转售或托管服务。Pro 版通过一次性买断获取授权。如需商业授权或团队合作方案，请通过上方微信联系作者。

> **说明**：当前仓库 `LICENSE` 文件为过渡期闭源协议；社区版正式开源时将切换为 Apache-2.0。
