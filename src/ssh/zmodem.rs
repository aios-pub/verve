//! ZMODEM (rz/sz) interception + transfer.
//!
//! Detects ZMODEM handshake frames in the SSH inbound stream, drives a
//! `zmodem2` poll-based state machine on the pump thread, and communicates
//! with the UI via [`ZmEvent`]s. Open/save dialogs are requested by sending
//! a [`ZmEvent::NeedSavePath`] / [`ZmEvent::NeedFileToSend`] which carries a
//! synchronous reply channel; the engine blocks (on the pump thread, which is
//! fine) until the UI replies through it.

use std::fs::File;
use std::io::{Read, Write};
use std::path::PathBuf;
// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZmRole {
    /// Remote ran `rz` — we send a local file (upload).
    SendToRemote,
    /// Remote ran `sz` — we receive a file (download).
    ReceiveFromRemote,
}

/// ZMODEM ZHEX frame start: `*`, `*`, ZDLE (0x18), `B` (0x42 = ZHEX encoding).
/// Two ASCII hex digits encoding the frame type follow, e.g. "00" = ZRQINIT
/// (remote ran `sz` — we receive) and "01" = ZRINIT (remote ran `rz` — we
/// send). lrzsz announces itself with one of these ZHEX frames.
const TRIGGER: &[u8] = b"**\x18B";

/// Events the engine surfaces to the UI.
pub enum ZmEvent {
    /// Transfer is starting. (After the user chose the file path.)
    Started {
        filename: String,
        total: u64,
        role: ZmRole,
    },
    Progress {
        filename: String,
        transferred: u64,
        total: u64,
        role: ZmRole,
    },
    Completed {
        filename: String,
        role: ZmRole,
        saved_to: Option<PathBuf>,
    },
    /// Transfer was cancelled (typically by the user dismissing the dialog).
    /// UI should not show an error; just stop displaying the progress bar
    /// and return to normal terminal rendering.
    Cancelled,
    /// UI must show a "save file" dialog with the suggested name, then call
    /// [`ZmodemSession::provide_save_path`].
    NeedSavePath {
        suggested_name: String,
    },
    /// UI must show an "open file" dialog, then call
    /// [`ZmodemSession::provide_file_to_send`].
    NeedFileToSend,
    Error {
        message: String,
    },
}

/// Sliding-window matcher for the ZMODEM trigger bytes in the inbound stream.
pub struct ZmodemDetector {
    matched: usize,
}

