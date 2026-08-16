//! Local PTY backend for the SSH panel's "local terminal" mode.
//!
//! Spawns the user's default shell inside a pseudo-terminal (portable-pty:
//! openpty on Unix, ConPTY on Windows) and exposes the same channel shape the
//! remote SSH sessions use:
//! - `input_tx`  (keystrokes → PTY stdin)
//! - `resize_tx` (terminal size → TIOCSWINSZ / ConPTY resize)
//! - `output_rx` (PTY output → `TerminalView::feed`)
//!
//! The three I/O threads are plain std threads: tokio's unbounded channels
//! work without a runtime (`send` / `blocking_recv`), and the receiver side is
//! awaited on the GPUI executor exactly like the Docker exec sessions.

use std::io::{Read, Write};

use anyhow::{Context, Result};
use portable_pty::{Child, CommandBuilder, MasterPty, PtySize};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::ssh::client::ResizeCmd;

/// Event flowing from the PTY reader thread back to the UI.
pub enum LocalPtyEvent {
    Data(Vec<u8>),
    /// The child exited. On the master side this surfaces either as a clean
    /// EOF (`read == 0`) or as an io error (EIO on macOS) — both are normal
    /// closure, not failures.
    Closed,
}

/// A live local PTY session. Keeping the senders alive keeps the writer and
/// resizer threads running; call [`LocalPtySession::kill`] when the user
/// closes the tab so the shell process itself is terminated.
pub struct LocalPtySession {
    /// The shell program path (e.g. "/bin/zsh") — for tab labels.
    pub shell: String,
    pub input_tx: UnboundedSender<Vec<u8>>,
    pub resize_tx: UnboundedSender<ResizeCmd>,
    child: Box<dyn Child + Send>,
}

impl LocalPtySession {
    /// Terminate the shell process (tab close / panel teardown).
    pub fn kill(&mut self) {
        if let Err(err) = self.child.kill() {
            log::warn!("local pty: kill failed: {err:#}");
        }
    }

    /// Short display name for tab labels: "/bin/zsh" → "zsh",
    /// "C:\...\pwsh.exe" → "pwsh.exe".
    pub fn shell_name(&self) -> String {
        shell_display_name(&self.shell)
    }
}

fn shell_display_name(path: &str) -> String {
    path.rsplit(['/', '\\'])
        .find(|segment| !segment.is_empty())
        .unwrap_or(path)
        .to_string()
}

/// Pick the shell for the local terminal: `$SHELL` when it exists on disk,
/// falling back to the platform default. Windows probes pwsh → powershell →
/// cmd along PATH.
pub fn detect_shell() -> String {
    #[cfg(windows)]
    {
        for name in ["pwsh.exe", "powershell.exe", "cmd.exe"] {
            if let Some(path) = find_in_path(name) {
                return path;
            }
        }
        // cmd.exe is always present; last-resort default.
        "cmd.exe".to_string()
    }

    #[cfg(not(windows))]
    {
        let fallback = if cfg!(target_os = "macos") {
            "/bin/zsh"
        } else {
            "/bin/bash"
        };
        if let Ok(shell) = std::env::var("SHELL") {
            if !shell.is_empty() && std::path::Path::new(&shell).exists() {
                return shell;
            }
            log::warn!("local pty: $SHELL={shell:?} missing on disk, using {fallback}");
        }
        fallback.to_string()
    }
}

#[cfg(windows)]
fn find_in_path(name: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
        .and_then(|full| full.into_os_string().into_string().ok())
}

