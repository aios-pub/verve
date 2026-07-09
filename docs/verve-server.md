# verve-server — 云端文档管理部署指南

`verve-server` 是 Verve 的独立云端服务，无需桌面 app 即可在线管理 API 文档分享。支持上传项目、创建/删除分享、按链接浏览文档（含严格的有效期 + 密码访问控制）。

## 1. 适用场景

- 团队协作：把 API 文档托管在云端，成员通过链接访问。
- 对外发布：生成公开/密码保护的文档链接分享给前端/客户端开发者。
- 持续交付：CI 流水线上传最新项目 JSON，文档始终与代码同步。

## 2. 快速开始（Docker，推荐）

```bash
# 构建
docker build -t verve-server .

# 运行（端口 3097，数据持久化到 verve-data 卷）
docker run -d --name verve-server \
  -p 3097:3097 \
  -v verve-data:/data \
  verve-server
```

浏览器打开 `http://<服务器IP>:3097/admin` 进入管理后台，即可：
- 📤 **上传项目**：粘贴 Verve 导出的 `Project` JSON（桌面端「项目管理 → 导出 JSON」可生成）
- 🔗 **创建分享**：选择项目、标题、公开/密码
- 🌐 **浏览/删除**分享链接

## 3. 直接运行（二进制）

```bash
# 编译（仅需 Rust 工具链，无 GUI 依赖）
cargo build --release --bin verve-server

# 运行
./target/release/verve-server                      # 默认 0.0.0.0:3097
./target/release/verve-server --port 80            # 自定义端口
./target/release/verve-server --host 127.0.0.1 9000  # 仅本机，端口 9000
./target/release/verve-server --data-dir /var/verve  # 自定义数据目录
```

## 4. 命令行参数

| 参数 | 说明 | 默认值 |
|---|---|---|
| `--host <HOST>` / `-h` | 绑定地址 | `0.0.0.0` |
| `--port <PORT>` / `-p` | 绑定端口 | `3097` |
| `--data-dir <DIR>` / `-d` | 数据根目录（含 `.verve/`） | `$HOME` |
| `--help` | 帮助 | — |
| 裸数字（如 `8080`） | 等同 `--port` | — |

## 5. 数据目录结构

```
<data-dir>/.verve/
├── shares.json                 # 所有分享配置
└── cloud/
    └── projects/
        ├── <project-id>.json   # 每个 Project 一个文件（Verve 原生格式）
        └── ...
```

所有数据均为 JSON 文件，可手动编辑、备份、同步。删除/重建容器时挂载的卷会保留数据。

## 6. HTTP 端点

| 方法 | 路径 | 说明 |
|---|---|---|
| `GET` | `/` | 首页 |
| `GET` | `/admin` | 管理后台（上传项目/管理分享） |
| `POST` | `/admin/api/projects` | 上传项目（body = Project JSON） |
| `POST` | `/admin/api/projects/<id>` | 删除项目 |
| `POST` | `/admin/api/shares` | 创建分享（form-encoded: `project_id`, `title`, `public`, `password`） |
| `POST` | `/admin/api/shares/<id>` | 删除分享 |
| `GET` | `/s/<id>` | 查看分享文档（严格有效期 + 密码） |
| `GET` | `/s/<id>/password` | 密码输入页 |
| `GET` | `/s/<id>/export.html` | 下载自包含 HTML |
| `GET` | `/health` | 健康检查 `{"status":"ok"}` |

## 7. 访问控制（严格强制）

每个 `/s/<id>` 请求都经过三重服务器端校验：

1. **有效期**：超过配置的 `created_at + 有效期` → `410 Gone`
2. **密码**：非公开分享需有效签名 Cookie，否则重定向到密码页（POST 表单校验后下发 HttpOnly Cookie，有效 24h）
3. **范围**：`Request` 范围只暴露目标接口，`Folder` 范围只暴露该文件夹子树

## 8. CI/CD 自动上传

在流水线中用 `curl` 上传项目，文档随构建自动更新：

```bash
# 导出项目 JSON 后
curl -X POST http://verve-server:3097/admin/api/projects \
  -H "Content-Type: application/json" \
  --data-binary @project.json
```

项目按 `id` 去重，重复上传即覆盖。

## 9. 反向代理（Nginx 示例）

```nginx
server {
    listen 80;
    server_name docs.example.com;

    location / {
        proxy_pass http://127.0.0.1:3097;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        # 允许上传较大的项目 JSON
        client_max_body_size 10m;
    }
}
```

配合 Let's Encrypt 即可获得 HTTPS 文档链接。

## 10. 桌面端协作

桌面 app 内建的分享功能（标题栏「分享项目」按钮 / 请求区「分享接口」按钮）默认连接本机 `127.0.0.1:3097`。两者共享同一套 `shares.json` + HTML 渲染逻辑，文档样式完全一致。云端部署时，把桌面端导出的 `Project` JSON 上传到 `/admin` 即可。
