<div align="center">

# ⚡ Verve

**基于 Rust + GPUI 构建的原生、GPU 加速研发工作台，在一个窗口内集成接口调试、抓包、服务器排查、压测、Mock、笔记、Hosts 与 JSON 等日常开发工具。**

### 原生研发工作台

Verve 基于 **Rust + [GPUI](https://github.com/zed-industries/zed)**（与 Zed 编辑器同款的 GPU 加速 UI 框架）构建——而非 Electron。整个应用由 GPU 渲染，秒级启动，内存占用不到 100 MB，即使在流式查看容器日志或跑压测时也能保持流畅。

`接口与测试` · `终端与排查` · `Mock 与抓包` · `文档与 Git` · `笔记` · `Hosts` · `JSON`

<br/>

> 🚫 **告别 Postman + iTerm2 + Docker Desktop + Swagger + 笔记软件 + JSON 格式化器 + Hosts 编辑器……的来回切换。**
> Verve 是一个把日常开发高频操作融为一体的原生桌面应用——基于 Rust + GPUI 构建，专为拒绝 Electron 臃肿的开发者而生。

<br/>

<table>
  <tr>
    <td align="center"><b>🦀 Rust + GPUI</b><br/><sub>GPU 渲染，无 Chromium</sub></td>
    <td align="center"><b>⚡ <1 秒启动</b><br/><sub>即开即用</sub></td>
    <td align="center"><b>💾 <100MB 内存</b><br/><sub>轻量 5 倍</sub></td>
    <td align="center"><b>🔒 离线优先</b><br/><sub>数据本地留存</sub></td>
    <td align="center"><b>🖥️ 跨平台</b><br/><sub>macOS · Linux · Windows</sub></td>
  </tr>
</table>

<br/>

[功能特性](#-功能特性) · [社区版 vs Pro 版](#-社区版-vs-pro-版) · [快速开始](#-快速开始) · [Pro 版获取](#-pro-版--赞助获取)

</div>

---

## 🦀 基于 Rust + GPUI 构建 —— 为什么重要

Verve 使用 **Rust** 编写，由 **[GPUI](https://github.com/zed-industries/zed)**（与 Zed 编辑器同款的 GPU 加速框架）渲染。非 Electron、无 Chromium。这是与 Electron 类工具最直观的差距：

|             | Verve（Rust + GPUI） | Electron 类工具  |
| ----------- | ------------- | ------------- |
| ⚡ **启动速度**  | < 1 秒         | 3–5 秒         |
| 💾 **内存占用** | < 100 MB      | 500 MB+       |
| 🎨 **渲染方式** | 原生 GPU，~60fps | Chromium 软件合成 |
| 🛡️ **安全性** | 内存安全，零成本抽象    | GC 卡顿，V8 开销   |

这意味着：调试接口时响应即时，高负载下终端依然流畅，且耗电极低，全天候保持轻量。

---

## 🆍 两个版本

Verve 提供两个版本，使用同一个原生应用、同一套日常接口工作流——Pro 版在社区版基础上叠加服务器排查、测试、知识工具与云端分享。

### 🆍 社区版 —— 免费开源

**社区版**免费，计划采用 **GPL-3.0** 开源许可发布。覆盖个人开发者日常接口工作所需的全部能力：HTTP 调试、抓包、JSON 格式化、Hosts 管理、本地 Mock 服务，以及**本地文档分享**——集成在一个快速、轻量的原生应用中。

### 💎 Pro 版 —— 赞助获取

**Pro 版**通过**赞助**获取（早鸟 **¥99**，原价 **¥199**）。在社区版基础上新增：服务器排查（**SSH / Docker / Kubernetes Pod 观测**）、**压力测试**与**自动化测试套件**、**Markdown 笔记与文件编辑器**、**Git 多工作区同步**，以及**云端文档分享**（推送到自托管 `verve-server` 获取公网链接）。

→ 完整功能对比：[社区版 vs Pro 版](#-社区版-vs-pro-版)
→ 如何获取：[Pro 版获取](#-pro-版--赞助获取)

---

## 🆚 社区版 vs Pro 版

**版本划分原则**：本地、个人能力 → 社区版（开源、免费）；服务器排查、测试、知识工具与云端部署 → Pro 版（赞助获取）。

> ✅ 两版均含 · ❌ 该版不含 · 💎 Pro 版专属

### 总览

| | 🆍 社区版 | 💎 Pro 版 |
|---|---|---|
| **定位** | 个人开发者本地接口工具箱 | 本地工具箱 + 服务器排查 + 测试 + 知识 + 云端 |
| **授权** | GPL-3.0（计划开源） | 专有商业授权（Verve Pro License） |
| **源代码** | 公开可审计 | 不公开 |
| **获取方式** | 免费下载 / 源码构建 | 赞助 ¥99 早鸟 / ¥199 原价 |
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
| **压力测试** —— 自研引擎，并发/时长/QPS，实时延迟图表（p50/p90/p95/p99） | ❌ | 💎 |
| 压测场景模式 —— 多步骤测试用例跨 worker 循环执行 | ❌ | 💎 |
| **自动化测试套件** —— suite/case/step，Request/If/Loop/Wait/Script 步骤，`apt.assert` + JsonPath/Header/Status 提取器 | ❌ | 💎 |

### 🛠️ 开发小工具

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| JSON 格式化器（可折叠树形） | ✅ | ✅ |
| Hosts 管理（读取 `/etc/hosts`、profile 化、环境绑定） | ✅ | ✅ |
| 22 套内置主题（Catppuccin / Gruvbox / Tokyo Night / Solarized 等） | ✅ | ✅ |
| 国际化 i18n（简体中文默认 / English） | ✅ | ✅ |
| 导入（Postman v2.1 / OpenAPI 3 / Postman 7+） | ✅ | ✅ |
| 导出（Markdown / JSON / Postman，双向兼容） | ✅ | ✅ |

### 🖥️ 服务器排查（Pro 专属）

服务器排查功能聚焦于日常排障工作流——查看、读日志、exec 进运行中的目标。它们**不是**完整的集群生命周期管理工具（不支持资源创建、不支持 deployment 管理）。

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| **SSH 终端** —— 密码 / 私钥认证，多标签会话 | ❌ | 💎 |
| 终端模拟 —— 完整 ANSI / xterm-256color（16 色 + 256 色 + 真彩色） | ❌ | 💎 |
| 粘贴（Cmd/Ctrl+V）· 终端文本复制 · 主机卡片管理 | ❌ | 💎 |
| 跳板机（ProxyJump，链式 `direct-tcpip` 隧道） | ❌ | 💎 |
| **SFTP** —— 列表 / 新建目录 / 重命名 / 上传 / 下载、递归 `rm -rf`、传输进度 | ❌ | 💎 |
| **Zmodem** —— 终端内 `rz` / `sz` 文件传输 | ❌ | 💎 |
| **SSH 本地端口转发**（`-L`，一键暴露内网服务） | ❌ | 💎 |
| TOFU `known_hosts` 校验（host-key 不匹配时拒绝连接） | ❌ | 💎 |
| **凭据安全存储** —— OS 钥匙链 + AES-256-GCM / Argon2id 加密保险库 | ❌ | 💎 |
| **Docker** —— 容器列表 / 启动 / 停止 / 重启 / 删除，镜像列表与清理 | ❌ | 💎 |
| Docker —— `docker logs -f` 日志流，`docker exec -it`（真实 PTY，多标签） | ❌ | 💎 |
| Docker —— 远程 daemon（`DOCKER_HOST` 或 **SSH 隧道** `docker system dial-stdio`） | ❌ | 💎 |
| **Kubernetes** —— 解析 `~/.kube/config`，切换上下文，列出 **Pod 与命名空间** | ❌ | 💎 |
| K8s —— `kubectl logs -f`、`kubectl exec -it`（PTY）、`kubectl port-forward` | ❌ | 💎 |
| K8s —— 直连（API Server）或 **SSH 隧道**连接模式 | ❌ | 💎 |

> **按设计未实现**：SSH Agent 认证与远程 `-R` 转发；Docker 镜像构建/拉取/推送、网络/卷/compose/swarm、容器详情与资源占用；Kubernetes 除 pod/namespace 外的资源（无 service/deployment/configmap/…）、不支持 apply/create/delete、不支持 helm/kustomize。

### 📝 文档与知识（分界）

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| **文档分享 —— 本地生成**（项目/文件夹/单接口生成自包含 HTML） | ✅ | ✅ |
| 文档分享 —— 查看（二维码 + 链接 + HTML 导出） | ✅ | ✅ |
| 文档分享 —— 访问控制（有效期 + 密码，本地 server 强制） | ✅ | ✅ |
| 文档分享 —— 字段级显示开关（9 个开关） | ✅ | ✅ |
| **Markdown 笔记** —— 块编辑器、笔记树（文件夹/置顶/标签）、实时预览画布 | ❌ | 💎 |
| **笔记导出 PDF**（内置字体，标题/代码块/列表/链接） | ❌ | 💎 |
| **PDF 查看 / 编辑**（Pdfium 原生，文字/图片/擦除/页面操作） | ❌ | 💎 |
| **独立 Markdown 文件编辑器**（多标签，Finder 双击 / `verve file.md`） | ❌ | 💎 |

### 🌍 云端与同步（Pro 专属）

| 功能 | 社区版 | Pro 版 |
|---|:---:|:---:|
| **Git 多工作区同步**（每工作区 = 一个 git 分支，每 30 分钟自动 commit + sync） | ❌ | 💎 |
| Git —— 用户名/密码/Token 认证、分支管理 UI、按工作空间隔离 `workspace.json` | ❌ | 💎 |
| **自托管 `verve-server`**（独立二进制，绑 `0.0.0.0`，文件后端存储） | ❌ | 💎 |
| **云端文档分享** —— 推送项目到远程 verve-server，获取公网 `/s/<id>` 链接 | ❌ | 💎 |
| `verve-server` `/admin` Web UI —— 上传 / 创建 / 浏览 / 删除分享，多租户 | ❌ | 💎 |

> ℹ️ 文档分享生成的是上传时刻项目的**只读快照**。不支持已分享文档的实时协同编辑或实时同步。

### 📄 授权与使用权利

| | 社区版 | Pro 版 |
|---|---|---|
| **获取方式** | 免费下载 / 源码构建 | 赞助 ¥99 早鸟 / ¥199 原价 |
| **授权类型** | GPL-3.0（计划） | 专有商业授权 |
| **源代码可见** | ✅ 公开 | ❌ 不公开 |
| **个人使用** | ✅ | ✅ |
| **商业使用** | ✅ | ✅（赞助后） |
| **修改 / 二次开发** | ✅（衍生作品须同样采用 GPL-3.0） | ❌ |
| **再分发** | ✅（遵守 GPL-3.0，需公开源码） | ❌ |
| **商用托管 / SaaS** | ✅ | ❌（需联系作者） |
| **逆向工程** | ✅ | ❌ |
| **更新方式** | 社区维护 / 自行编译 | 官方预编译二进制 + 持续更新 |
| **技术支持** | 社区 Issue | 优先技术支持 + 新功能优先体验 |

### 我该选哪个？

- **社区版** —— 你是个人开发者 / 学生，日常就是调试接口、抓包、格式化 JSON、管 Hosts、做本地 Mock、本地分享文档，想要一个快速、开源、免费的原生工具箱。
- **Pro 版** —— 你还需要 SSH 进服务器排障、看 Docker/K8s 日志、跑压测和自动化测试、用 Markdown 记笔记/编辑文件、跨机器同步工作区，或把文档分享到公网云端链接。

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
- 严格访问控制：有效期 + 密码保护（本地 server 强制）
- 字段级显示开关（9 个：描述 / 参数 / 请求头 / Body / 鉴权 / Cookie / 路径 / 示例 / Mock）
- **推送到远程 `verve-server` 获取公网链接属于 Pro 版功能** —— 见[云端与同步](#-云端与同步pro-专属)

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

- 通过**密码 / 私钥**认证连接 Linux 服务器（支持密钥 passphrase）
- 多标签终端会话（切换 / 关闭 / 持久保持）
- 内置终端模拟器，完整 ANSI / xterm-256color（16 色 + 256 色 + 真彩色），支持 IME 输入
- 粘贴（Cmd/Ctrl+V）· 终端文本复制
- **跳板机**（ProxyJump，链式 `direct-tcpip` 隧道）
- TOFU `known_hosts` —— 首次连接记录，再次连接比对，不匹配时拒绝
- 主机卡片管理，**凭据安全存储**（OS 钥匙链 + AES-256-GCM / Argon2id 加密保险库）

> *SSH Agent 认证、远程（`-R`）端口转发暂未实现。*

### 📁 SFTP 与文件传输

<div align="center">
  <img src="./assets/verve_demo/ssh-file.png" width="850" alt="SFTP 文件浏览" />
</div>

- 基于 SSH 的 SFTP —— 列表 / 新建目录 / 重命名 / 上传 / 下载、递归目录删除、64 KiB 分块流式传输带进度

### 📁 Zmodem 传输

- **`rz` / `sz`** 终端内直传文件，通过 `**\x18B` 握手帧自动检测，无需额外配置

### 🔀 SSH 端口转发

- **本地端口转发（`-L`）** —— 一键把内网服务暴露给接口调试器

### 🐳 Docker

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

- 连接**本地或远程** Docker 守护进程（远程通过 `DOCKER_HOST`，或经 **SSH 隧道** `docker system dial-stdio`）
- **容器** —— 列表 / 启动 / 停止 / 重启 / 删除
- **镜像** —— 列表 / 清理未使用
- **日志** —— `docker logs -f --tail=N` 流式输出
- **exec** —— `docker exec -it`，真实 PTY（自动探测 bash/sh/dash/ash），多标签

> *不支持镜像构建/拉取/推送、网络/卷/compose/swarm、容器资源占用——这是排障工具，不是完整的 Docker 管理器。*

### ☸️ Kubernetes

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

- 解析 `~/.kube/config` + Verve 管理的 kubeconfig，切换上下文（Token / ClientCert / BasicAuth）
- **Pod 与命名空间** —— 列表（含 containerStatuses：ready / restartCount），`-A` 或指定命名空间
- **日志** —— `kubectl logs -f --tail=N [-c container]` 流式输出
- **exec** —— `kubectl exec -it`，真实 PTY
- **端口转发** —— `kubectl port-forward pod/<pod> <local>:<remote>`（自动分配本地端口）
- 两种连接模式：**直连**（API Server）或 **SSH 隧道**（经 Verve SSH 跳板机）

> *仅支持 pod 与 namespace——无 service/deployment/configmap/ingress/node，不支持 apply/create/delete，不支持 helm/kustomize。这是 Pod 级观测与调试工具。*

### ⚡ 压力测试

<div align="center">
  <img src="./assets/verve_demo/stress.png" width="850" alt="压力测试" />
</div>

- **自研压测引擎**（基于 `reqwest`，非外部二进制）—— 并发 / 时长 / QPS 上限 / 超时，keepalive 与重定向控制
- **实时图表** —— 每 200ms 一个 `StressSnapshot`（窗口 RPS、p50/p90/p95/p99、延迟直方图、状态码分布）
- **场景模式** —— 多个 worker 各自循环跑一个多步骤 TestCase（复用 autotest runner），按迭代记录延迟与通过/失败计数

### 🧪 自动化测试

<div align="center">
  <img src="./assets/verve_demo/auto_test.png" width="850" alt="自动化测试" />
</div>

- **套件 → 用例 → 步骤** 三层结构（类 Postman Collection Runner），持久化到 `workspace.json`
- **步骤类型**：`Request`（HTTP）· `If`（JS 条件 → then/else）· `Loop`（Repeat / ForEach 遍历 JSON 数组 / While）· `Wait` · `Script`
- **断言** —— 通过 JavaScript（boa engine）的 `apt.assert(condition, message?)`，每步记录通过/失败计数
- **变量提取器** —— JsonPath（点路径）· Header · StatusCode · ResponseTime · Body；提取的变量对后续步骤可见，跨迭代持续
- **脚本 API** —— `apt.variables` / `apt.environment` / `apt.setVariable` / `apt.assert` / `apt.echo` / `console.log`，绑定 `response` 对象

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

### 🌍 Git 同步 & 云端

<div align="center">
  <img src="./assets/verve_demo/git_sync_time.png" width="850" alt="Git 版本同步" />
</div>

- 多工作空间支持（每个工作空间 = 一个 git 分支）
- 每 30 分钟自动提交 + 自动同步
- 按工作空间隔离 `workspace.json`
- 用户名 / 密码 / Token 认证 · 分支管理界面

### 📦 自托管 `verve-server`（云端分享）

Pro 版内置独立的服务端二进制，可在云端托管分享文档，并提供管理后台用于上传项目、创建/管理分享链接。

- 绑 `0.0.0.0`，文件后端存储（`<data>/cloud/`）
- **云端文档分享** —— 推送项目 JSON 到远程 server，获取公网 `/s/<id>` 链接（Bearer token 鉴权上传）
- `/admin` Web UI —— 上传 / 创建 / 浏览 / 删除分享，多租户隔离
- 分享的文档是上传时刻项目的**只读快照**

完整部署指南见 [`docs/verve-server.md`](./docs/verve-server.md)。

---

## 🚀 快速开始

### 🆍 社区版

- **预编译包**：从 [Releases](../../releases) 下载对应平台（macOS / Linux / Windows）的最新社区版安装包。
- **源码构建**：`git clone` 本仓库，然后 `cargo build --release`。
- 首次启动会自动创建数据目录并附带演示项目，可立即上手体验。

### 💎 Pro 版

Pro 版（预编译二进制 + 持续更新 + 优先技术支持）通过赞助获取。请查看下方 [Pro 版获取](#-pro-版--赞助获取) 章节。

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

## 💎 Pro 版 —— 赞助获取

> ⚠️ **限时首发价 —— 即将结束！**

**Pro 版**解锁社区版之外的全部能力——SSH / Docker / Kubernetes Pod 排障、压测与自动化测试、Markdown 笔记与文件编辑器、Git 多工作区同步，以及经自托管 `verve-server` 的云端文档分享。通过 **赞助** 模式获取，以下价格为 **限时早鸟价**。

### 🔥 早鸟特惠 —— 赞助 **99 元**

当前 **99 元** 早鸟价为限时首发优惠，**活动结束后将恢复原价 199 元。** 现在锁定最低价：

> **今日赞助 99 元起** 即可解锁 Pro 版，包含：
>
> - ✅ 全部 Pro 能力（SSH / Docker / K8s Pod 排障 / 压测 / 自动化测试 / Markdown 笔记 / Git 同步 / 云端文档分享）
> - ✅ 所有正式版功能更新（每个未来版本，免费）
> - ✅ 优先技术支持
> - ✅ 新功能优先体验
> - ✅ macOS / Linux / Windows 预编译二进制，零构建烦恼

<table>
  <tr>
    <td align="center">
      <img src="./assets/wechat_official.jpg" width="200" alt="微信公众号（赞助入口）" />
      <br />
      <b>① 赞助 —— 扫码进入公众号文章赞助</b>
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
> 1. **赞助** —— 扫描左侧二维码进入**微信公众号文章页**完成赞助（99 元起）。
> 2. **加我** —— 扫描右侧二维码添加我的**个人微信**为好友。
> 3. 发送赞助截图，我会为你发送最新 Pro 版下载链接与激活指引。

⏰ **别错过 —— 首发期结束后即将涨价。**

---

## 💬 问题反馈

遇到 Bug 或有功能建议？欢迎 [提交 Issue](../../issues/new)。

---

## 📄 License / 授权

Verve 提供两个版本，分别采用不同授权：

- **社区版** —— 计划采用 **GPL-3.0** 开源许可发布。源代码将公开；可在 GPL-3.0 条款下使用、修改、分发（衍生作品须采用兼容的开源协议并公开源码）。
- **Pro 版** —— 专有软件，采用 **Verve Pro License**，**不开放源代码**。未经作者书面许可，禁止以下行为：反向工程 / 反编译 / 反汇编；复制 / 修改 / 二次分发软件或衍生品；用于商业转售或托管服务。Pro 版通过赞助获取。如需商业授权或团队合作方案，请通过上方微信联系作者。

> **说明**：当前仓库 `LICENSE` 文件为过渡期闭源协议；社区版正式开源时将切换为 GPL-3.0。

---

## ⚠️ 免责声明

Verve 是一个持续开发中的项目。**本文档不保证所描述的全部功能均已可用、完整或稳定。** 部分功能可能尚未实现、处于实验阶段，或在不另行通知的情况下发生变更。**具体以试用版的实际能力为准。** 软件的真实状态请以最新的预览版 / Pro 版为准。
