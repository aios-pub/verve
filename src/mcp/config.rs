//! MCP client configuration snippets for the one-click copy button.
//!
//! All snippets launch this binary as a stdio MCP server (`verve mcp`).
//! `VERVE_BIN` overrides the command path (useful for dev builds / portable
//! installs where `current_exe` points inside an app bundle).

use serde_json::{Value, json};

/// Which MCP host the snippet is generated for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClientKind {
    /// Generic `mcpServers` JSON (Claude Desktop, Cursor, most clients).
    Generic,
    /// Claude Desktop desktop app config file.
    ClaudeDesktop,
    /// Cursor IDE `~/.cursor/mcp.json`.
    Cursor,
    /// VS Code / Copilot `.vscode/mcp.json`.
    VsCode,
}

impl ClientKind {
    /// Parse from a CLI/UI string identifier.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "generic" | "general" | "通用" => Some(Self::Generic),
            "claude-desktop" | "claudedesktop" | "claude" => Some(Self::ClaudeDesktop),
            "cursor" => Some(Self::Cursor),
            "vscode" | "vs-code" | "vs_code" => Some(Self::VsCode),
            _ => None,
        }
    }

    /// Human label used on the copy buttons.
    pub fn label(self) -> &'static str {
        match self {
            Self::Generic => "通用 JSON",
            Self::ClaudeDesktop => "Claude Desktop",
            Self::Cursor => "Cursor",
            Self::VsCode => "VS Code",
        }
    }

    /// Where the snippet should be pasted (None = command line / docs).
    pub fn config_path_hint(self) -> Option<String> {
        let home = std::env::var_os("HOME").map(|h| h.to_string_lossy().to_string());
        #[cfg(windows)]
        let appdata = std::env::var("APPDATA").ok();
        match self {
            Self::ClaudeDesktop => {
                #[cfg(target_os = "macos")]
                {
                    home.map(|h| format!("{h}/Library/Application Support/Claude/claude_desktop_config.json"))
                }
                #[cfg(windows)]
                {
                    appdata.map(|a| format!("{a}\\Claude\\claude_desktop_config.json"))
                }
                #[cfg(all(not(target_os = "macos"), not(windows)))]
                {
                    home.map(|h| format!("{h}/.config/Claude/claude_desktop_config.json"))
                }
            }
            Self::Cursor => home.map(|h| format!("{h}/.cursor/mcp.json")),
            Self::VsCode => Some(".vscode/mcp.json（项目级）或用户设置".to_string()),
            Self::Generic => None,
        }
    }
}

/// The `(command, args)` pair clients use to launch the server.
pub fn command_pair() -> (String, Vec<String>) {
    let command = std::env::var("VERVE_BIN")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| {
            std::env::current_exe()
                .ok()
                .map(|p| p.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "verve".to_string());
    (command, vec!["mcp".to_string()])
}

fn server_entry() -> Value {
    let (command, args) = command_pair();
    json!({ "command": command, "args": args })
}

/// Configuration snippet for JSON-file based clients.
pub fn client_config(kind: ClientKind) -> String {
    let value = match kind {
        ClientKind::VsCode => {
            json!({
                "servers": {
                    "verve": {
                        "type": "stdio",
                        "command": server_entry()["command"].clone(),
                        "args": server_entry()["args"].clone(),
                    }
                }
            })
        }
        _ => json!({ "mcpServers": { "verve": server_entry() } }),
    };
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| String::from("{}"))
}

/// `claude mcp add` shell command for the Claude Code CLI.
pub fn claude_code_command(scope_user: bool) -> String {
    let (command, args) = command_pair();
    let scope = if scope_user { " -s user" } else { "" };
    // Quote the command path only when it contains spaces (app bundles).
    let command = if command.contains(' ') {
        format!("\"{command}\"")
    } else {
        command
    };
    let args = args.join(" ");
    format!("claude mcp add verve{scope} -- {command} {args}")
}
