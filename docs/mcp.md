# MCP 接入：让外部 AI 管理 Verve 接口

Verve 内置 [MCP（Model Context Protocol）](https://modelcontextprotocol.io/) 服务端。
配置一次后，Claude Desktop、Cursor、Claude Code、VS Code（Copilot Chat）等 AI 客户端
就可以直接**读取和管理你在 Verve 中的项目、目录、接口（HTTP 请求定义）与环境变量**，
例如：

- “看看 Verve 里订单服务有哪些接口，给我生成一份接口清单”
- “把这个 curl 存成 Verve 接口，放到『订单模块』目录下”
- “给所有 `/api/v1` 的接口打上 v1 标签，并把状态改成『已发布』”
- “在测试环境里加一个 `baseUrl` 变量”

服务通过 **stdio** 在本机运行（客户端以子进程方式启动 `verve mcp`），不监听任何网络端口。

---

## 一、一键接入（推荐）

打开 Verve：**项目管理 → 对外能力 → MCP 接入**。

1. 点 **「一键复制配置」**，得到通用 JSON：

   ```json
   {
     "mcpServers": {
       "verve": {
         "command": "/Applications/Verve.app/Contents/MacOS/verve",
         "args": ["mcp"]
       }
     }
   }
   ```

   `command` 已自动填好当前 Verve 可执行文件的绝对路径。

2. 按你使用的客户端粘贴：

   | 客户端 | 接入方式 |
   | --- | --- |
   | **Claude Code** | 点「Claude Code 命令」复制，在终端执行即可（已带 `-s user`，全局生效） |
   | **Claude Desktop** | 点「Claude Desktop」复制 JSON，粘贴到配置文件 `claude_desktop_config.json` |
   | **Cursor** | 点「Cursor」复制 JSON，粘贴到 `~/.cursor/mcp.json` |
   | **VS Code** | 点「VS Code」复制 JSON，粘贴到项目 `.vscode/mcp.json` |
   | 其他 MCP 客户端 | 使用「一键复制配置」的通用 `mcpServers` JSON |

3. 重启 / 刷新客户端，工具列表中出现 `list_projects`、`create_request` 等即接入成功。

> 开发构建（`cargo run`）时 `command` 会指向 `target/debug/verve`，同样可用。
> 也可以设置环境变量 `VERVE_BIN` 覆盖配置里的可执行文件路径。

---

## 二、命令行用法

```
verve mcp [选项]
```

| 选项 | 说明 |
| --- | --- |
| `-c, --print-config <generic\|claude-desktop\|cursor\|vscode>` | 打印对应客户端配置 JSON 后退出 |
| `-d, --data-dir <目录>` | 指定工作区数据目录（默认 `~/.verve`） |
| `-h, --help` | 显示帮助 |

示例：

```bash
# 打印通用配置
verve mcp --print-config generic

# 让 MCP 服务读写指定目录下的工作区
verve mcp --data-dir /path/to/workspace
```

MCP 协议帧走标准输入/输出，**所有日志只写标准错误**，不会污染协议。

---

## 三、提供的工具

### 读取

| 工具 | 作用 |
| --- | --- |
| `list_projects` | 列出所有项目（id、名称、接口/目录/环境数量、当前激活项目） |
| `list_requests` | 列出接口摘要（id、名称、方法、协议、URL、所在目录、标签、状态），支持按目录子树与关键字过滤 |
| `get_request` | 获取单个接口的完整定义（参数、请求头、Cookie、请求体、认证、变量、脚本、示例响应等） |
| `list_folders` | 获取项目的完整目录树 |
| `list_environments` | 列出环境及变量（**包含变量值/密钥**） |

### 管理（写入）

| 工具 | 作用 |
| --- | --- |
| `create_project` | 新建项目 |
| `create_folder` | 新建目录（可指定父目录） |
| `create_request` | 新建接口（方法、URL、参数、请求头、请求体、认证、标签等） |
| `update_request` | 部分更新接口（任一字段缺省即不修改；参数/请求头/请求体/认证给出时整体替换），支持改协议与状态标签 |
| `rename_node` | 重命名接口或目录 |
| `move_node` | 移动接口或目录（省略 `folder_id` 即移到根目录） |
| `delete_node` | 删除接口或目录（目录会连同子内容一起删除） |
| `set_environment_variable` | 新增/更新环境变量（环境不存在时自动创建） |
| `delete_environment_variable` | 删除环境变量 |

所有工具都支持可选的 `project_id`（id 或唯一项目名），省略时操作**当前激活项目**；
接口/目录同样支持用 id 或唯一名称引用，与 Verve 内置 AI 助手的容错规则一致。

---

## 四、数据安全与一致性

- **仅本机、仅 stdio**：MCP 服务不开放任何网络端口；只有能在你电脑上启动该进程的
  AI 客户端可以连接。
- **写保护**：每次写入前自动把 `workspace.json` 备份为 `workspace.json.bak`；
  写入后做 JSON 往返校验，一旦异常自动回滚（与内置 AI 助手同一套保护逻辑）。
- **热重载**：Verve 桌面端运行时会监听 `workspace.json`，AI 通过 MCP 做出的变更
  约 1～2 秒内自动出现在界面上，并弹出「工作区已更新」提示。
  - 若界面上存在**尚未保存的编辑**，为避免覆盖你的输入，本次外部变更会等下次
    界面保存后再处理。
- **作用域**：MCP 操作的是当前激活工作区（对应 `workspaces.json` 中 active 的
  git 分支工作区）。
- **密钥可见性**：`get_request` / `list_environments` 会返回认证配置与环境变量值，
  AI 才能据此实际调用接口；请勿把 MCP 配置添加到不受信任的客户端。

## 五、常见问题

- **客户端提示找不到 `verve`？** 使用「项目管理 → 对外能力 → MCP 接入」里复制的
  绝对路径，不要手填 `verve`。
- **改了没生效？** 确认客户端连接的是同一个数据目录（默认 `~/.verve`，
  可在命令中加 `--data-dir`）；Verve 端切到对应工作区即可看到。
- **想验证服务是否正常？** 终端运行 `verve mcp`，进程会等待 stdio 输入而不报错，
  即说明启动成功（按 Ctrl-C 退出）。
