//! macOS input-source switching via the Carbon TIS API.
//!
//! Used to reset the keyboard to an English hardware layout when a terminal
//! session opens, so keystrokes reach the shell instead of being swallowed by
//! the Chinese IME. `TISSelectInputSource` needs no Accessibility permission
//! and runs synchronously (unlike simulating the 英数 key via System Events,
//! which requires assistive access and has launch latency).

use std::ffi::c_void;

type CFTypeRef = *const c_void;
type CFStringRef = CFTypeRef;
type CFArrayRef = CFTypeRef;
type CFDictionaryRef = CFTypeRef;
type CFBooleanRef = CFTypeRef;
type TISInputSourceRef = CFTypeRef;
type OSStatus = i32;

#[link(name = "Carbon", kind = "framework")]
unsafe extern "C" {
    static kTISPropertyInputSourceID: CFStringRef;
    static kTISPropertyInputSourceIsEnableCapable: CFStringRef;
    static kTISPropertyInputSourceType: CFStringRef;
    static kTISTypeKeyboardLayout: CFStringRef;

    fn TISGetInputSourceProperty(
        inputSource: TISInputSourceRef,
        propertyKey: CFStringRef,
    ) -> CFTypeRef;
    fn TISCreateInputSourceList(
        properties: CFDictionaryRef,
        includeAllInstalled: u8,
    ) -> CFArrayRef;
    fn TISSelectInputSource(inputSource: TISInputSourceRef) -> OSStatus;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    static kCFBooleanTrue: CFBooleanRef;
    fn CFDictionaryCreate(
        allocator: *const c_void,
        keys: *const CFTypeRef,
        values: *const CFTypeRef,
        numValues: isize,
        keyCallBacks: *const c_void,
        valueCallBacks: *const c_void,
    ) -> CFDictionaryRef;
    fn CFArrayGetCount(array: CFArrayRef) -> isize;
    fn CFArrayGetValueAtIndex(array: CFArrayRef, idx: isize) -> CFTypeRef;
    fn CFBooleanGetValue(boolean: CFBooleanRef) -> bool;
    fn CFStringGetCString(
        s: CFStringRef,
        buffer: *mut u8,
        bufferSize: isize,
        encoding: u32,
    ) -> bool;
    fn CFRelease(cf: CFTypeRef);
}

const KCF_STRING_ENCODING_UTF8: u32 = 0x0800_0100;

/// Preferred ASCII keyboard-layout source IDs, best first.
const PREFERRED_IDS: [&str; 3] = [
    "com.apple.keylayout.ABC",
    "com.apple.keylayout.US",
    "com.apple.keylayout.USInternational-PC",
];

/// Read a source's `InputSourceID` property as an owned String.
///
/// # Safety
/// `source` must be a valid `TISInputSourceRef`.
unsafe fn source_id(source: TISInputSourceRef) -> Option<String> {
    // SAFETY: `source` is valid per the function contract; the property key
    // is a framework constant.
    let id_str = unsafe { TISGetInputSourceProperty(source, kTISPropertyInputSourceID) };
    if id_str.is_null() {
        return None;
    }
    let mut buf = [0u8; 256];
    // SAFETY: `id_str` is a CFStringRef returned by TIS; `buf` is a valid
    // 256-byte buffer with room for the NUL terminator.
    let copied = unsafe {
        CFStringGetCString(
            id_str as CFStringRef,
            buf.as_mut_ptr(),
            buf.len() as isize,
            KCF_STRING_ENCODING_UTF8,
        )
    };
    if !copied {
        return None;
    }
    let len = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    String::from_utf8_lossy(buf.get(..len).unwrap_or(&[])).into_owned().into()
}

/// Read a boolean property off an input source (e.g. IsEnableCapable).
///
/// # Safety
/// `source` must be a valid `TISInputSourceRef`.
unsafe fn source_bool_property(source: TISInputSourceRef, key: CFStringRef) -> bool {
    // SAFETY: `source` is valid per the function contract.
    let value = unsafe { TISGetInputSourceProperty(source, key) };
    if value.is_null() {
        return false;
    }
    // SAFETY: boolean TIS properties return a CFBooleanRef.
    unsafe { CFBooleanGetValue(value as CFBooleanRef) }
}

