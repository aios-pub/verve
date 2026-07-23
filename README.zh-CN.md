<div align="center">

# Verve

**离线优先的 API 协同开发平台**

HTTP 调试 · SSH 终端 · Docker 与 Kubernetes · 压测 · Mock 与抓包 · 文档分享 · Git 同步

一个基于 Rust + GPUI 构建的原生桌面工作台

[功能特性](#-功能特性) · [最新更新](#-最新更新) · [快速开始](#-快速开始) · [正式版](#-正式版--限时优惠)

</div>

---

## ✨ 功能特性

### 🔌 API 接口调试
- 项目 → 文件夹 → 接口树，支持多级嵌套
- 完整 HTTP 方法：`GET POST PUT DELETE PATCH HEAD OPTIONS`
- 请求体：none / form-data / x-www-form-urlencoded / raw（JSON / XML / Text / HTML / JS）
- `{{变量}}` 占位符替换，多作用域优先级（接口 > 文件夹 > 环境 > 全局）
- 前置脚本 & 后置测试（通过 `boa_engine` 执行 JavaScript）
- 响应面板：状态码 / 耗时 / 大小 / 响应头 / 响应体（JSON 美化）
- 接口克隆 · JSON 格式化与校验 · 历史记录隐藏

### 🔐 SSH 终端管理
- 通过密码 / 私钥 / Agent 认证连接 Linux 服务器
- 多标签终端会话（切换 / 关闭 / 持久保持），支持多终端
- 内置终端模拟器（alacritty_terminal + GPUI 渲染）
- ANSI 颜色支持（16 色 + 256 色 + 真彩色）
- Tab 键自动补全 · 粘贴支持（Cmd/Ctrl+V）· 终端文本复制
- 跳板机支持（通过 `channel_open_direct_tcpip` 实现 ProxyJump）
- 自动重连（指数退避策略）
- 主机卡片管理，凭据安全存储（OS 密钥链 + 加密保险库）

### 📁 SFTP 与文件传输
- 基于 SSH 的 SFTP 文件浏览（上传 / 下载，原生文件对话框）
- **Zmodem `rz` / `sz`** 终端内直传文件，无需额外配置

### 🔀 SSH 端口转发
- 本地端口转发（`-L`，基于 `channel_open_direct_tcpip`）
- 一键把内网服务暴露给接口调试器

### 🐳 Docker 管理
- 连接本地或远程 Docker 守护进程（Docker 主机卡片）
- 容器列表 / 启动 / 停止 / 重启 / 删除，镜像列表与清理
- 容器日志流、进入容器执行命令（真实 PTY，多标签 Shell）
- Shell 自动补全 · 容器过滤 · 远程守护进程 SSH 隧道

### ☸️ Kubernetes 管理
- 解析 `~/.kube/config` + Verve 管理的 kubeconfig，切换上下文
- Pod 列表、日志流、进入 Pod/容器执行、端口转发
- 两种连接模式：**直连**（API Server）或 **SSH 隧道**（经跳板机）
- kubectl Shell 自动补全 · 多标签 · Shell 备份脚本

### ⚡ 压力测试
- 内置压测引擎（基于 `oha`）—— 并发 + 时长，**实时图表** 流式输出
- 场景压测：多步骤测试用例循环执行，延迟分位追踪
- 结果指标带标签 · 国际化

### 🧪 自动化测试
- 测试套件 / 用例 / 步骤，顺序执行器
- 复用脚本与 HTTP 基础设施进行断言

### 🎭 Mock 服务
- 基于规则的 Mock 响应，由统一的 share server 提供服务（端口 3097）
- 按方法 + 路径（精确 → 前缀 → 正则）+ 查询参数 + 请求头匹配，按优先级排序

### 🌐 HTTP 抓包代理
- 本地 HTTP 正向代理（`127.0.0.1:<端口>`）
- 请求 + 响应成对记录到环形缓冲区，应用内查看

### 📄 文档分享
- 从项目 / 文件夹 / 单个接口生成自包含 HTML 文档
- 模块化布局：字段展示控制，带必填标记的彩色参数表格
- 二维码分享 + 链接分享 + HTML 导出
- 严格访问控制：有效期 + 密码保护（服务端强制）
- 本地 HTTP 服务器（`verve-server`）+ **推送到远程 `verve-server`** 获取公网链接
- 管理 Web UI：上传 / 创建 / 浏览 / 删除分享

### 🗒️ Markdown 笔记
- 笔记树（文件夹、置顶、标签），**实时预览画布**
- 笔记导出为 **PDF**（内置字体，标题、代码块、列表、链接）
- PDF 查看器（支持缩放）· PDF 编辑器

### 🌍 Git 版本同步
- 多工作空间支持（每个工作空间 = 一个 git 分支）
- 每 30 分钟自动提交 + 自动同步
- 按工作空间隔离 `workspace.json`
- 用户名 / 密码 / Token 认证 · 分支管理界面

### 🛠️ 更多工具
- **Hosts 文件管理** —— 读取 `/etc/hosts`，管理多套配置
- **数据库客户端启动器** —— 自动检测并启动 TablePlus、DBeaver、DataGrip、pgAdmin、RedisInsight、Sequel Ace 等（macOS / Linux / Windows）
- 国际化：简体中文（默认）/ English
- 22 个内置主题（Catppuccin、Gruvbox、Tokyo Night、Solarized、Everforest、Flexoki 等）
- 导入：Postman v2.1 / OpenAPI 3 / Postman 7+（完全格式兼容）
- 导出：Markdown / JSON / Postman 格式（双向兼容）
- 可配置首页指向 · 自动更新检测 · 跨平台打包（macOS / Linux / Windows）

---

## 🆕 最新更新

### v0.2.0（当前版本）
- 🐳 **Docker 管理** —— 容器、镜像、日志、exec、多标签 Shell、远程守护进程隧道
- ☸️ **Kubernetes 管理** —— kubeconfig 上下文、Pod、日志、exec、端口转发、SSH 隧道 API Server
- ⚡ **压力测试** —— 基于 `oha` 的压测引擎，实时图表 + 场景模式
- 🧪 **自动化测试** —— 套件 / 用例 / 步骤执行器，支持断言
- 🎭 **Mock 服务** —— 按优先级匹配规则（精确 / 前缀 / 正则）
- 🌐 **HTTP 抓包代理** —— 抓取本地 HTTP 流量
- 📁 **SFTP 浏览** + 🔀 **SSH 端口转发** + **Zmodem `rz`/`sz`** 文件传输
- 🗒️ **Markdown 笔记** 实时预览 + **PDF 导出 / 查看 / 编辑**
- 🛠️ **Hosts 文件管理** + **数据库客户端启动器**
- 🔗 **远程 `verve-server` 推送**，生成公网文档链接
- 🎨 Docker / K8s 面板优化、可拖拽 Dock、自定义菜单、全新分享图标

### v0.1.0
- ✅ HTTP 接口调试（全方法 / 全 Body 类型 / 变量替换 / 脚本）
- ✅ SSH 终端（多标签 / 跳板机 / 自动重连 / Tab 补全）
- ✅ 文档分享（HTML 生成 / 严格访问控制 / `verve-server` 云端）
- ✅ Git 版本同步（多工作空间 / 自动提交 / 分支管理）
- ✅ 国际化（中文 / English）· 22 主题 · 导入导出 · 跨平台打包

### 计划中
- ⬜ 远程（`-R`）端口转发
- ⬜ HTTPS MITM 抓包（可信 CA）
- ⬜ 终端配色自定义

---

## 🚀 快速开始

### 编译运行
```bash
# 需要 Rust 1.95.0+
cargo run
```

首次启动自动创建 `~/.verve/` 数据目录，含演示项目。

### 打包发布
```bash
# macOS
./scripts/build.sh macos    # → .app + .dmg

# Linux
./scripts/build.sh linux    # → .deb + .AppImage

# Windows
./scripts/build.sh windows  # → .msi

# 当前平台
./scripts/build.sh all
```

---

## 🏗️ 技术栈

| 组件 | 技术 |
|------|------|
| UI 框架 | [GPUI](https://github.com/zed-industries/zed)（Zed 编辑器同款） |
| SSH 客户端 | [russh](https://github.com/Eugeny/russh) 0.60（纯 Rust） |
| 终端模拟 | [alacritty_terminal](https://crates.io/crates/alacritty_terminal) 0.24 |
| Git 引擎 | [gix](https://crates.io/crates/gix) 0.69（gitoxide） |
| 脚本引擎 | [boa_engine](https://crates.io/crates/boa_engine)（JavaScript） |
| 压测引擎 | [oha](https://crates.io/crates/oha) 1.15 |
| PDF 渲染 | [pdfium-render](https://crates.io/crates/pdfium-render) + [printpdf](https://crates.io/crates/printpdf) |
| 加密存储 | AES-256-GCM + Argon2 |

---

## 📦 verve-server（云端部署）

Verve 内置独立的服务器二进制，可在云端托管文档与 Mock 接口：

```bash
verve-server --port 8080
# 管理后台：http://your-server:8080/admin
```

Docker 部署：
```bash
docker build -t verve-server .
docker run -d -p 3097:3097 -v verve-data:/data verve-server
```

完整部署指南见 [`docs/verve-server.md`](./docs/verve-server.md)。

---

## 📋 快捷键

| 快捷键 | 功能 |
|--------|------|
| `Cmd/Ctrl+Enter` | 发送请求 |
| `Cmd/Ctrl+S` | 保存工作空间 |
| `Cmd/Ctrl+N` | 新建接口 |
| `Cmd/Ctrl+V` | 粘贴（终端中） |
| `Tab` | 自动补全（终端中） |

---

## 💰 正式版 —— 限时优惠

> ⚠️ **限时首发价 —— 即将结束！**

Verve 完全开源、开箱即用。**正式版**（预编译二进制 + 持续更新 + 优先支持）通过 **赞赏制** 提供，以下价格为 **限时早鸟价**。

### 🔥 早鸟特惠 —— 仅需 **99 元**
当前 **99 元** 早鸟价为限时首发优惠，**活动结束后将恢复原价 199 元。** 现在锁定最低价：

> **今日赞赏 99 元起** 即可获取正式版授权，包含：
> - ✅ 所有正式版功能更新（每个未来版本，免费）
> - ✅ 优先技术支持
> - ✅ 新功能优先体验
> - ✅ macOS / Linux / Windows 预编译二进制，零构建烦恼

<table>
  <tr>
    <td align="center">
      <img src="./assets/wechat.jpg" width="200" alt="微信赞赏码" />
      <br />
      <b>微信赞赏</b>
    </td>
  </tr>
</table>

⏰ **别错过 —— 首发期结束后即将涨价。** 赞赏后请在微信备注你的 GitHub 用户名或邮箱，我会为你发送正式版下载链接。

---

## 💬 问题反馈

遇到 Bug 或有功能建议？欢迎 [提交 Issue](../../issues/new)。

---

## License

源代码采用 **MIT** 协议开源，可自由使用、修改、分发。

正式版（含预编译二进制 + 持续更新 + 技术支持）通过赞赏制提供。
