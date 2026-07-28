//! External-disk pre-check (PRD §7) — run before any migration starts.
//!
//! Resolves, for a chosen external target root:
//! - whether it is an *external* volume (internal drives are rejected),
//! - the filesystem kind (APFS / exFAT / Other — exFAT gets a permissions
//!   warning, APFS is recommended),
//! - the storage medium (SSD / HDD / Unknown — HDD gets a hard warning),
//! - free + total bytes (free must cover the total migration size).
//!
//! macOS uses `diskutil info -plist` + `statvfs`; non-macOS falls back to
//! `statvfs` only and reports `is_external = Unknown` / `medium = Unknown`,
//! so the UI can still proceed on Linux/Windows without the OS-specific probes.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Filesystem family (PRD §7.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum FsKind {
    Apfs,
    Exfat,
    Other,
}

impl FsKind {
    pub fn label(self) -> &'static str {
        match self {
            FsKind::Apfs => "APFS",
            FsKind::Exfat => "exFAT",
            FsKind::Other => "其他",
        }
    }
}

/// Storage medium (PRD §7.1, §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Medium {
    Ssd,
    Hdd,
    Unknown,
}

impl Medium {
    pub fn label(self) -> &'static str {
        match self {
            Medium::Ssd => "SSD",
            Medium::Hdd => "HDD（机械硬盘）",
            Medium::Unknown => "未知",
        }
    }
}

/// Whether the target volume is internal or external.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum Internality {
    External,
    Internal,
    Unknown,
}

/// Full probe outcome for a chosen target root.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProbeResult {
    pub target_root: String,
    pub internality: Internality,
    pub fs_kind: FsKind,
    pub medium: Medium,
    pub free_bytes: u64,
    pub total_bytes: u64,
    pub volume_name: Option<String>,
}

/// Per-PRD §7.2, the recommendation level for the chosen disk. Drives which
/// dialog the UI shows (light hint / hard warning / reject).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recommendation {
    /// Thunderbolt/USB4 SSD — light hint, proceed directly.
    SsdOk,
    /// HDD — hard warning, user must confirm before continuing.
    HddWarn,
    /// exFAT — permissions/attributes warning, APFS recommended.
    ExfatWarn,
    /// Space insufficient — hard reject.
    InsufficientSpace,
    /// Internal volume — hard reject (PRD §7.1).
    InternalRejected,
    /// Free/medium unknown — proceed but note the uncertainty.
    UnknownProceed,
}

impl ProbeResult {
    /// The PRD §7.2 recommendation derived from this probe.
    pub fn recommendation(&self, required_bytes: u64) -> Recommendation {
        if self.internality == Internality::Internal {
            return Recommendation::InternalRejected;
        }
        if self.free_bytes < required_bytes {
            return Recommendation::InsufficientSpace;
        }
        if self.medium == Medium::Hdd {
            return Recommendation::HddWarn;
        }
        if self.fs_kind == FsKind::Exfat {
            return Recommendation::ExfatWarn;
        }
        if self.medium == Medium::Ssd {
            Recommendation::SsdOk
        } else {
            Recommendation::UnknownProceed
        }
    }
}

/// Probe a chosen target root. Returns `Err` only on catastrophic failure
/// (path unreadable); an unreadable `diskutil`/`statvfs` degrades gracefully
/// into an `Unknown`/zero result so the UI can still report something.
pub fn probe(target_root: &Path) -> Result<ProbeResult, String> {
    // Must exist and be a directory.
    let md = std::fs::metadata(target_root)
        .map_err(|e| format!("无法读取目标目录 {}: {}", target_root.display(), e))?;
    if !md.is_dir() {
        return Err(format!("目标不是目录：{}", target_root.display()));
    }

    let (free_bytes, total_bytes) = statvfs(target_root).unwrap_or((0, 0));

    // OS-specific externality / medium / fs detection.
    #[cfg(target_os = "macos")]
    let (internality, fs_kind, medium, volume_name) = probe_macos(target_root, free_bytes);
    #[cfg(not(target_os = "macos"))]
    let (internality, fs_kind, medium, volume_name) = probe_other(target_root);

    Ok(ProbeResult {
        target_root: target_root.to_string_lossy().into_owned(),
        internality,
        fs_kind,
        medium,
        free_bytes,
        total_bytes,
        volume_name,
    })
}

