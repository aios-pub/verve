//! Local PTY backend for the SSH panel's "local terminal" mode.
//!
//! Spawns the user's default shell inside a pseudo-terminal and exposes the
//! same channel shape the remote SSH sessions use:
//! - `input_tx`  (keystrokes → PTY stdin)
//! - `resize_tx` (terminal size → TIOCSWINSZ / ConPTY resize)
//! - `output_rx` (PTY output → `TerminalView::feed`)
//!
//! Backend selection:
//! - Unix: portable-pty (openpty).
//! - Windows: our own ConPTY implementation (`super::win_conpty`) —
//!   portable-pty 0.9's spawn shape leaves shells silent on current Win11
//!   builds (see win_conpty.rs for details).
//!
//! The three I/O threads are plain std threads: tokio's unbounded channels
//! work without a runtime (`send` / `blocking_recv`), and the receiver side is
//! awaited on the GPUI executor exactly like the Docker exec sessions.

use std::io::{Read, Write};
use std::time::Duration;

use anyhow::{Context, Result};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::ssh::client::ResizeCmd;

/// How long the reader-thread watchdog waits for the first byte before
/// reporting a silent shell via [`LocalPtyEvent::Stall`].
const STALL_TIMEOUT: Duration = Duration::from_secs(5);
/// Poll granularity of the stall watchdog.
const STALL_POLL: Duration = Duration::from_millis(100);

/// Event flowing from the PTY reader thread back to the UI.
pub enum LocalPtyEvent {
    Data(Vec<u8>),
    /// The shell is alive but produced no output within [`STALL_TIMEOUT`] of
    /// spawn. Surfaces a UI hint instead of leaving the tab silently blank
    /// (hung profile, broken std handles, …). Sent at most once.
    Stall,
    /// The child exited. On the master side this surfaces either as a clean
    /// EOF (`read == 0`) or as an io error (EIO on macOS, broken pipe on
    /// Windows) — both are normal closure, not failures.
    Closed,
}

/// Platform-agnostic child handle so the Windows ConPTY backend doesn't need
/// portable-pty's `Child` trait (Windows doesn't use portable-pty at all).
pub(crate) trait LocalChild: Send {
    /// Terminate the shell process (tab close / panel teardown).
    fn kill(&mut self);
}

/// A live local PTY session. Keeping the senders alive keeps the writer and
/// resizer threads running; call [`LocalPtySession::kill`] when the user
/// closes the tab so the shell process itself is terminated.
pub struct LocalPtySession {
    /// The shell program path (e.g. "/bin/zsh") — for tab labels.
    pub shell: String,
    pub input_tx: UnboundedSender<Vec<u8>>,
    pub resize_tx: UnboundedSender<ResizeCmd>,
    child: Box<dyn LocalChild>,
}

impl LocalPtySession {
    /// Terminate the shell process (tab close / panel teardown).
    pub fn kill(&mut self) {
        self.child.kill();
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
    spawn_pty_with_shell(&detect_shell(), cols, rows)
}

/// Spawn a specific shell program in a fresh pseudo-terminal — the testable
/// core of [`spawn_local_pty`] (lets the cmd.exe-vs-powershell comparison
/// tests pin an explicit shell).
fn spawn_pty_with_shell(
    shell: &str,
    cols: u16,
    rows: u16,
) -> Result<(LocalPtySession, UnboundedReceiver<LocalPtyEvent>)> {
    #[cfg(windows)]
    {
        spawn_windows_pty(shell, cols, rows)
    }
    #[cfg(not(windows))]
    {
        spawn_portable_pty(shell, cols, rows)
    }
}

// --- shared I/O plumbing -----------------------------------------------------

/// Resize sink for the resizer thread: the portable-pty master (Unix) and the
/// ConPTY master (Windows) both plug into the same thread.
pub(super) trait PtyResizer: Send {
    fn resize(&self, cols: u16, rows: u16);
}

/// Reader thread: blocking read loop → `Data` events; child exit → `Closed`.
/// Also arms the stall watchdog that reports a totally silent shell.
fn spawn_reader_thread<R: Read + Send + 'static>(
    mut reader: R,
    output_tx: UnboundedSender<LocalPtyEvent>,
) -> Result<()> {
    let got_data = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch_data = got_data.clone();
    let watch_tx = output_tx.clone();

    std::thread::Builder::new()
        .name("local-pty-reader".into())
        .spawn(move || {
            let mut total = 0u64;
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        total += n as u64;
                        got_data.store(true, std::sync::atomic::Ordering::Relaxed);
                        if output_tx.send(LocalPtyEvent::Data(buf[..n].to_vec())).is_err() {
                            break; // UI side is gone
                        }
                    }
                    Err(err) => {
                        // A master-side read error is how "child exited"
                        // surfaces (EIO on macOS, broken pipe on Windows) —
                        // normal closure, but the details are diagnostic.
                        log::info!("local pty reader 结束 after {total} bytes: {err}");
                        break;
                    }
                }
            }
            log::info!("local pty reader EOF after {total} bytes");
            let _ = output_tx.send(LocalPtyEvent::Closed);
        })
        .context("启动 pty 读取线程失败")?;

    // Watchdog: if nothing at all arrived within STALL_TIMEOUT, surface one
    // Stall event instead of leaving the tab silently blank.
    std::thread::Builder::new()
        .name("local-pty-stall-watchdog".into())
        .spawn(move || {
            let checks = (STALL_TIMEOUT.as_millis() / STALL_POLL.as_millis()).max(1) as u32;
            for _ in 0..checks {
                if watch_data.load(std::sync::atomic::Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(STALL_POLL);
            }
            if !watch_data.load(std::sync::atomic::Ordering::Relaxed) {
                log::warn!("local pty: {STALL_TIMEOUT:?} 内未收到任何输出");
                let _ = watch_tx.send(LocalPtyEvent::Stall);
            }
        })
        .context("启动 pty stall 看门狗失败")?;
    Ok(())
}

