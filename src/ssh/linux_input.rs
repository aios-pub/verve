//! Linux input-method deactivation for terminal sessions.
//!
//! Linux has no unified system API for switching input sources (IBus and
//! Fcitx expose different D-Bus interfaces, and X11/Wayland differ again), so
//! we drive the standard CLI tools shipped with the IME frameworks:
//! - fcitx5 / fcitx4: `<tool>-remote -c` deactivates the IM, falling back to
//!   direct (English) input. Works on X11 and Wayland — it talks to the
//!   daemon via D-Bus, no display protocol involved.
//! - IBus: `ibus engine xkb:us::eng` switches to the English xkb engine
//!   (the standard id on IBus setups that also have Chinese engines).
//!
//! With no IME framework installed (plain XIM / no Chinese input) there is
//! nothing to switch; this returns Err and the caller logs and ignores it.

/// Deactivate the input method / switch to English input. Returns a
/// description of the mechanism used, for logging.
pub fn select_english_keyboard_layout() -> Result<String, String> {
    // Fcitx5 first, then legacy Fcitx4. A missing binary fails fast
    // (Command::spawn → ENOENT), costing well under a millisecond.
    for tool in ["fcitx5-remote", "fcitx-remote"] {
        if let Ok(output) = std::process::Command::new(tool).arg("-c").output() {
            if output.status.success() {
                return Ok(format!("{tool} -c"));
            }
        }
    }
    if let Ok(output) = std::process::Command::new("ibus")
        .args(["engine", "xkb:us::eng"])
        .output()
    {
        if output.status.success() {
            return Ok("ibus engine xkb:us::eng".to_string());
        }
    }
    Err("未检测到 fcitx5/fcitx/ibus(或切换未成功)".to_string())
}
