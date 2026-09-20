//! Windows ConPTY backend for the SSH panel's local-terminal mode.
//!
//! portable-pty 0.9 spawns the ConPTY client with `STARTF_USESTDHANDLES` and
//! all three std handles set to `INVALID_HANDLE_VALUE`, plus the undocumented
//! `CreatePseudoConsole` flags `RESIZE_QUIRK | WIN32_INPUT_MODE`. On current
//! Windows 11 builds (observed on 26200) that shape leaves the spawned shell
//! alive but completely silent — cmd.exe included: only conhost's startup
//! `ESC[6n` probe ever arrives on the master side (the
//! `ssh::local_pty::tests` end-to-end cases pin this behavior).
//!
//! This module implements the documented ConPTY usage from Microsoft's
//! pseudoconsole sample instead: `CreatePseudoConsole(flags = 0)`, a
//! `STARTUPINFOEXW` whose only attribute is the pseudoconsole, **no**
//! `STARTF_USESTDHANDLES` (so the child gets real console std handles), and
//! `lpApplicationName` + a quoted command line. The pipes feeding the
//! reader/writer threads and the resize plumbing are shared with the
//! portable-pty backend in `local_pty.rs`.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::mem::{size_of, zeroed, MaybeUninit};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::sync::Arc;

use anyhow::{anyhow, ensure, Context, Result};
use winapi::shared::winerror::S_OK;
use winapi::um::consoleapi::{ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole};
use winapi::um::processthreadsapi::{
    CreateProcessW, DeleteProcThreadAttributeList, InitializeProcThreadAttributeList,
    TerminateProcess, UpdateProcThreadAttribute, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
};
use winapi::um::synchapi::WaitForSingleObject;
use winapi::um::winbase::{
    CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT, INFINITE, STARTUPINFOEXW,
};
use winapi::um::wincontypes::COORD;
use winapi::um::winnt::HANDLE;

use super::local_pty::{LocalChild, PtyResizer};

/// `PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE` — winapi 0.3 does not export it.
const PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE: usize = 0x0002_0016;

/// The pseudoconsole handle plus everything needed to drive one session.
pub(super) struct ConPtyBackend {
    /// Console output (conhost → terminal). Moved into the reader thread.
    pub reader: std::io::PipeReader,
    /// Console input (keyboard → conhost). Moved into the writer thread.
    pub writer: std::io::PipeWriter,
    /// Keeps the pseudoconsole alive; drives resizes.
    pub master: Arc<ConPtyMaster>,
    pub child: Box<dyn LocalChild>,
}

/// Owning wrapper around the raw `HPCON`. Closing it (explicitly from the
/// child watcher, or on drop) ends the console and closes the output pipe the
/// reader thread blocks on.
pub(super) struct ConPtyMaster {
    hpc: HANDLE,
    closed: std::sync::atomic::AtomicBool,
}
// Raw handle wrapper: used from the resizer thread (and closed there on drop).
unsafe impl Send for ConPtyMaster {}
unsafe impl Sync for ConPtyMaster {}

impl ConPtyMaster {
    fn resize_pseudoconsole(&self, cols: u16, rows: u16) -> Result<()> {
        let result = unsafe {
            ResizePseudoConsole(self.hpc, COORD {
                X: cols as i16,
                Y: rows as i16,
            })
        };
        ensure!(
            result == S_OK,
            "resize pseudo console to {cols}x{rows} failed: HRESULT {result:#x}"
        );
        Ok(())
    }

    /// Close the pseudoconsole exactly once. On current Windows builds the
    /// conhost keeps serving (and holding the output pipe open) after its
    /// client exits — EOF on the master side only happens once the console is
    /// closed, which is what the MS pseudoconsole sample does explicitly.
    fn terminate(&self) {
        if !self
            .closed
            .swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            unsafe { ClosePseudoConsole(self.hpc) };
        }
    }
}

impl PtyResizer for Arc<ConPtyMaster> {
    fn resize(&self, cols: u16, rows: u16) {
        if let Err(err) = self.resize_pseudoconsole(cols, rows) {
            log::warn!("local pty resize 失败: {err:#}");
        }
    }
}

impl Drop for ConPtyMaster {
    fn drop(&mut self) {
        self.terminate();
    }
}

pub(super) struct ConPtyChild {
    proc: OwnedHandle,
}

impl LocalChild for ConPtyChild {
    fn kill(&mut self) {
        unsafe {
            if TerminateProcess(self.proc.as_raw_handle(), 1) == 0 {
                log::warn!(
                    "local pty: TerminateProcess failed: {}",
                    std::io::Error::last_os_error()
                );
            }
            // Reap so the process object (and our handle) is released.
            WaitForSingleObject(self.proc.as_raw_handle(), INFINITE);
        }
    }
}

