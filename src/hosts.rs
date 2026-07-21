//! System hosts file utilities (read-only).
//!
//! The `/etc/hosts` file (or platform equivalent) is read and exposed as a
//! list of `(ip, hostname)` entries. Verve does NOT modify the system file in
//! this iteration — users open it in their external editor. Platform hosts
//! file paths:
//!   - macOS / Linux: /etc/hosts
//!   - Windows:       C:\Windows\System32\drivers\etc\hosts

use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostEntry {
    pub ip: String,
    pub host: String,
    pub comment: Option<String>,
}

/// Return the platform hosts file path.
pub fn hosts_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(r"C:\Windows\System32\drivers\etc\hosts")
    }
    #[cfg(not(target_os = "windows"))]
    {
        PathBuf::from("/etc/hosts")
    }
}

/// Read and parse the hosts file. Returns an empty vec on IO / permission error
/// (the UI surfaces the error string separately).
pub fn read_hosts() -> Result<Vec<HostEntry>, String> {
    let raw = read_hosts_string();
    Ok(parse_hosts(&raw))
}

/// Read the raw hosts file content. Returns empty string on error.
pub fn read_hosts_string() -> String {
    let path = hosts_path();
    std::fs::read_to_string(&path).unwrap_or_default()
}

/// Parse hosts file content into entries. Lines starting with `#` are comments.
/// Inline comments (`# ...` after the host) are captured.
pub fn parse_hosts(s: &str) -> Vec<HostEntry> {
    let mut out = Vec::new();
    for raw_line in s.lines() {
        let line = raw_line.trim_end();
        if line.trim_start().starts_with('#') || line.trim().is_empty() {
            continue;
        }
        // Split off inline comment.
        let (body, comment) = match line.find('#') {
            Some(i) => (line[..i].trim(), Some(line[i + 1..].trim().to_string())),
            None => (line.trim(), None),
        };
        let mut parts = body.split_whitespace();
        let Some(ip) = parts.next() else { continue };
        for host in parts {
            out.push(HostEntry {
                ip: ip.to_string(),
                host: host.to_string(),
                comment: comment.clone(),
            });
        }
    }
    out
}

/// Open the hosts file in the system's default editor (macOS: `osascript` to
/// ask for Terminal sudo edit; Linux: `xdg-open` / `$EDITOR`; Windows: notepad).
/// This does NOT block; errors are returned as a string for UI display.
pub fn open_in_editor() -> Result<(), String> {
    let path = hosts_path();
    #[cfg(target_os = "macos")]
    {
        // sudo nano /etc/hosts via Terminal.app — prompts the user for password.
        let script = format!(
            "tell application \"Terminal\" to do script \"sudo nano {}\"",
            path.display()
        );
        std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .spawn()
            .map_err(|e| format!("osascript 启动失败: {e}"))?;
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        let editor = std::env::var("EDITOR").unwrap_or_else(|_| "/usr/bin/editor".into());
        std::process::Command::new(editor)
            .arg(&path)
            .spawn()
            .map_err(|e| format!("打开编辑器失败: {e}"))?;
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("notepad")
            .arg(&path)
            .spawn()
            .map_err(|e| format!("notepad 启动失败: {e}"))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_basic() {
        let text = "\
# comment line
127.0.0.1  localhost
::1        localhost ip6-localhost
10.0.0.1   dev.example.com # dev box
";
        let e = parse_hosts(text);
        assert_eq!(e.len(), 4);
        assert_eq!(e[0].ip, "127.0.0.1");
        assert_eq!(e[0].host, "localhost");
        assert_eq!(e[2].host, "ip6-localhost");
        assert_eq!(e[3].comment.as_deref(), Some("dev box"));
    }

    #[test]
    fn skips_blank_and_comment_only() {
        let text = "# only a comment\n\n";
        assert!(parse_hosts(text).is_empty());
    }
}
