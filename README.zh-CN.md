<div align="center">

# ⚡ Verve

### 一站式研发工作台

**调试接口、排查问题、查看日志、进容器看状态——一个窗口搞定日常开发的高频操作。** 内置 HTTP 调试、SSH 终端、Docker / K8s 只读查看与排查、压测、Mock、抓包,外加笔记、Hosts、JSON 格式化等开发小工具,以及文档分享与 Git 同步。开发者日常反复要做的事,终于汇聚于一处。

`接口与测试` · `终端与排查` · `Mock 与抓包` · `文档与 Git` · `笔记` · `Hosts` · `JSON` · `PDF` · `数据库工具`

<br/>

> 🚫 **告别 Postman + iTerm2 + Docker Desktop + Swagger + 笔记软件 + JSON 格式化器 + Hosts 编辑器……的来回切换。**
> Verve 是一个把日常开发高频操作融为一体的原生桌面应用——基于 Rust 构建,专为拒绝 Electron 臃肿的开发者而生。

<br/>

<table>
  <tr>
    <td align="center"><b>🦀 Rust 原生</b><br/><sub>非 Electron,无 Chromium</sub></td>
    <td align="center"><b>⚡ <1 秒启动</b><br/><sub>即开即用</sub></td>
    <td align="center"><b>💾 <100MB 内存</b><br/><sub>轻量 5 倍</sub></td>
    <td align="center"><b>🔒 离线优先</b><br/><sub>数据本地留存</sub></td>
    <td align="center"><b>🖥️ 跨平台</b><br/><sub>macOS · Linux · Windows</sub></td>
  </tr>
</table>

<br/>

