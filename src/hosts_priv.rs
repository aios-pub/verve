//! Privilege escalation helpers for writing to the system hosts file.
//!
//! Writing to `/etc/hosts` (or `C:\Windows\System32\drivers\etc\hosts`) requires
//! elevated privileges. We use platform-native mechanisms to prompt the user:
//!   - macOS: `osascript` with `with administrator privileges` (native dialog)
//!   - Linux: `pkexec` (PolicyKit graphical prompt) if available, else fallback
//!   - Windows: PowerShell `Start-Process -Verb RunAs` (UAC prompt)

use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, Result};

use crate::hosts::hosts_path;
use crate::state::persistence::data_dir;

/// Write the given content to the system hosts file via privilege escalation.
/// Returns an error message suitable for UI display on failure.
pub fn write_system_hosts(content: &str) -> Result<()> {
    let dir = data_dir()?;
    let staging = dir.join("hosts.staging");
    fs::write(&staging, content).context("write staging file")?;
    let target = hosts_path();

    #[cfg(target_os = "macos")]
    {
        // Use osascript to copy the staging file with administrator privileges.
        // We need to quote paths carefully for AppleScript.
        let script = format!(
            "do shell script \"cp '{}' '{}' && chmod 644 '{}'\" with administrator privileges",
            staging.display(),
            target.display(),
            target.display()
        );
        let output = std::process::Command::new("osascript")
            .arg("-e")
            .arg(&script)
            .output()
            .context("osascript failed to launch")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("管理员权限被拒绝或失败: {}", stderr.trim());
        }
    }

    #[cfg(target_os = "linux")]
    {
        // Try pkexec first (graphical PolicyKit prompt), fallback to sudo in terminal.
        let output = std::process::Command::new("pkexec")
            .arg("cp")
            .arg(staging.to_str().unwrap())
            .arg(target.to_str().unwrap())
            .output();
        match output {
            Ok(out) if out.status.success() => {
                let _ = std::process::Command::new("pkexec")
                    .arg("chmod")
                    .arg("644")
                    .arg(target.to_str().unwrap())
                    .output();
            }
            _ => {
                // Fallback: launch terminal with sudo.
                let editor = std::env::var("EDITOR").unwrap_or_else(|_| "/usr/bin/editor".into());
                let _ = std::process::Command::new(editor).arg(&target).spawn();
                anyhow::bail!("请在打开的编辑器中保存 hosts 文件");
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        // Use PowerShell Start-Process with RunAs (UAC prompt) to run cmd copy.
        let ps_cmd = format!(
            "Start-Process -FilePath 'cmd.exe' -ArgumentList '/c copy /Y \"{}\" \"{}\"' -Verb RunAs -Wait",
            staging.display().to_string().replace('\\', "/"),
            target.display().to_string().replace('\\', "/")
        );
        let output = std::process::Command::new("powershell")
            .arg("-Command")
            .arg(&ps_cmd)
            .output()
            .context("powershell failed to launch")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("UAC 权限被拒绝或失败: {}", stderr.trim());
        }
    }

    // Clean up staging file.
    let _ = fs::remove_file(&staging);
    Ok(())
}
