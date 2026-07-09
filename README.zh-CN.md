<div align="center">

# Verve

**离线优先的 API 协同开发平台**

HTTP 调试 · SSH 终端 · 文档分享 · Git 版本控制

一个基于 Rust + GPUI 构建的原生桌面应用

</div>

---

## ✨ 功能特性

### 🔌 API 接口管理
- 项目 → 文件夹 → 接口树，支持多级嵌套
- 完整 HTTP 调试：`GET POST PUT DELETE PATCH HEAD OPTIONS`
- 请求体：none / form-data / x-www-form-urlencoded / raw（JSON/XML/Text/HTML/JS）
- `{{变量}}` 占位符替换，多作用域优先级（接口 > 文件夹 > 环境 > 全局）
- 前置脚本 & 后置测试（通过 `boa_engine` 执行 JavaScript）
- 响应面板：状态码 / 耗时 / 大小 / 响应头 / 响应体（JSON 美化）

### 🔐 SSH 终端管理
- 通过密码 / 私钥 / Agent 认证连接 Linux 服务器
- 多标签终端会话（切换 / 关闭 / 持久保持）
- 内置终端模拟器（alacritty_terminal + GPUI 渲染）
- ANSI 颜色支持（16 色 + 256 色 + 真彩色）
- Tab 键自动补全 · 粘贴支持（Cmd/Ctrl+V）
- 跳板机支持（通过 `channel_open_direct_tcpip` 实现 ProxyJump）
- 自动重连（指数退避策略）
- 主机卡片管理，凭据安全存储（OS 密钥链 + 加密保险库）

### 📄 文档分享
- 从项目 / 文件夹 / 单个接口生成自包含 HTML 文档
- 模块化布局：目录名作为模块标题，字段展示控制
- 带必填标记的彩色参数表格
- 二维码分享 + 链接分享 + HTML 导出
- 严格访问控制：有效期 + 密码保护（服务端强制）
- 本地 HTTP 服务器（`verve-server`），支持云端部署 + 管理 Web UI

### 🌍 Git 版本同步
- 多工作空间支持（每个工作空间 = 一个 git 分支）
- 每 30 分钟自动提交 + 自动同步
- 按工作空间隔离 `workspace.json`
- 用户名 / 密码 / Token 认证 · 分支管理界面

### 🎨 其他
- 国际化：简体中文（默认）/ English
- 22 个内置主题（Catppuccin、Gruvbox、Tokyo Night 等）
- 导入：Postman v2.1 / OpenAPI 3 / Apipost 7+（完全格式兼容）
- 导出：Markdown / JSON / Apipost 格式（双向兼容）
- 可配置首页指向 · 跨平台打包（macOS / Linux / Windows）

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
./scripts/build.sh linux    # → .deb

# Windows
./scripts/build.sh windows  # → .msi
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
| 加密存储 | AES-256-GCM + Argon2 |

---

## 📦 verve-server（云端部署）

Verve 包含独立的服务器二进制，可部署到云端在线托管文档：

```bash
verve-server --port 8080
# 管理后台：http://your-server:8080/admin
```

支持 Docker 部署：
```bash
docker build -t verve-server .
docker run -d -p 3097:3097 -v verve-data:/data verve-server
```

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

## 💰 正式版

Verve 当前为开源预览版，持续迭代中。

正式版将通过**赞赏制**提供：

> **赞赏 99 元起**即可获取正式版授权，包含：
> - 所有正式版功能更新
> - 优先技术支持
> - 新功能优先体验

<table>
  <tr>
    <td align="center">
      <img src="./assets/wechat.jpg" width="200" alt="微信赞赏码" />
      <br />
      <b>微信赞赏</b>
    </td>
  </tr>
</table>

赞赏后请在微信备注你的 GitHub 用户名或邮箱，我会为你发送正式版下载链接。

---

## 📝 更新日志

### v0.1.0（当前预览版）
- ✅ HTTP 接口调试（全方法 / 全 Body 类型 / 变量替换 / 脚本）
- ✅ SSH 终端（多标签 / 跳板机 / 自动重连 / Tab 补全）
- ✅ 文档分享（HTML 生成 / 严格访问控制 / verve-server 云端）
- ✅ Git 版本同步（多工作空间 / 自动提交 / 分支管理）
- ✅ 国际化（中文 / English）
- ✅ 导入导出（Postman / OpenAPI / Apipost 双向兼容）
- ✅ 跨平台打包（macOS / Linux / Windows）

### 计划中
- ⬜ SFTP 文件浏览
- ⬜ 端口转发管理
- ⬜ 自动化测试
- ⬜ 终端配色自定义

---

## 💬 问题反馈

遇到 Bug 或有功能建议？欢迎 [提交 Issue](../../issues/new)。

---

## License

源代码采用 **MIT** 协议开源，可自由使用、修改、分发。

正式版（含预编译二进制 + 持续更新 + 技术支持）通过赞赏制提供。