/// Writer thread: keystrokes → pty stdin. Exits when every input sender drops.
fn spawn_writer_thread<W: Write + Send + 'static>(
    mut writer: W,
    mut input_rx: UnboundedReceiver<Vec<u8>>,
) -> Result<()> {
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
    Ok(())
}

/// Resizer thread: owns the pty master so it stays alive for the reader/writer
/// clones above, and applies window-size changes. Exits when every resize
/// sender drops (tab closed), which closes the pty as well.
fn spawn_resizer_thread<M: PtyResizer + 'static>(
    master: M,
    mut resize_rx: UnboundedReceiver<ResizeCmd>,
) -> Result<()> {
    std::thread::Builder::new()
        .name("local-pty-resizer".into())
        .spawn(move || {
            while let Some(cmd) = resize_rx.blocking_recv() {
                master.resize(
                    cmd.cols.min(u16::MAX as u32) as u16,
                    cmd.rows.min(u16::MAX as u32) as u16,
                );
            }
        })
        .context("启动 pty resize 线程失败")?;
    Ok(())
}

/// Channel wiring shared by both backends.
fn wire_session<R, W, M>(
    shell: &str,
    reader: R,
    writer: W,
    master: M,
    child: Box<dyn LocalChild>,
) -> Result<(LocalPtySession, UnboundedReceiver<LocalPtyEvent>)>
where
    R: Read + Send + 'static,
    W: Write + Send + 'static,
    M: PtyResizer + 'static,
{
    let (input_tx, input_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let (resize_tx, resize_rx) = tokio::sync::mpsc::unbounded_channel::<ResizeCmd>();
    let (output_tx, output_rx) = tokio::sync::mpsc::unbounded_channel::<LocalPtyEvent>();
    spawn_reader_thread(reader, output_tx)?;
    spawn_writer_thread(writer, input_rx)?;
    spawn_resizer_thread(master, resize_rx)?;

    Ok((
        LocalPtySession {
            shell: shell.to_string(),
            input_tx,
            resize_tx,
            child,
        },
        output_rx,
    ))
}

// --- Windows: own ConPTY backend ---------------------------------------------

#[cfg(windows)]
fn spawn_windows_pty(
    shell: &str,
    cols: u16,
    rows: u16,
) -> Result<(LocalPtySession, UnboundedReceiver<LocalPtyEvent>)> {
    let backend = super::win_conpty::open_conpty(shell, cols, rows)
        .with_context(|| format!("打开本地伪终端失败({shell})"))?;
    wire_session(
        shell,
        backend.reader,
        backend.writer,
        backend.master,
        backend.child,
    )
}

// --- Unix: portable-pty (openpty) --------------------------------------------

#[cfg(not(windows))]
mod portable {
    use super::*;

    pub(super) struct PortableChild(pub(crate) Box<dyn portable_pty::Child + Send>);

    impl LocalChild for PortableChild {
        fn kill(&mut self) {
            if let Err(err) = self.0.kill() {
                log::warn!("local pty: kill failed: {err:#}");
            }
        }
    }

    pub(super) struct PortableMaster(pub(super) Box<dyn portable_pty::MasterPty + Send>);

    impl PtyResizer for PortableMaster {
        fn resize(&self, cols: u16, rows: u16) {
            let size = portable_pty::PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            };
            if let Err(err) = self.0.resize(size) {
                log::warn!("local pty resize 失败: {err:#}");
            }
        }
    }
}