[功能特性](#-功能特性) · [最新更新](#-最新更新) · [快速开始](#-快速开始) · [正式版](#-正式版--限时优惠)

</div>

---

## 🦀 基于 Rust 构建 —— 为什么重要

Verve 从底层起完全使用 **Rust** 构建。这是与 Electron 类工具最直观的差距:

|             | Verve（Rust）   | Electron 类工具  |
| ----------- | ------------- | ------------- |
| ⚡ **启动速度**  | < 1 秒         | 3–5 秒         |
| 💾 **内存占用** | < 100 MB      | 500 MB+       |
| 🎨 **渲染方式** | 原生 GPU,~60fps | Chromium 软件合成 |
| 🛡️ **安全性** | 内存安全,零成本抽象    | GC 卡顿,V8 开销   |

这意味着:调试接口时响应即时,高负载下终端依然流畅,且耗电极低,全天候保持轻量。

---

## ✨ 功能特性

Verve 覆盖开发者的完整日常工作流——远不止接口调试。四大支柱,一个应用:

| 支柱            | 包含能力                                                                              |
| ------------- | --------------------------------------------------------------------------------- |
| 🧪 **接口与测试**  | HTTP 调试 · 压力测试 · 自动化测试套件 · Mock 服务 · HTTP 抓包代理                                    |
| 🖥️ **终端与排查** | SSH 终端 · SFTP 文件传输 · 端口转发 · Docker 容器查看与日志排查 · Kubernetes Pod 查看与日志排查             |
| 🤝 **协作与文档**  | 文档分享(HTML/二维码/链接) · Git 版本同步 · 云端 `verve-server`                                  |
| 🛠️ **开发小工具** | Markdown 笔记(+ PDF 导出) · Hosts 管理 · JSON 格式化 · PDF 查看/编辑 · 22 主题 · 国际化 |

下方逐一展开 👇

---

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
- 主机卡片管理，凭据安全存储（OS 密钥链 + 加密保险库）

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

### 🎭 Mock 服务

<div align="center">
  <img src="./assets/verve_demo/mock.png" width="850" alt="Mock 服务" />
</div>

- 基于规则的 Mock 响应，由统一的 share server 提供服务（端口 3097）
- 按方法 + 路径（精确 → 前缀 → 正则）+ 查询参数 + 请求头匹配，按优先级排序

### 🌐 HTTP 抓包代理

<div align="center">
  <img src="./assets/verve_demo/http-captrue.png" width="850" alt="HTTP 抓包代理" />
</div>

- 本地 HTTP 正向代理（`127.0.0.1:<端口>`）
- 请求 + 响应成对记录到环形缓冲区，应用内查看

### 📄 文档分享

<div align="center">
  <img src="./assets/verve_demo/doc.png" width="850" alt="文档分享" />
</div>

- 从项目 / 文件夹 / 单个接口生成自包含 HTML 文档
- 模块化布局：字段展示控制，带必填标记的彩色参数表格
- 二维码分享 + 链接分享 + HTML 导出
- 严格访问控制：有效期 + 密码保护（服务端强制）
- 本地 HTTP 服务器（`verve-server`）+ **推送到远程 `verve-server`** 获取公网链接
- 管理 Web UI：上传 / 创建 / 浏览 / 删除分享

### 🗒️ Markdown 笔记

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

### 🌍 Git 版本同步

<div align="center">
  <img src="./assets/verve_demo/git_sync_time.png" width="850" alt="Git 版本同步" />
</div>

- 多工作空间支持（每个工作空间 = 一个 git 分支）
- 每 30 分钟自动提交 + 自动同步
- 按工作空间隔离 `workspace.json`
- 用户名 / 密码 / Token 认证 · 分支管理界面

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

## 🆕 最新更新

### v0.2.0（当前版本）

- 🐳 **Docker 管理** —— 容器、镜像、日志、exec、多标签 Shell、远程守护进程隧道
- ☸️ **Kubernetes 管理** —— kubeconfig 上下文、Pod、日志、exec、端口转发、SSH 隧道 API Server
- ⚡ **压力测试** —— 压测引擎，实时图表 + 场景模式
- 🧪 **自动化测试** —— 套件 / 用例 / 步骤执行器，支持断言
- 🎭 **Mock 服务** —— 按优先级匹配规则（精确 / 前缀 / 正则）
- 🌐 **HTTP 抓包代理** —— 抓取本地 HTTP 流量
- 📁 **SFTP 浏览** + 🔀 **SSH 端口转发** + **Zmodem `rz`/`sz`** 文件传输
- 🗒️ **Markdown 笔记** 实时预览 + **PDF 导出 / 查看 / 编辑**
- 🛠️ **Hosts 文件管理**
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

Verve 为闭源专有软件，提供开箱即用的安装包，无需任何编译工具链。

1. **下载**对应平台（macOS / Linux / Windows）的最新预览版安装包。
2. 首次启动会自动创建数据目录并附带演示项目，可立即上手体验。
3. 想要包含持续更新与优先技术支持的**正式版**？请查看下方[正式版](#-正式版--限时优惠)章节。

---

## 📦 verve-server（云端部署）

Verve 内置独立的服务端，可在云端托管文档与 Mock 接口，并提供管理后台用于上传项目、创建/管理分享链接。

完整部署指南见 [`docs/verve-server.md`](./docs/verve-server.md)。

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

## 💰 正式版 —— 限时优惠

> ⚠️ **限时首发价 —— 即将结束！**

Verve 为**闭源专有软件**，官方提供预览版试用。**正式版**（预编译二进制 + 持续更新 + 优先支持）通过 **赞赏制** 提供，以下价格为 **限时早鸟价**。

### 🔥 早鸟特惠 —— 仅需 **99 元**

当前 **99 元** 早鸟价为限时首发优惠，**活动结束后将恢复原价 199 元。** 现在锁定最低价：

> **今日赞赏 99 元起** 即可获取正式版授权，包含：
> 
> - ✅ 所有正式版功能更新（每个未来版本，免费）
> - ✅ 优先技术支持
> - ✅ 新功能优先体验
> - ✅ macOS / Linux / Windows 预编译二进制，零构建烦恼

<table>
  <tr>
    <td align="center">
      <img src="./assets/wechat_official.jpg" width="200" alt="微信公众号（赞赏入口）" />
      <br />
      <b>① 赞赏 —— 扫码进入公众号文章赞赏</b>
    </td>
    <td align="center">
      <img src="./assets/wechat.jpg" width="200" alt="作者个人微信" />
      <br />
      <b>② 添加我的个人微信</b>
    </td>
  </tr>
</table>

> 📩 **如何获取最新正式版：**
> 
> 1. **赞赏** —— 扫描左侧二维码进入**微信公众号文章页**完成赞赏（99 元起）。
> 2. **加我** —— 扫描右侧二维码添加我的**个人微信**为好友。
> 3. 发送赞赏截图，我会为你发送最新正式版下载链接与激活指引。

⏰ **别错过 —— 首发期结束后即将涨价。**

---

## 💬 问题反馈

遇到 Bug 或有功能建议？欢迎 [提交 Issue](../../issues/new)。

---

## 📄 License / 授权

Verve 为**闭源专有软件（Proprietary Software）**，**不开放源代码**，All Rights Reserved。

未经作者书面许可，禁止以下行为：

- ❌ 对软件进行反向工程、反编译、反汇编
- ❌ 复制、修改、二次分发软件或其衍生品
- ❌ 将软件用于商业转售或托管服务

**允许**：下载官方发布的预览版/正式版用于个人与团队日常开发使用。

正式版（含预编译二进制 + 持续更新 + 技术支持）通过赞赏制获取授权。如需商业授权或团队合作方案，请通过上方微信联系作者。