/// Create the pseudoconsole + pipes and spawn `shell` inside it.
pub(super) fn open_conpty(shell: &str, cols: u16, rows: u16) -> Result<ConPtyBackend> {
    // in:  our_in_write → conhost_in_read → console keyboard buffer
    // out: console screen → conhost_out_write → our_out_read
    let (conhost_in_read, our_in_write) = std::io::pipe().context("创建 ConPTY 输入管道失败")?;
    let (our_out_read, conhost_out_write) = std::io::pipe().context("创建 ConPTY 输出管道失败")?;

    let mut hpc: HANDLE = std::ptr::null_mut();
    let result = unsafe {
        CreatePseudoConsole(
            COORD {
                X: cols as i16,
                Y: rows as i16,
            },
            conhost_in_read.as_raw_handle(),
            conhost_out_write.as_raw_handle(),
            0,
            &mut hpc,
        )
    };
    ensure!(
        result == S_OK,
        "CreatePseudoConsole failed: HRESULT {result:#x}"
    );
    // On success the pseudoconsole has duplicated both pipe handles, so our
    // conhost-side ends can go away (same as the reference sample).
    drop(conhost_in_read);
    drop(conhost_out_write);

    let master = Arc::new(ConPtyMaster {
        hpc,
        closed: std::sync::atomic::AtomicBool::new(false),
    });
    let child = spawn_client(hpc, shell).context("启动本地 shell 失败")?;

    // Watcher: when the shell exits, close the pseudoconsole so the output
    // pipe hits EOF and the reader thread reports Closed. Keeps a master
    // reference so the console outlives an early resizer-thread shutdown.
    let watch_master = master.clone();
    let watch_proc = child.proc.try_clone().context("克隆 shell 进程句柄失败")?;
    std::thread::Builder::new()
        .name("local-pty-child-watcher".into())
        .spawn(move || {
            unsafe { WaitForSingleObject(watch_proc.as_raw_handle(), INFINITE) };
            log::info!("local pty: shell 进程退出，关闭伪终端");
            watch_master.terminate();
        })
        .context("启动 pty child watcher 失败")?;

    Ok(ConPtyBackend {
        reader: our_out_read,
        writer: our_in_write,
        master,
        child: Box::new(child),
    })
}

/// Spawn `shell` attached to the pseudoconsole via the documented
/// EXTENDED_STARTUPINFO_PRESENT + PSEUDOCONSOLE-attribute dance.
fn spawn_client(hpc: HANDLE, shell: &str) -> Result<ConPtyChild> {
    // Quoted copy as the command line; lpApplicationName stays NULL so
    // CreateProcessW resolves bare names (e.g. "cmd.exe") through the usual
    // search order.
    let mut cmdline: Vec<u16> = OsStr::new(&format!("\"{shell}\""))
        .encode_wide()
        .collect();
    cmdline.push(0);

    let mut env_block = environment_block();
    let cwd = current_directory();

    // Without STARTF_USESTDHANDLES the child inherits the PARENT's std
    // handle values. A console parent (cargo run, tests) would therefore
    // leak its real console into the shell — PowerShell then writes the
    // banner/prompt there instead of into the pseudoconsole. Nulling our
    // std handles around the call makes the child initialize fresh
    // CONIN$/CONOUT$ for its own console, which IS the pseudoconsole —
    // the same fallback a std-less GUI process gets naturally.
    let saved_std = take_std_handles();
    let spawn_result =
        create_pseudoconsole_client(&mut cmdline, &mut env_block, cwd.as_deref(), hpc, shell);
    restore_std_handles(saved_std);
    spawn_result
}

