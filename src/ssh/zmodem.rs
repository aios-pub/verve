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

/// ZMODEM frame start: `*`, `*`, ZDLE (0x18), `B`. The fifth byte is `0`
/// (ZBIN/16-bit CRC) or `1` (ZBIN32/32-bit CRC).
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
    NeedSavePath { suggested_name: String },
    /// UI must show an "open file" dialog, then call
    /// [`ZmodemSession::provide_file_to_send`].
    NeedFileToSend,
    Error { message: String },
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
    /// Tracks whether we've already emitted the NeedFileToSend prompt.
    send_prompt_emitted: bool,
    /// Tracks whether we've already emitted NeedSavePath for receiver side.
    recv_prompt_emitted: bool,
}

impl ZmodemSession {
    pub fn new() -> Self {
        Self {
            role: None,
            initial_buf: Vec::new(),
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
        if self.done {
            return;
        }
        if self.role.is_none() {
            self.initial_buf.extend_from_slice(data);
            self.try_resolve_role();
            if self.role.is_none() {
                return;
            }
        }
        let initial = std::mem::take(&mut self.initial_buf);
        if !initial.is_empty() {
            self.submit_wire(&initial);
            self.run();
            if self.done {
                return;
            }
        }
        if !data.is_empty() {
            self.submit_wire(data);
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
        self.run();
    }

    fn provide_path(&mut self, path: Option<PathBuf>, for_role: ZmRole) {
        let Some(path) = path else {
            self.cancel_user();
            return;
        };
        match for_role {
            ZmRole::ReceiveFromRemote => {
                match File::create(&path) {
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
                }
            }
            ZmRole::SendToRemote => {
                match File::open(&path) {
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
                            if let Err(e) = s.start_file(info) {
                                self.cancel(&format!("start_file 失败: {e}"));
                                return;
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
                }
            }
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
    }

    fn cancel_user(&mut self) {
        self.cancel_with_event(ZmEvent::Cancelled);
    }

    /// Inspect the buffered bytes after the `**\x18B` trigger and read the
    /// ZF type byte to determine role, then instantiate the state machine.
    fn try_resolve_role(&mut self) {
        let buf = &self.initial_buf;
        let Some(trigger_pos) = buf.windows(4).position(|w| w == TRIGGER) else {
            return;
        };
        let after = &buf[trigger_pos + 4..];
        if after.len() < 2 {
            return;
        }
        let _frame_byte = after[0]; // '0' or '1'
        let zf_type = after[1];
        let role = if zf_type == 0 {
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
    }

    fn submit_wire(&mut self, mut data: &[u8]) {
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
            if consumed == 0 {
                break;
            }
            data = &data[consumed..];
        }
    }

    /// Loop: poll the state machine and handle each Action until Idle.
    /// Prompts for file paths are handled by sending an event and then
    /// blocking on a sync channel — the pump thread is a dedicated OS thread
    /// so blocking is acceptable.
    fn run(&mut self) {
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

            let action = if let Some(r) = &mut self.receiver {
                r.poll()
            } else if let Some(s) = &mut self.sender {
                s.poll()
            } else {
                return;
            };

            match action {
                zmodem2::Action::WriteWire(b) => {
                    self.outgoing.extend_from_slice(b);
                    let n = b.len();
                    if let Some(r) = &mut self.receiver {
                        r.wire_written(n);
                    } else if let Some(s) = &mut self.sender {
                        s.wire_written(n);
                    }
                }
                zmodem2::Action::WriteFile(chunk) => {
                    let Some(f) = &mut self.file else {
                        // No file open — means we need to prompt; loop back
                        // to `should_prompt_for_file`.
                        continue;
                    };
                    if let Err(e) = f.write_all(chunk) {
                        self.cancel(&format!("写入文件失败: {e}"));
                        return;
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
                            return;
                        }
                    }
                }
                zmodem2::Action::ReadFile { max_len, .. } => {
                    let Some(f) = &mut self.file else {
                        continue;
                    };
                    let mut buf = vec![0u8; max_len];
                    let n = match f.read(&mut buf) {
                        Ok(n) => n,
                        Err(e) => {
                            self.cancel(&format!("读取文件失败: {e}"));
                            return;
                        }
                    };
                    buf.truncate(n);
                    if n == 0 {
                        // EOF
                        if let Some(s) = &mut self.sender {
                            if let Err(e) = s.finish() {
                                self.cancel(&format!("finish(): {e}"));
                                return;
                            }
                        }
                    } else if let Some(s) = &mut self.sender {
                        if let Err(e) = s.submit_file(&buf) {
                            self.cancel(&format!("submit_file: {e}"));
                            return;
                        }
                        self.transferred += n as u64;
                        self.events.push(ZmEvent::Progress {
                            filename: self.filename.clone(),
                            transferred: self.transferred,
                            total: self.total,
                            role: ZmRole::SendToRemote,
                        });
                    }
                }
                zmodem2::Action::Event(ev) => match ev {
                    zmodem2::Event::FileStarted(info) => {
                        let is_receiver = self.role == Some(ZmRole::ReceiveFromRemote);
                        if is_receiver {
                            let name = String::from_utf8_lossy(info.name).to_string();
                            let size = info.size.map(|p| p.get()).unwrap_or(0) as u64;
                            self.filename = name;
                            self.total = size;
                            self.recv_prompt_emitted = false;
                            // run() loops back to should_prompt_for_file → emits NeedSavePath.
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
                        if let Some(s) = &mut self.sender {
                            let _ = s.finish();
                        }
                    }
                    zmodem2::Event::SessionCompleted => {
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
                        return;
                    }
                    zmodem2::Event::Aborted => {
                        self.cancel("传输被对手中止");
                        return;
                    }
                    _ => {
                        // Future/unknown event: ignore.
                    }
                },
                zmodem2::Action::Idle => {
                    return;
                }
                _ => {
                    // Future/unknown action: stop processing.
                    return;
                }
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
            Some(ZmRole::SendToRemote) => !self.send_prompt_emitted,
            None => false,
        }
    }
}

