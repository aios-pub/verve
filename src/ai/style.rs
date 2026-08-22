//! The thinking status-word lists shown while the agent streams.
//!
//! Usage mirrors the octos persona-service pattern: a per-turn rotator swaps
//! one word every [`STATUS_WORD_INTERVAL`] for the whole in-flight turn; the
//! pool is picked by whether the user's message contains CJK (language
//! follows what they typed, not the UI locale), and the starting index is
//! seeded from the wall clock so back-to-back turns don't repeat words.

/// English thinking-indicator words.
pub const DEFAULT_STATUS_WORDS: &[&str] = &[
    "Considering",
    "Synthesizing",
    "Pondering",
    "Reflecting",
    "Analyzing",
    "Processing",
    "Formulating",
    "Contemplating",
    "Deliberating",
    "Evaluating",
    "Composing",
    "Reasoning",
    "Assembling",
    "Connecting",
    "Weaving",
    "Distilling",
];

/// Chinese internet-slang status words.
pub const DEFAULT_STATUS_WORDS_ZH: &[&str] = &[
    "正在炼丹",
    "疯狂输出中",
    "施法中",
    "渡劫中",
    "DNA动了",
    "拿捏中",
    "在线发癫中",
    "格局打开中",
    "遥遥领先中",
    "上强度了",
    "起飞中",
    "充能中",
    "buff叠满了",
    "原地升天中",
    "开大中",
    "整活中",
    "在卷了",
    "燃起来了",
    "超级加倍中",
    "觉醒中",
];

/// How often the in-flight indicator swaps its status word (octos parity).
pub const STATUS_WORD_INTERVAL: std::time::Duration = std::time::Duration::from_secs(8);

/// Whether the text contains CJK — decides which word pool to rotate through.
pub fn prompt_has_cjk(text: &str) -> bool {
    text.chars().any(|c| {
        matches!(
            c,
            '\u{4E00}'..='\u{9FFF}' | '\u{3400}'..='\u{4DBF}' | '\u{F900}'..='\u{FAFF}'
        )
    })
}

/// Rotate a status word by index; `zh` picks the internet-slang pool.
pub fn status_word(zh: bool, idx: usize) -> &'static str {
    let words = if zh {
        DEFAULT_STATUS_WORDS_ZH
    } else {
        DEFAULT_STATUS_WORDS
    };
    words
        .get(idx % words.len())
        .copied()
        .unwrap_or(if zh { "思考中" } else { "Thinking" })
}

#[cfg(test)]
mod tests {
    use super::{prompt_has_cjk, status_word, DEFAULT_STATUS_WORDS, DEFAULT_STATUS_WORDS_ZH};

    #[test]
    fn status_words_rotate_by_pool() {
        assert_eq!(status_word(true, 0), "正在炼丹");
        assert_eq!(status_word(true, DEFAULT_STATUS_WORDS_ZH.len()), "正在炼丹");
        assert_eq!(status_word(false, 0), "Considering");
        assert_eq!(status_word(false, DEFAULT_STATUS_WORDS.len()), "Considering");
        assert!(!status_word(true, 3).is_empty());
    }

    #[test]
    fn cjk_detection() {
        assert!(prompt_has_cjk("帮我设计接口"));
        assert!(prompt_has_cjk("hello 世界"));
        assert!(!prompt_has_cjk("design an API"));
        assert!(!prompt_has_cjk(""));
    }
}