/// The CreateProcessW call proper (std handles are managed by the caller).
fn create_pseudoconsole_client(
    cmdline: &mut [u16],
    env_block: &mut [u16],
    cwd: Option<&[u16]>,
    hpc: HANDLE,
    shell: &str,
) -> Result<ConPtyChild> {
    unsafe {
        // Probe the required attribute-list size, then allocate it. The list
        // holds pointers internally, so allocate it usize-aligned.
        let mut attr_size: usize = 0;
        let _ = InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut attr_size);
        ensure!(
            attr_size > 0,
            "InitializeProcThreadAttributeList 未返回所需大小"
        );
        let mut list =
            vec![MaybeUninit::<usize>::uninit(); attr_size.div_ceil(size_of::<usize>())];
        let list_ptr = list.as_mut_ptr().cast::<PROC_THREAD_ATTRIBUTE_LIST>();
        ensure!(
            InitializeProcThreadAttributeList(list_ptr, 1, 0, &mut attr_size) != 0,
            "InitializeProcThreadAttributeList 失败: {}",
            std::io::Error::last_os_error()
        );

        if UpdateProcThreadAttribute(
            list_ptr,
            0,
            PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE,
            hpc.cast(),
            size_of::<HANDLE>(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        ) == 0
        {
            let err = std::io::Error::last_os_error();
            DeleteProcThreadAttributeList(list_ptr);
            return Err(anyhow!("UpdateProcThreadAttribute 失败: {err}"));
        }

        // Plain STARTUPINFOEXW: dwFlags stays 0 — notably NO
        // STARTF_USESTDHANDLES, so the child gets fresh console std handles
        // instead of invalid placeholders (portable-pty 0.9 sets those and
        // the shell goes silent).
        let mut si: STARTUPINFOEXW = zeroed();
        si.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
        si.lpAttributeList = list_ptr;
        let mut pi: PROCESS_INFORMATION = zeroed();

        let ok = CreateProcessW(
            std::ptr::null_mut(),
            cmdline.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            EXTENDED_STARTUPINFO_PRESENT | CREATE_UNICODE_ENVIRONMENT,
            env_block.as_mut_ptr().cast(),
            cwd.map_or(std::ptr::null(), |c| c.as_ptr()),
            &mut si.StartupInfo,
            &mut pi,
        );
        DeleteProcThreadAttributeList(list_ptr);
        if ok == 0 {
            return Err(anyhow!(
                "CreateProcessW({shell}) 失败: {}",
                std::io::Error::last_os_error()
            ));
        }

        // Own the process handle for kill/wait; close the thread handle.
        let _thread = OwnedHandle::from_raw_handle(pi.hThread);
        let process = OwnedHandle::from_raw_handle(pi.hProcess);
        Ok(ConPtyChild { proc: process })
    }
}

/// Snapshot our std handle values and set them to NULL (see `spawn_client`).
fn take_std_handles() -> [HANDLE; 3] {
    use winapi::um::processenv::{GetStdHandle, SetStdHandle};
    use winapi::um::winbase::{STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    unsafe {
        let saved = [
            GetStdHandle(STD_INPUT_HANDLE),
            GetStdHandle(STD_OUTPUT_HANDLE),
            GetStdHandle(STD_ERROR_HANDLE),
        ];
        for slot in [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            SetStdHandle(slot, std::ptr::null_mut());
        }
        saved
    }
}

/// Restore the std handle values captured by [`take_std_handles`].
fn restore_std_handles(saved: [HANDLE; 3]) {
    use winapi::um::processenv::SetStdHandle;
    use winapi::um::winbase::{STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE};
    unsafe {
        let _ = SetStdHandle(STD_INPUT_HANDLE, saved[0]);
        let _ = SetStdHandle(STD_OUTPUT_HANDLE, saved[1]);
        let _ = SetStdHandle(STD_ERROR_HANDLE, saved[2]);
    }
}

/// UTF-16 environment block: the current process environment plus the same
/// overrides the portable-pty path applies (TERM, derived LANG). Keys are
/// folded case-insensitively — a duplicate `TERM=`/`term=` pair in the block
/// would make the child's env lookups ambiguous.
fn environment_block() -> Vec<u16> {
    let mut env: BTreeMap<String, (OsString, OsString)> = BTreeMap::new();
    for (key, value) in std::env::vars_os() {
        let fold = key.to_string_lossy().to_ascii_lowercase();
        env.entry(fold).or_insert((key, value));
    }
    let mut put = |key: &str, value: String| {
        env.insert(
            key.to_ascii_lowercase(),
            (OsString::from(key), OsString::from(value)),
        );
    };
    put("TERM", "xterm-256color".to_string());
    if std::env::var_os("LANG").is_none() {
        let lang = sys_locale::get_locale()
            .map(|l| l.replace(['-', '_'], "_"))
            .unwrap_or_else(|| "en_US".to_string());
        put("LANG", format!("{lang}.UTF-8"));
    }

    let mut block: Vec<u16> = Vec::new();
    for (key, value) in env.values() {
        block.extend(key.encode_wide());
        block.push(b'=' as u16);
        block.extend(value.encode_wide());
        block.push(0);
    }
    // Final terminator expected by CreateProcessW.
    block.push(0);
    block
}

/// HOME (when valid) else USERPROFILE, null-terminated for CreateProcessW.
fn current_directory() -> Option<Vec<u16>> {
    let home = std::env::var_os("HOME")
        .filter(|p| Path::new(p).is_dir())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|p| Path::new(p).is_dir()))?;
    let mut wide: Vec<u16> = home.encode_wide().collect();
    wide.push(0);
    Some(wide)
}