/// Spawn the user's shell in a fresh pseudo-terminal sized `cols` x `rows`.
/// Returns the session handle (child process + input/resize senders) plus the
/// output receiver the UI consumes; they are split so the async UI loop can
/// own the receiver while the panel stores the session.
pub fn spawn_local_pty(
    cols: u16,
    rows: u16,
) -> Result<(LocalPtySession, UnboundedReceiver<LocalPtyEvent>)> {
    let shell = detect_shell();
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .with_context(|| format!("打开本地伪终端失败({shell})"))?;

    let mut cmd = CommandBuilder::new(&shell);
    cmd.env("TERM", "xterm-256color");
    // GUI-launched apps on macOS don't inherit the shell's LANG, which makes
    // tools like less/grep mis-handle UTF-8. Derive one only if unset.
    if std::env::var_os("LANG").is_none() {
        let lang = sys_locale::get_locale()
            .map(|l| l.replace(['-', '_'], "_"))
            .unwrap_or_else(|| "en_US".to_string());
        cmd.env("LANG", format!("{lang}.UTF-8"));
    }
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    if let Some(home) = home {
        cmd.cwd(home);
    }

    let child = pair
        .slave
        .spawn_command(cmd)
        .with_context(|| format!("启动本地 shell 失败({shell})"))?;
    // Drop our slave handle so the reader observes the child's exit as EOF
    // instead of the pty staying open on our side forever.
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().context("克隆 pty 读取端失败")?;
    let mut writer = pair.master.take_writer().context("获取 pty 写入端失败")?;

    let (input_tx, mut input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (resize_tx, mut resize_rx) = tokio::sync::mpsc::unbounded_channel::<ResizeCmd>();
    let (output_tx, output_rx) = tokio::sync::mpsc::unbounded_channel::<LocalPtyEvent>();

    // Reader: blocking read loop → Data events; child exit → Closed.
    std::thread::Builder::new()
        .name("local-pty-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if output_tx.send(LocalPtyEvent::Data(buf[..n].to_vec())).is_err() {
                            break; // UI side is gone
                        }
                    }
                    Err(err) => {
                        // EIO is how Unix reports "child died" on the master
                        // side — normal closure, not an error.
                        log::debug!("local pty reader 结束: {err}");
                        break;
                    }
                }
            }
            let _ = output_tx.send(LocalPtyEvent::Closed);
        })
        .context("启动 pty 读取线程失败")?;

    // Writer: keystrokes → pty stdin. Exits when every input sender drops.
    std::thread::Builder::new()
        .name("local-pty-writer".into())
        .spawn(move || {
            while let Some(bytes) = input_rx.blocking_recv() {
                if let Err(err) = writer.write_all(&bytes).and_then(|_| writer.flush()) {
                    log::debug!("local pty writer 结束: {err}");
                    break;
                }
            }
        })
        .context("启动 pty 写入线程失败")?;

    // Resizer: owns the master so the pty stays alive for the reader/writer
    // clones above, and applies window-size changes. Exits when every resize
    // sender drops (tab closed), which closes the pty as well.
    std::thread::Builder::new()
        .name("local-pty-resizer".into())
        .spawn(move || {
            while let Some(cmd) = resize_rx.blocking_recv() {
                let size = PtySize {
                    rows: cmd.rows.min(u16::MAX as u32) as u16,
                    cols: cmd.cols.min(u16::MAX as u32) as u16,
                    pixel_width: 0,
                    pixel_height: 0,
                };
                if let Err(err) = pair.master.resize(size) {
                    log::warn!("local pty resize 失败: {err:#}");
                }
            }
        })
        .context("启动 pty resize 线程失败")?;

    Ok((
        LocalPtySession {
            shell,
            input_tx,
            resize_tx,
            child,
        },
        output_rx,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// End-to-end pump check: spawn the shell, run printf, then exit — the
    /// reader must deliver the program output followed by Closed.
    #[test]
    fn spawn_exec_and_exit() {
        let (mut session, mut output_rx) = spawn_local_pty(80, 24).expect("spawn_local_pty");
        let shell = session.shell_name();

        // The marker only appears if printf actually executes — the typed-line
        // echo shows the literal `VT_%s_OK` instead of the expanded result.
        session
            .input_tx
            .send(b"printf 'VT_%s_OK\\r\\n' PASSED\r".to_vec())
            .expect("send input");

        let mut out: Vec<u8> = Vec::new();
        let marker: &[u8] = b"VT_PASSED_OK";
        let mut saw_marker = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline && !saw_marker {
            match output_rx.try_recv() {
                Ok(LocalPtyEvent::Data(d)) => {
                    if out.len() < 64_000 {
                        out.extend_from_slice(&d);
                    }
                    saw_marker = out.windows(marker.len()).any(|w| w == marker);
                }
                Ok(LocalPtyEvent::Closed) => break,
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        assert!(
            saw_marker,
            "marker not found; shell={shell} out={:?}",
            String::from_utf8_lossy(&out)
        );

        session.input_tx.send(b"exit\r".to_vec()).expect("send exit");
        let mut closed = false;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline && !closed {
            match output_rx.try_recv() {
                Ok(LocalPtyEvent::Closed) => closed = true,
                Ok(LocalPtyEvent::Data(_)) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        assert!(closed, "expected Closed event after `exit`");
        session.kill();
    }
}