impl ZmodemDetector {
    pub fn new() -> Self {
        Self { matched: 0 }
    }
    pub fn reset(&mut self) {
        self.matched = 0;
    }
    /// Returns the byte index (into `data`) where the trigger completed if a
    /// ZMODEM session starts here, otherwise None. When Some(idx) is returned,
    /// bytes `data[..=idx]` should NOT be forwarded to the terminal; the full
    /// `data` slice should be passed to the new session.
    pub fn feed(&mut self, data: &[u8]) -> Option<usize> {
        for (i, &b) in data.iter().enumerate() {
            if b == TRIGGER[self.matched] {
                self.matched += 1;
                if self.matched == TRIGGER.len() {
                    self.matched = 0;
                    return Some(i);
                }
            } else {
                self.matched = if b == TRIGGER[0] { 1 } else { 0 };
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// ZmodemSession — drives the zmodem2 state machine
// ---------------------------------------------------------------------------

pub struct ZmodemSession {
    role: Option<ZmRole>,
    initial_buf: Vec<u8>,
    /// Wire bytes the state machine could not consume yet (e.g. ZDATA that
    /// arrived while a file-path prompt is still unanswered). Re-submitted
    /// on resume()/the next feed; dropping them would corrupt the transfer.
    pending_wire: Vec<u8>,
    receiver: Option<zmodem2::Receiver>,
    sender: Option<zmodem2::Sender>,
    file: Option<File>,
    saved_to: Option<PathBuf>,
    filename: String,
    total: u64,
    transferred: u64,
    outgoing: Vec<u8>,
    events: Vec<ZmEvent>,
    done: bool,
    /// True when the session ended via cancel/abort/error rather than a
    /// clean protocol completion. The pump thread uses this to decide
    /// whether a post-session trigger-detection grace window is needed.
    cancelled: bool,
    /// Tracks whether we've already emitted the NeedFileToSend prompt.
    send_prompt_emitted: bool,
    /// Tracks whether we've already emitted NeedSavePath for receiver side.
    recv_prompt_emitted: bool,
}

/// Cheap debug label for an Action variant.
fn action_label(a: &zmodem2::Action) -> &'static str {
    match a {
        zmodem2::Action::WriteWire(_) => "WriteWire",
        zmodem2::Action::ReadFile { .. } => "ReadFile",
        zmodem2::Action::WriteFile(_) => "WriteFile",
        zmodem2::Action::Event(_) => "Event",
        zmodem2::Action::Idle => "Idle",
        _ => "?",
    }
}

impl ZmodemSession {
    pub fn new() -> Self {
        Self {
            role: None,
            initial_buf: Vec::new(),
            pending_wire: Vec::new(),
            receiver: None,
            sender: None,
            file: None,
            saved_to: None,
            filename: String::new(),
            total: 0,
            transferred: 0,
            outgoing: Vec::new(),
            events: Vec::new(),
            done: false,
            cancelled: false,
            send_prompt_emitted: false,
            recv_prompt_emitted: false,
        }
    }

    pub fn role(&self) -> Option<ZmRole> {
        self.role
    }
    pub fn is_done(&self) -> bool {
        self.done
    }
    /// True when the session ended via user-cancel, abort, or protocol error
    /// (anything routed through cancel_with_event) instead of completing the
    /// closing handshake cleanly.
    pub fn was_cancelled(&self) -> bool {
        self.cancelled
    }

    /// Drain pending UI events. The SSH panel's pump loop calls this after
    /// each feed and forwards events to the GPUI UI.
    pub fn pop_events(&mut self) -> Vec<ZmEvent> {
        std::mem::take(&mut self.events)
    }

    /// Outbound bytes that must be written to the SSH input channel.
    /// Drains the internal buffer.
    pub fn drain_outgoing(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.outgoing)
    }

    /// Feed inbound SSH bytes. Called by the pump thread whenever data arrives
    /// from SSH. This function drives the state machine synchronously and may
    /// block briefly when a UI prompt is pending (we wait on a sync channel).
    pub fn feed(&mut self, data: &[u8]) {
        log::info!(
            "zmodem feed {} bytes; role={:?} done={}",
            data.len(),
            self.role,
            self.done
        );
        if self.done {
            return;
        }
        let mut resolved_now = false;
        let mut skip_prefix: usize = 0;
        if self.role.is_none() {
            self.initial_buf.extend_from_slice(data);
            if let Some(trigger_pos) = self.try_resolve_role() {
                // Discard bytes BEFORE the ZMODEM trigger — they are echoed
                // terminal text (the "rz" command, prompt, etc.) and must
                // not be fed to the state machine, otherwise they corrupt
                // protocol parsing.
                skip_prefix = trigger_pos;
                resolved_now = true;
            } else {
                return;
            }
        }
        if resolved_now {
            let initial = std::mem::take(&mut self.initial_buf);
            // initial holds everything so far including the echoed prefix
            // before the trigger. Feed from trigger_pos onward only.
            if initial.len() > skip_prefix {
                self.submit_wire(&initial[skip_prefix..]);
                self.run();
                if self.done {
                    return;
                }
            }
        } else if !data.is_empty() || !self.pending_wire.is_empty() {
            // Prepend any bytes the state machine couldn't consume earlier
            // so the wire order is preserved.
            if self.pending_wire.is_empty() {
                self.submit_wire(data);
            } else {
                let mut combined = std::mem::take(&mut self.pending_wire);
                combined.extend_from_slice(data);
                self.submit_wire(&combined);
            }
            self.run();
        }
    }

    /// Reply to a `NeedSavePath` event with the chosen save location. Pass
    /// None to cancel the transfer. After calling this, invoke `resume()`
    /// (or just call `feed(&[])` ) to continue processing.
    pub fn provide_save_path(&mut self, path: Option<PathBuf>) {
        self.provide_path(path, ZmRole::ReceiveFromRemote);
    }

    /// Reply to a `NeedFileToSend` event with the chosen file to upload.
    /// Pass None to cancel.
    pub fn provide_file_to_send(&mut self, path: Option<PathBuf>) {
        self.provide_path(path, ZmRole::SendToRemote);
    }

    /// Continue driving the state machine after a path was provided. The
    /// pump thread calls this after calling `provide_*_path`.
    pub fn resume(&mut self) {
        // Even if cancelled (done=true), drain outgoing ZCAN bytes that
        // cancel() queued so the remote lrzsz actually sees the abort.
        if self.done {
            return;
        }
        // Re-submit any wire bytes that were held back while the prompt was
        // unanswered before continuing the poll loop.
        let pending = std::mem::take(&mut self.pending_wire);
        if !pending.is_empty() {
            self.submit_wire(&pending);
        }
        self.run();
    }

    fn provide_path(&mut self, path: Option<PathBuf>, for_role: ZmRole) {
        let Some(path) = path else {
            self.cancel_user();
            return;
        };
        match for_role {
            ZmRole::ReceiveFromRemote => match File::create(&path) {
                Ok(f) => {
                    self.saved_to = Some(path.clone());
                    self.file = Some(f);
                    let fname = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file")
                        .to_string();
                    self.filename = fname.clone();
                    self.events.push(ZmEvent::Started {
                        filename: fname,
                        total: self.total,
                        role: ZmRole::ReceiveFromRemote,
                    });
                }
                Err(e) => {
                    self.cancel(&format!("无法创建文件 {}: {}", path.display(), e));
                }
            },
            ZmRole::SendToRemote => match File::open(&path) {
                Ok(f) => {
                    let size = f.metadata().map(|m| m.len()).unwrap_or(0);
                    let fname = path
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("file")
                        .to_string();
                    self.file = Some(f);
                    self.total = size;
                    self.filename = fname.clone();

                    let info = zmodem2::FileInfo::new(
                        fname.as_bytes(),
                        Some(zmodem2::Position::new(size.min(u32::MAX as u64) as u32)),
                    );
                    if let Some(s) = &mut self.sender {
                        match s.start_file(info) {
                            Ok(()) => {
                                log::info!("zmodem start_file OK, size={}", size);
                            }
                            Err(e) => {
                                log::error!("zmodem start_file failed: {:?}", e);
                                self.cancel(&format!("start_file 失败: {e}"));
                                return;
                            }
                        }
                    }
                    self.events.push(ZmEvent::Started {
                        filename: fname,
                        total: size,
                        role: ZmRole::SendToRemote,
                    });
                }
                Err(e) => {
                    self.cancel(&format!("无法打开文件 {}: {}", path.display(), e));
                }
            },
        }
    }

    // -- internal --

    fn cancel(&mut self, msg: &str) {
        self.cancel_with_event(ZmEvent::Error {
            message: msg.to_string(),
        });
    }

    /// Abort the session, send ZCAN on the wire, and push the given event
    /// (either Cancelled or Error).
    fn cancel_with_event(&mut self, ev: ZmEvent) {
        // zmodem2's abort() just sets an internal flag; it doesn't queue the
        // ZCAN wire bytes itself. The ZMODEM spec says we can abort by sending
        // a series of CAN (0x18) bytes followed by a BS (0x08); lrzsz
        // recognizes this sequence reliably. We also send ZDLE+ZCAN via the
        // state machine in case it has queued anything.
        let mut to_send: Vec<u8> = Vec::new();
        // Take and drop the state machines.
        if let Some(mut r) = self.receiver.take() {
            let _ = r.abort();
            let n = match r.poll() {
                zmodem2::Action::WriteWire(b) => {
                    to_send.extend_from_slice(b);
                    b.len()
                }
                _ => 0,
            };
            if n > 0 {
                r.wire_written(n);
            }
        }
        if let Some(mut s) = self.sender.take() {
            s.abort();
            let n = match s.poll() {
                zmodem2::Action::WriteWire(b) => {
                    to_send.extend_from_slice(b);
                    b.len()
                }
                _ => 0,
            };
            if n > 0 {
                s.wire_written(n);
            }
        }
        // If the state machine didn't produce anything (most cases), send the
        // canonical abort sequence: eight CAN bytes plus BS. lrzsz checks for
        // two consecutive CANs as abort, but sending eight is the traditional
        // value used by the reference lrzsz implementation.
        if to_send.is_empty() {
            to_send.extend(std::iter::repeat(0x18u8).take(8));
            to_send.push(0x08);
        }
        self.outgoing.extend_from_slice(&to_send);
        self.events.push(ev);
        self.done = true;
        self.cancelled = true;
    }

    fn cancel_user(&mut self) {
        self.cancel_with_event(ZmEvent::Cancelled);
    }

    /// Inspect the buffered bytes after the `**\x18B` trigger and read the
    /// ZF type byte to determine role, then instantiate the state machine.
    /// Returns Some(trigger_pos) if role was resolved (position in
    /// initial_buf where the ZMODEM frame starts).
    fn try_resolve_role(&mut self) -> Option<usize> {
        let buf = &self.initial_buf;
        let trigger_pos = buf.windows(4).position(|w| w == TRIGGER)?;
        let after = &buf[trigger_pos + 4..];
        if after.len() < 2 {
            return None;
        }
        // ZHEX headers encode the frame type as two ASCII hex digits.
        // after[0] is the high digit ('0' for all types we care about);
        // after[1] is the low digit:
        //   '0' (ZRQINIT) — sent by the sending program, i.e. remote ran
        //       `sz`, so WE receive the file.
        //   '1' (ZRINIT) — sent by the receiving program, i.e. remote ran
        //       `rz`, so WE send the file.
        // NOTE: compare against the ASCII byte b'0', not integer 0 — an
        // integer comparison never matches and forces every session into
        // the sender role, breaking `sz` downloads.
        let zf_type = after[1];
        let role = if zf_type == b'0' {
            ZmRole::ReceiveFromRemote
        } else {
            ZmRole::SendToRemote
        };
        self.role = Some(role);
        match role {
            ZmRole::ReceiveFromRemote => match zmodem2::Receiver::new() {
                Ok(r) => self.receiver = Some(r),
                Err(e) => {
                    self.events.push(ZmEvent::Error {
                        message: format!("ZMODEM receiver init 失败: {e}"),
                    });
                    self.done = true;
                }
            },
            ZmRole::SendToRemote => match zmodem2::Sender::new() {
                Ok(s) => self.sender = Some(s),
                Err(e) => {
                    self.events.push(ZmEvent::Error {
                        message: format!("ZMODEM sender init 失败: {e}"),
                    });
                    self.done = true;
                }
            },
        }
        Some(trigger_pos)
    }

    /// Submit all of `data` into the state machine, driving it forward as we
    /// go. The state machine refuses wire bytes while it has outgoing pending
    /// (Backpressure), so we drive it via poll_once() before, between, and
    /// after submissions until it is Idle/blocked.
    fn submit_wire(&mut self, mut data: &[u8]) {
        // First drive any pre-existing work (e.g. Sender's initial ZRQINIT).
        while !self.done && self.poll_once() {}
        while !data.is_empty() && !self.done {
            let consumed = if let Some(r) = &mut self.receiver {
                match r.submit_wire(data) {
                    Ok(n) => n,
                    Err(e) => {
                        self.cancel(&format!("ZMODEM 协议错误: {e}"));
                        return;
                    }
                }
            } else if let Some(s) = &mut self.sender {
                match s.submit_wire(data) {
                    Ok(n) => n,
                    Err(e) => {
                        self.cancel(&format!("ZMODEM 协议错误: {e}"));
                        return;
                    }
                }
            } else {
                return;
            };
            log::info!("zmodem submit_wire: consumed={}/{}", consumed, data.len());
            if consumed == 0 {
                // Blocked — drive the machine (flush acks, handle events)
                // and retry. If nothing moves we are waiting on a file-path
                // prompt: stash the unsubmitted bytes instead of dropping
                // them (dropping would corrupt the transfer).
                if !self.poll_once() {
                    log::info!("zmodem submit_wire blocked; stashing {} bytes", data.len());
                    self.pending_wire.extend_from_slice(data);
                    break;
                }
                continue;
            }
            data = &data[consumed..];
            // Drive the machine so wire bytes produced in response (acks,
            // ZDATA, ZFIN…) are flushed to self.outgoing before the next
            // submission.
            while !self.done && self.poll_once() {}
        }
    }

    /// Loop: drive the state machine via poll_once() until it is Idle or
    /// blocked. Prompts for file paths are handled by sending an event and
    /// then returning — the pump thread forwards the event to the UI, which
    /// calls back into provide_*_path; feed/resume then continues this loop.
    fn run(&mut self) {
        if self.done {
            return;
        }
        log::info!(
            "zmodem run loop starting; role={:?} events={}",
            self.role,
            self.events.len()
        );
        loop {
            if self.done {
                return;
            }

            // Emit prompt events if needed. After emitting, return from run()
            // so the pump thread can forward the prompt event to the UI. The
            // UI calls provide_save_path / provide_file_to_send (which opens
            // the file), then calls resume()/feed() to continue.
            if self.should_prompt_for_file() {
                if self.role == Some(ZmRole::ReceiveFromRemote) {
                    let suggested = self.filename.clone();
                    self.events.push(ZmEvent::NeedSavePath {
                        suggested_name: suggested,
                    });
                    self.recv_prompt_emitted = true;
                } else {
                    self.events.push(ZmEvent::NeedFileToSend);
                    self.send_prompt_emitted = true;
                }
                return;
            }

            if !self.poll_once() {
                return;
            }
        }
    }

    /// Poll the state machine once and fully handle the returned action.
    ///
    /// This is the ONLY place `poll()` is called. zmodem2's `poll()` pops
    /// events and file-data destructively — a helper that polls and handles
    /// only some action kinds would silently drop protocol events (e.g.
    /// FileCompleted/SessionCompleted) and stall the session.
    ///
    /// Returns `true` when progress was made (poll again), `false` when the
    /// machine is Idle or blocked waiting for something external: more wire
    /// bytes, or a file path from the UI prompt (an un-acked WriteFile /
    /// ReadFile is re-offered by zmodem2 on the next poll, so nothing is
    /// lost by stopping here).
    fn poll_once(&mut self) -> bool {
        let action = if let Some(r) = &mut self.receiver {
            r.poll()
        } else if let Some(s) = &mut self.sender {
            s.poll()
        } else {
            return false;
        };
        log::info!("zmodem poll action: {:?}", action_label(&action));

        match action {
            zmodem2::Action::WriteWire(b) => {
                self.outgoing.extend_from_slice(b);
                let n = b.len();
                // Tell the state machine the bytes were written. Without
                // this, zmodem2 keeps outgoing() == true and refuses all
                // further submit_wire (Backpressure), stalling the session.
                if let Some(r) = &mut self.receiver {
                    r.wire_written(n);
                } else if let Some(s) = &mut self.sender {
                    s.wire_written(n);
                }
                true
            }
            zmodem2::Action::WriteFile(chunk) => {
                let Some(f) = &mut self.file else {
                    // No file open — the save-path prompt is still pending.
                    // The chunk stays buffered inside zmodem2 (it is only
                    // acked by file_written), so it will be offered again
                    // after the UI provides the path.
                    return false;
                };
                if let Err(e) = f.write_all(chunk) {
                    self.cancel(&format!("写入文件失败: {e}"));
                    return false;
                }
                let n = chunk.len();
                self.transferred += n as u64;
                self.events.push(ZmEvent::Progress {
                    filename: self.filename.clone(),
                    transferred: self.transferred,
                    total: self.total,
                    role: ZmRole::ReceiveFromRemote,
                });
                if let Some(r) = &mut self.receiver {
                    if let Err(e) = r.file_written(n) {
                        self.cancel(&format!("file_written: {e}"));
                        return false;
                    }
                }
                true
            }
            zmodem2::Action::ReadFile { offset, max_len } => {
                let Some(f) = &mut self.file else {
                    // Open-file prompt still pending; the read request stays
                    // queued inside zmodem2 until submit_file answers it.
                    return false;
                };
                use std::io::Seek;
                let off = offset.get() as u64;
                if let Err(e) = f.seek(std::io::SeekFrom::Start(off)) {
                    self.cancel(&format!("seek 失败: {e}"));
                    return false;
                }
                let mut buf = vec![0u8; max_len];
                let n = match f.read(&mut buf) {
                    Ok(n) => n,
                    Err(e) => {
                        self.cancel(&format!("读取文件失败: {e}"));
                        return false;
                    }
                };
                buf.truncate(n);
                log::info!(
                    "zmodem ReadFile off={} max={} read={} total={}",
                    off,
                    max_len,
                    n,
                    self.total
                );
                if n == 0 {
                    // EOF — shouldn't normally happen (offset==total triggers
                    // ZEOF via zmodem2 itself), but handle defensively. Stop
                    // polling; the read request stays pending.
                    if let Some(s) = &mut self.sender {
                        let _ = s.finish();
                    }
                    return false;
                }
                if let Some(s) = &mut self.sender {
                    if let Err(e) = s.submit_file(&buf[..n]) {
                        self.cancel(&format!("submit_file: {e}"));
                        return false;
                    }
                }
                self.transferred = off + n as u64;
                self.events.push(ZmEvent::Progress {
                    filename: self.filename.clone(),
                    transferred: self.transferred,
                    total: self.total,
                    role: ZmRole::SendToRemote,
                });
                true
            }
            zmodem2::Action::Event(ev) => {
                match ev {
                    zmodem2::Event::FileStarted(info) => {
                        let is_receiver = self.role == Some(ZmRole::ReceiveFromRemote);
                        if is_receiver {
                            let name = String::from_utf8_lossy(info.name).to_string();
                            let size = info.size.map(|p| p.get()).unwrap_or(0) as u64;
                            self.filename = name;
                            self.total = size;
                            self.recv_prompt_emitted = false;
                            // run() loops back to should_prompt_for_file →
                            // emits NeedSavePath.
                        }
                    }
                    zmodem2::Event::FileCompleted => {
                        if let Some(f) = self.file.take() {
                            drop(f);
                        }
                        let saved = self.saved_to.clone();
                        let fname = std::mem::take(&mut self.filename);
                        let role = self.role.unwrap_or(ZmRole::ReceiveFromRemote);
                        self.events.push(ZmEvent::Completed {
                            filename: fname,
                            role,
                            saved_to: saved,
                        });
                        self.saved_to = None;
                        // Sender: request session end. In ReadyForFile state
                        // finish() queues ZFIN right away; the session is NOT
                        // done yet — the remote must answer ZFIN, after which
                        // zmodem2 queues "OO" and reports SessionCompleted.
                        // Marking done here would strand the remote `rz`
                        // waiting for "OO" with the tty stuck in raw mode
                        // (terminal appears unable to accept any input).
                        if let Some(s) = &mut self.sender {
                            if let Err(e) = s.finish() {
                                log::error!("zmodem finish() failed: {e}");
                            }
                        }
                    }
                    zmodem2::Event::SessionCompleted => {
                        // Flush the final wire bytes BEFORE marking done:
                        // poll() returns this event ahead of WriteWire, so
                        // the sender's "OO" (or the receiver's ZFIN reply)
                        // is still queued inside the state machine. Skipping
                        // this flush leaves the remote rz/sz hanging on the
                        // closing handshake.
                        while !self.done && self.poll_once() {}
                        if self.done {
                            // The flush surfaced an abort/error — don't
                            // overwrite it with a Completed event.
                            return true;
                        }
                        if let Some(f) = self.file.take() {
                            drop(f);
                        }
                        if !matches!(self.events.last(), Some(ZmEvent::Completed { .. })) {
                            let saved = self.saved_to.clone();
                            self.events.push(ZmEvent::Completed {
                                filename: std::mem::take(&mut self.filename),
                                role: self.role.unwrap_or(ZmRole::ReceiveFromRemote),
                                saved_to: saved,
                            });
                        }
                        self.done = true;
                    }
                    zmodem2::Event::Aborted => {
                        self.cancel("传输被对手中止");
                    }
                    _ => {
                        // Future/unknown event: ignore.
                    }
                }
                true
            }
            zmodem2::Action::Idle => false,
            _ => {
                // Future/unknown action: stop processing.
                false
            }
        }
    }

    /// True when the engine is paused waiting on a file path and the prompt
    /// event hasn't been emitted yet.
    fn should_prompt_for_file(&self) -> bool {
        if self.done || self.file.is_some() {
            return false;
        }
        match self.role {
            Some(ZmRole::ReceiveFromRemote) => {
                // Need to prompt after FileStarted set the filename.
                !self.filename.is_empty() && !self.recv_prompt_emitted
            }
            // For the sender side, we prompt once (before start_file) so the
            // user can choose the file. After the dialog reply the sender's
            // prompt is marked emitted and start_file queues ZFILE; the only
            // remaining work is responding to ZRPOS ReadFile requests which
            // is handled in the run loop, not via prompt.
            Some(ZmRole::SendToRemote) => !self.send_prompt_emitted,
            None => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests — loopback against zmodem2's own peer state machines, which stand in
// for the remote lrzsz `rz`/`sz` processes.
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Upload path (remote `rz`, we send). Regression test for the terminal
    /// freeze after upload: the session must NOT finish right after sending
    /// ZFIN — it has to wait for the remote's ZFIN reply, emit "OO", and only
    /// then complete, otherwise the remote rz hangs with the tty in raw mode.
    #[test]
    fn upload_runs_closing_handshake_to_completion() {
        // Remote stand-in for `rz`.
        let mut remote = zmodem2::Receiver::new().unwrap();
        // rz announces itself with a ZRINIT frame ("01" hex type digits).
        let zrinit = match remote.poll() {
            zmodem2::Action::WriteWire(b) => b.to_vec(),
            _ => panic!("receiver must start by emitting ZRINIT"),
        };
        remote.wire_written(zrinit.len());

        let mut session = ZmodemSession::new();
        session.feed(&zrinit);
        assert_eq!(session.role(), Some(ZmRole::SendToRemote));
        let events = session.pop_events();
        assert!(
            events.iter().any(|e| matches!(e, ZmEvent::NeedFileToSend)),
            "session must ask the UI for a file to send"
        );

        // Hand over a temp file as the upload source.
        let mut path = std::env::temp_dir();
        path.push(format!("verve-zm-up-{}.bin", std::process::id()));
        let payload: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        File::create(&path).unwrap().write_all(&payload).unwrap();
        session.provide_file_to_send(Some(path.clone()));
        session.resume();

        let mut received: Vec<u8> = Vec::new();
        let mut remote_done = false;
        let mut to_remote: Vec<u8> = Vec::new();
        for _ in 0..100_000 {
            to_remote.extend_from_slice(&session.drain_outgoing());
            while !to_remote.is_empty() {
                match remote.submit_wire(&to_remote) {
                    Ok(0) => break, // blocked — drain below, retry next round
                    Ok(n) => {
                        to_remote.drain(..n);
                    }
                    Err(e) => panic!("remote protocol error: {e}"),
                }
            }
            loop {
                match remote.poll() {
                    zmodem2::Action::WriteWire(b) => {
                        let bytes = b.to_vec();
                        remote.wire_written(bytes.len());
                        session.feed(&bytes);
                    }
                    zmodem2::Action::WriteFile(chunk) => {
                        received.extend_from_slice(chunk);
                        let n = chunk.len();
                        remote.file_written(n).unwrap();
                    }
                    zmodem2::Action::Event(zmodem2::Event::SessionCompleted) => {
                        remote_done = true;
                    }
                    zmodem2::Action::Event(_) => {}
                    _ => break,
                }
            }
            if session.is_done() {
                break;
            }
        }

        let _ = std::fs::remove_file(&path);
        assert!(session.is_done(), "session must run to completion");
        assert!(
            !session.was_cancelled(),
            "clean completion must not be flagged as cancelled (the pump \
             relies on this to skip the trigger-detection grace window so a \
             second `rz` is detected immediately)"
        );
        assert!(
            remote_done,
            "remote rz must receive the full closing handshake (ZFIN + OO)"
        );
        assert_eq!(received, payload, "file content must survive the upload");
        let events = session.pop_events();
        assert!(
            events
                .iter()
                .any(|e| matches!(e, ZmEvent::Completed { .. })),
            "UI must see a Completed event"
        );
    }

    /// Download path (remote `sz`, we receive). The remote sender announces
    /// itself with ZRQINIT ("00" hex type digits) — the session must resolve
    /// to ReceiveFromRemote, and at the end our ZFIN reply must actually go
    /// out on the wire (it is queued behind the SessionCompleted event).
    #[test]
    fn download_resolves_role_and_completes() {
        // Remote stand-in for `sz`.
        let mut remote = zmodem2::Sender::new().unwrap();
        let zrqinit = match remote.poll() {
            zmodem2::Action::WriteWire(b) => b.to_vec(),
            _ => panic!("sender must start by emitting ZRQINIT"),
        };
        remote.wire_written(zrqinit.len());

        let mut session = ZmodemSession::new();
        session.feed(&zrqinit);
        assert_eq!(session.role(), Some(ZmRole::ReceiveFromRemote));

        let payload: Vec<u8> = (0..12_345u32).map(|i| (i % 233) as u8).collect();
        remote
            .start_file(zmodem2::FileInfo::new(
                b"download.bin",
                Some(zmodem2::Position::new(payload.len() as u32)),
            ))
            .unwrap();

        let mut save_path = std::env::temp_dir();
        save_path.push(format!("verve-zm-down-{}.bin", std::process::id()));
        let _ = std::fs::remove_file(&save_path);

        let mut remote_done = false;
        let mut to_session: Vec<u8> = Vec::new();
        for _ in 0..100_000 {
            // Answer the save-path prompt as soon as it appears.
            for ev in session.pop_events() {
                if let ZmEvent::NeedSavePath { .. } = ev {
                    session.provide_save_path(Some(save_path.clone()));
                    session.resume();
                }
            }
            // remote -> session
            loop {
                match remote.poll() {
                    zmodem2::Action::WriteWire(b) => {
                        to_session.extend_from_slice(b);
                        let n = b.len();
                        remote.wire_written(n);
                    }
                    zmodem2::Action::ReadFile { offset, max_len } => {
                        let off = offset.get() as usize;
                        let end = (off + max_len).min(payload.len());
                        remote.submit_file(&payload[off..end]).unwrap();
                    }
                    zmodem2::Action::Event(zmodem2::Event::FileCompleted) => {
                        // sz finishes the session after its last file.
                        remote.finish().unwrap();
                    }
                    zmodem2::Action::Event(zmodem2::Event::SessionCompleted) => {
                        remote_done = true;
                    }
                    zmodem2::Action::Event(_) => {}
                    _ => break,
                }
            }
            if !to_session.is_empty() {
                let data = std::mem::take(&mut to_session);
                session.feed(&data);
            }
            // session -> remote (our ZRINIT/ZRPOS/ZFIN replies)
            let out = session.drain_outgoing();
            if !out.is_empty() {
                let mut slice = &out[..];
                while !slice.is_empty() {
                    match remote.submit_wire(slice) {
                        Ok(0) => break,
                        Ok(n) => slice = &slice[n..],
                        Err(e) => panic!("remote sender protocol error: {e}"),
                    }
                }
            }
            if session.is_done() && remote_done {
                break;
            }
        }

        assert!(session.is_done(), "download session must complete");
        assert!(!session.was_cancelled(), "clean download is not a cancel");
        assert!(
            remote_done,
            "remote sz must see our ZFIN reply and finish (SessionCompleted)"
        );
        let on_disk = std::fs::read(&save_path).unwrap();
        let _ = std::fs::remove_file(&save_path);
        assert_eq!(on_disk, payload, "downloaded content must match");
    }

    /// Cancelling via the dialog reply must flag the session so the pump
    /// applies a short trigger-detection grace window (against lrzsz
    /// retransmits), while clean completions get no grace.
    #[test]
    fn cancel_marks_was_cancelled() {
        let mut remote = zmodem2::Receiver::new().unwrap();
        let zrinit = match remote.poll() {
            zmodem2::Action::WriteWire(b) => b.to_vec(),
            _ => panic!("receiver must start by emitting ZRINIT"),
        };
        remote.wire_written(zrinit.len());

        let mut session = ZmodemSession::new();
        session.feed(&zrinit);
        assert_eq!(session.role(), Some(ZmRole::SendToRemote));

        // User dismisses the open-file dialog.
        session.provide_file_to_send(None);
        assert!(session.is_done());
        assert!(session.was_cancelled());
        // The canonical CAN*8 + BS abort sequence must be on the wire.
        let out = session.drain_outgoing();
        assert!(
            out.windows(9)
                .any(|w| w == [0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x18, 0x08]),
            "abort sequence must be emitted, got {out:?}"
        );
    }
}