/// POSIX free/total bytes via `statvfs`. Cross-platform (macOS/Linux).
fn statvfs(path: &Path) -> Option<(u64, u64)> {
    // SAFETY: statvfs takes a C string path and writes into a statvfs struct.
    // We only read the two fields we care about; the rest are left
    // uninitialised by us and ignored.
    #[cfg(unix)]
    unsafe {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt as _;

        let c_path = CString::new(path.as_os_str().as_bytes()).ok()?;
        let mut buf: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(c_path.as_ptr(), &mut buf) != 0 {
            return None;
        }
        let free = buf.f_bavail as u64 * buf.f_frsize as u64;
        let total = buf.f_blocks as u64 * buf.f_frsize as u64;
        Some((free, total))
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        None
    }
}

// ---------------------------------------------------------------------------
// macOS-specific probing.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn probe_macos(target_root: &Path, _free_bytes: u64) -> (Internality, FsKind, Medium, Option<String>) {
    // Resolve the target to a /Volumes mount and read diskutil info for it.
    let mount = resolve_mount(target_root);
    let plist = match mount.as_deref().and_then(run_diskutil) {
        Some(p) => p,
        None => {
            return (
                Internality::Unknown,
                FsKind::Other,
                Medium::Unknown,
                None,
            )
        }
    };
    let info = parse_diskutil_plist(&plist);

    // Internal? diskutil reports `Internal = true/false`.
    let internality = match info.internal {
        Some(true) => Internality::Internal,
        Some(false) => Internality::External,
        None => Internality::Unknown,
    };

    // Filesystem: match the `FilesystemType` / `Content` strings.
    let fs_kind = match info
        .filesystem_type
        .as_deref()
        .or(info.content.as_deref())
    {
        Some(s) => classify_fs(s),
        None => FsKind::Other,
    };

    // Medium: `SolidState` (bool) on modern macOS; fall back to
    // `RotationalRate == 0 → SSD`, else HDD.
    let medium = if info.solid_state == Some(true) || info.rotational_rate == Some(0) {
        Medium::Ssd
    } else if info.rotational_rate.unwrap_or(0) > 0 {
        Medium::Hdd
    } else if info.solid_state == Some(false) {
        Medium::Hdd
    } else {
        Medium::Unknown
    };

    (internality, fs_kind, medium, info.volume_name)
}

#[cfg(target_os = "macos")]
fn resolve_mount(target: &Path) -> Option<PathBuf> {
    // Walk up from the target to find the nearest /Volumes ancestor; if the
    // target itself is under /Volumes/<vol>/... we probe that volume.
    let canon = std::fs::canonicalize(target).unwrap_or_else(|_| target.to_path_buf());
    let mut cur: Option<&Path> = Some(&canon);
    while let Some(p) = cur {
        if p.starts_with("/Volumes") {
            // The volume root is /Volumes/<name>.
            let mut comps = p.components();
            let _root = comps.next(); // /
            let _volumes = comps.next(); // Volumes
            let name = comps.next(); // <vol name>
            match name {
                Some(n) if comps.next().is_none() => return Some(Path::new("/Volumes").join(n)),
                _ => return Some(p.to_path_buf()),
            }
        }
        cur = p.parent();
    }
    None
}

#[cfg(target_os = "macos")]
fn run_diskutil(mount: &Path) -> Option<String> {
    let out = std::process::Command::new("diskutil")
        .args(["info", "-plist"])
        .arg(mount)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    String::from_utf8(out.stdout).ok()
}

#[cfg(target_os = "macos")]
#[derive(Default)]
struct DiskutilInfo {
    internal: Option<bool>,
    solid_state: Option<bool>,
    rotational_rate: Option<u64>,
    filesystem_type: Option<String>,
    content: Option<String>,
    volume_name: Option<String>,
}

