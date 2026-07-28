//! Builtin auto-clustering rules (PRD §4) — three fixed rule sets.
//!
//! A [`CategoryRule`] carries `~/`-prefixed path patterns. [`scanner`] expands
//! them, checks which exist on disk, and assigns each real directory to the
//! single highest-priority matching group. No sandbox/cache-removal rules are
//! present here — those are filtered out by [`crate::blacklist`] before the
//! scanner ever sees them (PRD §4 title: "无沙盒规则").

use super::types::{CategoryRule, RiskLevel};

/// Group 1 — developer-tool caches (Safe).
///
/// Xcode/Android simulators, build caches, and package-manager caches. All
/// safe to move to a fast external SSD; an HDD would slow compile/simulator
/// launch but not break anything. Patterns with a trailing `*` (e.g.
/// `~/Library/Caches/*`) match every top-level child of that directory.
pub fn dev_caches() -> CategoryRule {
    CategoryRule {
        group_name: "开发工具缓存".to_string(),
        description: "Xcode/Android 模拟器、编译缓存、包管理器缓存，可安全迁移到高速外置固态。"
            .to_string(),
        path_patterns: vec![
            "~/.cargo".to_string(),
            "~/.rustup".to_string(),
            "~/.npm".to_string(),
            "~/Library/pnpm".to_string(),
            "~/.m2".to_string(),
            "~/.gradle".to_string(),
            "~/.cache".to_string(),
            "~/Library/Caches/*".to_string(),
            "~/Library/Developer/Xcode/DerivedData".to_string(),
            "~/Library/Developer/Xcode/Archives".to_string(),
            "~/Library/Developer/CoreSimulator".to_string(),
            "~/Library/Caches/com.apple.dt.Xcode".to_string(),
            "~/Library/Android".to_string(),
            "~/.android".to_string(),
            "~/Library/Caches/AndroidEmulator".to_string(),
            "~/.flutter".to_string(),
            "~/.dart-tool".to_string(),
            "~/.cocoapods".to_string(),
        ],
        priority: 10,
        default_risk: RiskLevel::Safe,
        hint_text: "🟢 安全 | 高速外置固态推荐迁移；机械硬盘会显著降低编译、模拟器启动速度"
            .to_string(),
    }
}

/// Group 2 — local code-project directories (PerformanceWarn).
///
/// Only recommended for Thunderbolt/USB4 SSD relocation; an HDD makes compile
/// and code-indexing painfully slow.
pub fn code_projects() -> CategoryRule {
    CategoryRule {
        group_name: "本地代码项目目录".to_string(),
        description: "开发工程根目录，建议仅迁移至雷电/USB4 外置 SSD。".to_string(),
        path_patterns: vec![
            "~/CodeProjects".to_string(),
            "~/RustroverProjects".to_string(),
            "~/PycharmProjects".to_string(),
        ],
        priority: 8,
        default_risk: RiskLevel::PerformanceWarn,
        hint_text: "⚠️ 性能敏感目录，仅建议雷电/USB4 外置 SSD 迁移；机械硬盘编译、代码索引卡顿明显"
            .to_string(),
    }
}

/// Group 3 — downloads / documents / media (Safe).
///
/// Low read/write pressure — safe on either SSD or HDD.
pub fn docs_media() -> CategoryRule {
    CategoryRule {
        group_name: "下载/文档/媒体文件".to_string(),
        description: "常规文件目录，读写压力低，固态/机械硬盘均可迁移。".to_string(),
        path_patterns: vec![
            "~/Downloads".to_string(),
            "~/Documents".to_string(),
            "~/Movies".to_string(),
            "~/Pictures".to_string(),
        ],
        priority: 5,
        default_risk: RiskLevel::Safe,
        hint_text: "🟢 安全 | 文件读写压力低，固态/机械硬盘均可迁移".to_string(),
    }
}

/// All builtin rules in priority order (highest first), as the scanner
/// expects them (PRD §5.3 — one directory joins exactly one group).
pub fn builtin_rules() -> Vec<CategoryRule> {
    vec![dev_caches(), code_projects(), docs_media()]
}

/// Group name reserved for user-added custom directories (PRD §6).
pub const CUSTOM_GROUP: &str = "用户自定义目录";

/// Group name used for non-empty home directories matched by no rule
/// (PRD §5.7 — 【其他目录】).
pub const OTHER_GROUP: &str = "其他目录";