/// Enumerate the user's ENABLED hardware keyboard layouts (excludes IMEs like
/// 简体拼音, which have a different input-source type). Returns owned IDs.
pub fn enabled_keyboard_layout_ids() -> Vec<String> {
    // SAFETY: the filter dictionary is built from valid framework constants
    // and released before returning; nothing escapes.
    unsafe {
        let keys = [
            kTISPropertyInputSourceType as CFTypeRef,
            kTISPropertyInputSourceIsEnableCapable as CFTypeRef,
        ];
        let values = [
            kTISTypeKeyboardLayout as CFTypeRef,
            kCFBooleanTrue as CFTypeRef,
        ];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            std::ptr::null(),
            std::ptr::null(),
        );
        if dict.is_null() {
            return Vec::new();
        }
        // includeAllInstalled = 0 → only sources enabled in System Settings.
        let list = TISCreateInputSourceList(dict, 0);
        CFRelease(dict);
        if list.is_null() {
            return Vec::new();
        }

        let count = CFArrayGetCount(list);
        let mut ids = Vec::new();
        for i in 0..count {
            let source = CFArrayGetValueAtIndex(list, i);
            if source.is_null() {
                continue;
            }
            if !source_bool_property(source, kTISPropertyInputSourceIsEnableCapable) {
                continue;
            }
            if let Some(id) = source_id(source) {
                ids.push(id);
            }
        }
        CFRelease(list);
        ids
    }
}

/// Select an English hardware keyboard layout (e.g. ABC), switching away from
/// any active IME. Synchronous and permission-free. Returns the selected
/// InputSourceID for logging.
pub fn select_english_keyboard_layout() -> Result<String, String> {
    // SAFETY: all refs come from a live TIS list and are used before release.
    unsafe {
        let keys = [
            kTISPropertyInputSourceType as CFTypeRef,
            kTISPropertyInputSourceIsEnableCapable as CFTypeRef,
        ];
        let values = [
            kTISTypeKeyboardLayout as CFTypeRef,
            kCFBooleanTrue as CFTypeRef,
        ];
        let dict = CFDictionaryCreate(
            std::ptr::null(),
            keys.as_ptr(),
            values.as_ptr(),
            keys.len() as isize,
            std::ptr::null(),
            std::ptr::null(),
        );
        if dict.is_null() {
            return Err("CFDictionaryCreate 返回 null".to_string());
        }
        let list = TISCreateInputSourceList(dict, 0);
        CFRelease(dict);
        if list.is_null() {
            return Err("TISCreateInputSourceList 返回 null".to_string());
        }

        let count = CFArrayGetCount(list);
        let mut sources: Vec<(TISInputSourceRef, String)> = Vec::new();
        for i in 0..count {
            let source = CFArrayGetValueAtIndex(list, i);
            if source.is_null() {
                continue;
            }
            if let Some(id) = source_id(source) {
                sources.push((source, id));
            }
        }

        // Preferred IDs first; any enabled layout is ASCII-capable anyway
        // (Chinese input lives in IME-mode sources, which the type filter
        // already excludes).
        let (source, id) = PREFERRED_IDS
            .iter()
            .find_map(|wanted| sources.iter().find(|(_, id)| id == wanted).cloned())
            .or_else(|| sources.first().cloned())
            .ok_or_else(|| "没有可用的英文键盘布局".to_string())?;

        let status = TISSelectInputSource(source);
        CFRelease(list);
        if status != 0 {
            return Err(format!("TISSelectInputSource 失败, OSStatus={status}"));
        }
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Enumeration only — no system state is changed. Every Mac has at least
    /// one enabled keyboard layout.
    #[test]
    fn has_enabled_keyboard_layouts() {
        let ids = enabled_keyboard_layout_ids();
        assert!(!ids.is_empty(), "enabled keyboard layouts: {ids:?}");
    }
}