/// Parse the small subset of `diskutil info -plist` fields we need. The plist
/// is XML with pairs `<key>Name</key><type>Value</type>`. We extract each key
/// we care about followed by the value of its immediately-following typed tag
/// (`<true/>`, `<false/>`, `<integer>..</integer>`, `<string>..</string>`).
/// A regex scan is simpler and more robust than a hand-rolled tokenizer.
#[cfg(target_os = "macos")]
fn parse_diskutil_plist(xml: &str) -> DiskutilInfo {
    use regex::Regex;
    use std::sync::OnceLock;

    static BOOL_RE: OnceLock<Regex> = OnceLock::new();
    static INT_RE: OnceLock<Regex> = OnceLock::new();
    static STR_RE: OnceLock<Regex> = OnceLock::new();
    let bool_re = BOOL_RE.get_or_init(|| {
        Regex::new(r"(?s)<key>([^<]+)</key>\s*<(true|false)/>").unwrap()
    });
    let int_re = INT_RE.get_or_init(|| {
        Regex::new(r"(?s)<key>([^<]+)</key>\s*<integer>(\d+)</integer>").unwrap()
    });
    let str_re = STR_RE.get_or_init(|| {
        Regex::new(r"(?s)<key>([^<]+)</key>\s*<string>([^<]*)</string>").unwrap()
    });

    let mut info = DiskutilInfo::default();
    for c in bool_re.captures_iter(xml) {
        let key = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let val = c.get(2).map(|m| m.as_str()) == Some("true");
        match key {
            "Internal" => info.internal = Some(val),
            "SolidState" => info.solid_state = Some(val),
            _ => {}
        }
    }
    for c in int_re.captures_iter(xml) {
        let key = c.get(1).map(|m| m.as_str()).unwrap_or("");
        if key == "RotationalRate" {
            info.rotational_rate = c.get(2).and_then(|m| m.as_str().parse().ok());
        }
    }
    for c in str_re.captures_iter(xml) {
        let key = c.get(1).map(|m| m.as_str()).unwrap_or("");
        let val = c.get(2).map(|m| m.as_str().to_string()).unwrap_or_default();
        match key {
            "FilesystemType" => info.filesystem_type = Some(val),
            "Content" => info.content = Some(val),
            "VolumeName" => info.volume_name = Some(val),
            _ => {}
        }
    }
    info
}

#[cfg(target_os = "macos")]
fn classify_fs(raw: &str) -> FsKind {
    let upper = raw.to_ascii_uppercase();
    if upper.contains("APFS") {
        FsKind::Apfs
    } else if upper.contains("EXFAT") {
        FsKind::Exfat
    } else {
        FsKind::Other
    }
}

// ---------------------------------------------------------------------------
// Non-macOS fallback.
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "macos"))]
fn probe_other(_target_root: &Path) -> (Internality, FsKind, Medium, Option<String>) {
    // Linux/Windows: we can't reliably tell internal vs external without a
    // mount-table walk; report Unknown and let the user confirm. The UI still
    // shows free-space checks and lets migration proceed.
    (
        Internality::Unknown,
        FsKind::Other,
        Medium::Unknown,
        None,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommendation_matrix() {
        let ok = ProbeResult {
            target_root: "/Volumes/Fast".into(),
            internality: Internality::External,
            fs_kind: FsKind::Apfs,
            medium: Medium::Ssd,
            free_bytes: 1_000_000_000_000,
            total_bytes: 2_000_000_000_000,
            volume_name: Some("Fast".into()),
        };
        assert_eq!(ok.recommendation(100), Recommendation::SsdOk);
        assert_eq!(
            ok.recommendation(2_000_000_000_000),
            Recommendation::InsufficientSpace
        );

        let hdd = ProbeResult {
            medium: Medium::Hdd,
            ..ok.clone()
        };
        assert_eq!(hdd.recommendation(100), Recommendation::HddWarn);

        let exfat = ProbeResult {
            fs_kind: FsKind::Exfat,
            medium: Medium::Ssd,
            ..ok.clone()
        };
        assert_eq!(exfat.recommendation(100), Recommendation::ExfatWarn);

        let internal = ProbeResult {
            internality: Internality::Internal,
            ..ok.clone()
        };
        assert_eq!(
            internal.recommendation(100),
            Recommendation::InternalRejected
        );
    }
}
