//! Windows input-source switching via user32 (no extra crates).
//!
//! `ActivateKeyboardLayout` switches the CALLING THREAD's keyboard layout —
//! i.e. only Verve's focused window, leaving other apps' input state alone —
//! away from the Chinese IME when a terminal session opens. Needs no
//! permission. Must be called from the UI thread that owns the focused
//! window (our call sites run inside GPUI update contexts, which qualifies).

type HKL = usize;

#[link(name = "user32")]
unsafe extern "C" {
    fn GetKeyboardLayoutList(nBuff: i32, lpList: *mut HKL) -> i32;
    fn ActivateKeyboardLayout(idLanguage: HKL, flags: u32) -> HKL;
}

/// The low word of an HKL is the LANGID; its low 10 bits are the primary
/// language (0x09 = English). 0x0409 is en-US.
const LANG_ENGLISH: u16 = 0x09;
const LANG_ENGLISH_US: u16 = 0x0409;

fn lang_id(hkl: HKL) -> u16 {
    (hkl & 0xFFFF) as u16
}

fn primary_language(hkl: HKL) -> u16 {
    lang_id(hkl) & 0x3FF
}

/// Activate an English keyboard layout for this thread. Returns a
/// description of the chosen layout for logging.
pub fn select_english_keyboard_layout() -> Result<String, String> {
    // SAFETY: plain win32 calls; the buffer is sized from the count reported
    // by the first GetKeyboardLayoutList invocation.
    unsafe {
        let count = GetKeyboardLayoutList(0, std::ptr::null_mut());
        if count <= 0 {
            return Err(format!("GetKeyboardLayoutList 报告布局数 {count}"));
        }
        let mut layouts = vec![0 as HKL; count as usize];
        let got = GetKeyboardLayoutList(count, layouts.as_mut_ptr());
        if got <= 0 {
            return Err(format!("GetKeyboardLayoutList 获取失败({got})"));
        }
        let usable = layouts.get(..got.max(0) as usize).unwrap_or(&layouts);
        let chosen = usable
            .iter()
            .copied()
            .find(|hkl| lang_id(*hkl) == LANG_ENGLISH_US)
            .or_else(|| {
                usable
                    .iter()
                    .copied()
                    .find(|hkl| primary_language(*hkl) == LANG_ENGLISH)
            })
            .ok_or_else(|| "系统没有已启用的英文键盘布局".to_string())?;
        // flags = 0: activate for this thread only (the default), not the
        // whole process (KLF_SETFORPROCESS) or system (KLF_REORDER).
        if ActivateKeyboardLayout(chosen, 0) == 0 {
            return Err(format!("ActivateKeyboardLayout({chosen:#x}) 失败"));
        }
        Ok(format!("HKL {chosen:#010x} (LANGID {:#06x})", lang_id(chosen)))
    }
}