#[cfg(not(windows))]
fn spawn_portable_pty(
    shell: &str,
    cols: u16,
    rows: u16,
) -> Result<(LocalPtySession, UnboundedReceiver<LocalPtyEvent>)> {
    use portable_pty::CommandBuilder;

    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(portable_pty::PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .with_context(|| format!("打开本地伪终端失败({shell})"))?;

    let mut cmd = CommandBuilder::new(shell);
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

    let reader = pair.master.try_clone_reader().context("克隆 pty 读取端失败")?;
    let writer = pair.master.take_writer().context("获取 pty 写入端失败")?;

    wire_session(
        shell,
        reader,
        writer,
        portable::PortableMaster(pair.master),
        Box::new(portable::PortableChild(child)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probe command that proves execution happened. The marker must NOT
    /// appear in the typed-line echo (ConPTY echoes input verbatim), only in
    /// the command's output — so each shell needs escape-char indirection:
    /// - POSIX: printf's `%s` consumes the PASSED argument.
    /// - cmd.exe: `^P` caret-escape collapses to a literal `P` at parse time.
    /// - PowerShell: backtick before an ordinary char yields that char.
    fn probe_command(shell_name: &str) -> Vec<u8> {
        #[cfg(windows)]
        {
            if shell_name.contains("cmd") {
                b"echo VT_^PASSED_OK\r".to_vec()
            } else {
                b"echo VT_`PASSED_OK\r".to_vec()
            }
        }
        #[cfg(not(windows))]
        {
            let _ = shell_name;
            b"printf 'VT_%s_OK\\r\\n' PASSED\r".to_vec()
        }
    }

    /// Pump output until `marker` shows up (or `Closed` / deadline). Returns
    /// (marker_seen, closed_seen) plus everything received, for diagnostics.
    fn pump_until_marker(
        session: &mut LocalPtySession,
        output_rx: &mut UnboundedReceiver<LocalPtyEvent>,
        marker: &[u8],
    ) -> (bool, bool, Vec<u8>) {
        session
            .input_tx
            .send(probe_command(&session.shell_name()))
            .expect("send probe");

        let mut out: Vec<u8> = Vec::new();
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
                Ok(LocalPtyEvent::Stall) => {}
                Ok(LocalPtyEvent::Closed) => return (saw_marker, true, out),
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        (saw_marker, false, out)
    }

    fn expect_closed(output_rx: &mut UnboundedReceiver<LocalPtyEvent>) -> bool {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        while std::time::Instant::now() < deadline {
            match output_rx.try_recv() {
                Ok(LocalPtyEvent::Closed) => return true,
                Ok(LocalPtyEvent::Stall) | Ok(LocalPtyEvent::Data(_)) => {}
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(20)),
            }
        }
        false
    }

    /// End-to-end pump check for the auto-detected shell: run the probe, then
    /// exit — the reader must deliver the program output followed by Closed.
    #[test]
    fn spawn_exec_and_exit() {
        let (mut session, mut output_rx) = spawn_local_pty(80, 24).expect("spawn_local_pty");
        let shell = session.shell_name();

        let (saw_marker, closed, out) =
            pump_until_marker(&mut session, &mut output_rx, b"VT_PASSED_OK");
        assert!(
            saw_marker,
            "marker not found; shell={shell} out={:?}",
            String::from_utf8_lossy(&out)
        );

        session.input_tx.send(b"exit\r".to_vec()).expect("send exit");
        let closed_after_exit = expect_closed(&mut output_rx);
        assert!(closed_after_exit || closed, "expected Closed event after `exit`");
        session.kill();
    }

    /// cmd.exe control group: cmd is a native console app, so if this passes
    /// while `spawn_exec_and_exit` (powershell) produces nothing, the failure
    /// is specific to .NET-based shells and their std-handle handling; if both
    /// are silent, the ConPTY invocation itself is at fault.
    #[cfg(windows)]
    #[test]
    fn spawn_cmd_exec_and_exit() {
        let (mut session, mut output_rx) =
            spawn_pty_with_shell("cmd.exe", 80, 24).expect("spawn cmd pty");
        assert_eq!(session.shell_name(), "cmd.exe");

        let (saw_marker, _closed, out) =
            pump_until_marker(&mut session, &mut output_rx, b"VT_PASSED_OK");
        assert!(
            saw_marker,
            "cmd marker not found; out={:?}",
            String::from_utf8_lossy(&out)
        );

        session.input_tx.send(b"exit\r".to_vec()).expect("send exit");
        let closed = expect_closed(&mut output_rx);
        assert!(closed, "expected Closed event after `exit`");
        session.kill();
    }
}
