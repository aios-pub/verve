//! AI provider configuration and persistence (`~/.verve/ai_providers.json`).
//!
//! Provider metadata (base URL / model / pricing / capability flags) lives in
//! plain JSON next to the other machine-local config files; the actual API
//! keys are stored in the encrypted vault (`crate::ssh::credentials`), the
//! same AES-256-GCM store used for SSH secrets. The JSON only records
//! `api_key_set` so the UI can show whether a key exists without ever
//! persisting the secret itself.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Token-count unit a user-entered price refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceUnit {
    Per1K,
    Per1M,
}

/// Agent write-access mode, switchable per conversation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentMode {
    /// All write operations are batched into a change plan the user applies.
    #[default]
    Plan,
    /// Write tools execute immediately (git auto-commits remain as rollback).
    FullAccess,
}

fn default_true() -> bool {
    true
}

fn default_price_unit() -> PriceUnit {
    PriceUnit::Per1M
}

fn default_max_iterations() -> usize {
    16
}

fn default_rate_retries() -> usize {
    20
}

/// One configured OpenAI-compatible provider.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiProviderConfig {
    pub id: String,
    pub name: String,
    pub base_url: String,
    /// The model used for the next request (switchable mid-conversation).
    pub model: String,
    /// All configured model ids for this provider (text / image / video
    /// models as the vendor exposes them); `model` must be one of these.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Whether the model accepts image content parts.
    #[serde(default)]
    pub supports_vision: bool,
    /// Mirrors the vault: true when a key is stored. Never the key itself.
    #[serde(default)]
    pub api_key_set: bool,
    /// User-entered price per unit for prompt tokens. `None` = no cost calc.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_price: Option<f64>,
    /// User-entered price per unit for completion tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_price: Option<f64>,
    #[serde(default = "default_price_unit")]
    pub price_unit: PriceUnit,
    /// Free-form currency label the user picks (e.g. "¥", "$", "CNY").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

impl AiProviderConfig {
    /// A new provider from a preset (fresh id, no key, no pricing — prices are
    /// deliberately never prefilled; the user sets them in the settings UI).
    pub fn from_preset(preset: &ProviderPreset) -> Self {
        Self {
            id: crate::state::models::new_id(),
            name: preset.name.to_string(),
            base_url: preset.base_url.to_string(),
            model: preset
                .models
                .first()
                .map(|m| m.to_string())
                .unwrap_or_else(|| preset.model.to_string()),
            models: preset.models.iter().map(|m| m.to_string()).collect(),
            enabled: true,
            supports_vision: preset.supports_vision,
            api_key_set: false,
            input_price: None,
            output_price: None,
            price_unit: PriceUnit::Per1M,
            currency: None,
        }
    }

    /// Cost of one exchange, or `None` when either price is unset.
    pub fn cost(&self, prompt_tokens: u64, completion_tokens: u64) -> Option<f64> {
        let input = self.input_price?;
        let output = self.output_price?;
        let unit = match self.price_unit {
            PriceUnit::Per1K => 1_000_f64,
            PriceUnit::Per1M => 1_000_000_f64,
        };
        Some(prompt_tokens as f64 * input / unit + completion_tokens as f64 * output / unit)
    }
}

/// Global AI settings (provider list + assistant behaviour).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AiSettings {
    #[serde(default)]
    pub providers: Vec<AiProviderConfig>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_provider_id: Option<String>,
    #[serde(default = "default_max_iterations")]
    pub max_agent_iterations: usize,
    /// Max retries on HTTP 429 rate limiting (exponential backoff).
    #[serde(default = "default_rate_retries")]
    pub max_rate_limit_retries: usize,
    #[serde(default)]
    pub agent_mode: AgentMode,
}

impl Default for AiSettings {
    fn default() -> Self {
        Self {
            providers: Vec::new(),
            active_provider_id: None,
            max_agent_iterations: default_max_iterations(),
            max_rate_limit_retries: default_rate_retries(),
            agent_mode: AgentMode::Plan,
        }
    }
}

impl AiSettings {
    /// The enabled provider referenced by `active_provider_id`, if valid.
    pub fn active_provider(&self) -> Option<&AiProviderConfig> {
        let id = self.active_provider_id.as_deref()?;
        self.providers
            .iter()
            .find(|p| p.id == id && p.enabled)
    }

    /// Clamp the iteration cap into the accepted 1..=100 range.
    pub fn clamped_iterations(&self) -> usize {
        self.max_agent_iterations.clamp(1, 100)
    }

    /// Clamp the 429 retry cap into the accepted 0..=100 range.
    pub fn clamped_rate_retries(&self) -> usize {
        self.max_rate_limit_retries.clamp(0, 100)
    }
}

/// A vendor preset for the "add provider" dropdown. Presets carry only the
/// endpoint and a default model — never prices or keys.
pub struct ProviderPreset {
    pub name: &'static str,
    pub base_url: &'static str,
    pub model: &'static str,
    /// Model ids offered by this vendor (first = default).
    pub models: &'static [&'static str],
    pub supports_vision: bool,
}

/// Built-in OpenAI-compatible presets.
pub fn presets() -> &'static [ProviderPreset] {
    static PRESETS: std::sync::OnceLock<Vec<ProviderPreset>> = std::sync::OnceLock::new();
    PRESETS.get_or_init(|| {
        vec![
            ProviderPreset {
                name: "Agnes AI",
                base_url: "https://apihub.agnes-ai.com/v1",
                model: "agnes-2.5-flash",
                // 文本/推理系(2.5 系列带多模态理解);图像生成(agnes-image-*)
                // 与视频(agnes-video-*)模型可在供应商设置里按需追加。
                models: &[
                    "agnes-2.5-flash",
                    "agnes-2.0-flash",
                    "agnes-2.5-pro",
                    "agnes-image-2.1-flash",
                    "agnes-video-v2.0",
                    "agnes-video-2.5",
                ],
                supports_vision: true,
            },
            ProviderPreset {
                name: "OpenAI",
                base_url: "https://api.openai.com/v1",
                model: "gpt-4o-mini",
                models: &["gpt-4o-mini", "gpt-4o", "gpt-4.1-mini"],
                supports_vision: true,
            },
            ProviderPreset {
                name: "DeepSeek",
                base_url: "https://api.deepseek.com/v1",
                model: "deepseek-chat",
                models: &["deepseek-chat", "deepseek-reasoner"],
                supports_vision: false,
            },
            ProviderPreset {
                name: "Kimi (月之暗面)",
                base_url: "https://api.moonshot.cn/v1",
                model: "kimi-latest",
                models: &["kimi-latest", "moonshot-v1-8k", "moonshot-v1-32k"],
                supports_vision: true,
            },
            ProviderPreset {
                name: "通义千问 (百炼兼容模式)",
                base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1",
                model: "qwen-plus",
                models: &["qwen-plus", "qwen-turbo", "qwen-max"],
                supports_vision: false,
            },
            ProviderPreset {
                name: "智谱 GLM",
                base_url: "https://open.bigmodel.cn/api/paas/v4",
                model: "glm-4.7-flash",
                models: &["glm-4.7-flash", "glm-4.7", "glm-4v-flash"],
                supports_vision: false,
            },
            ProviderPreset {
                name: "OpenRouter",
                base_url: "https://openrouter.ai/api/v1",
                model: "openai/gpt-4o-mini",
                models: &["openai/gpt-4o-mini"],
                supports_vision: true,
            },
            ProviderPreset {
                name: "Ollama (本地)",
                base_url: "http://localhost:11434/v1",
                model: "qwen2.5:7b",
                models: &["qwen2.5:7b"],
                supports_vision: false,
            },
            ProviderPreset {
                name: "自定义",
                base_url: "",
                model: "",
                models: &[],
                supports_vision: false,
            },
        ]
    })
}

// ---------------------------------------------------------------------------
// Persistence — `ai_providers.json` (machine-local, git-ignored).
// ---------------------------------------------------------------------------

fn ai_settings_path(dir: &Path) -> PathBuf {
    dir.join("ai_providers.json")
}

/// Load AI settings; first run or corruption yields defaults (and a fresh
/// empty file is NOT written back — saving happens on first user edit).
/// Mtime of the settings file — cheap staleness probe so UI dropdowns can
/// refresh after the (separate-window) provider dialog saves.
pub fn settings_mtime() -> Option<std::time::SystemTime> {
    let dir = crate::state::persistence::data_dir().ok()?;
    std::fs::metadata(ai_settings_path(&dir))
        .and_then(|m| m.modified())
        .ok()
}

pub fn load_ai_settings() -> AiSettings {
    let dir = match crate::state::persistence::data_dir() {
        Ok(d) => d,
        Err(e) => {
            log::error!("ai settings data_dir error: {e:?}");
            return AiSettings::default();
        }
    };
    let path = ai_settings_path(&dir);
    let mut settings = match std::fs::read_to_string(&path) {
        Ok(contents) => serde_json::from_str(&contents).unwrap_or_else(|e| {
            // Preserve the unreadable file: returning a default here would
            // get saved back over it on the next settings change, silently
            // destroying every configured provider.
            log::error!("ai_providers.json 解析失败，已备份为 .bak 并使用默认值: {e:?}");
            let _ = std::fs::rename(&path, ai_settings_path(&dir).with_extension("json.bak"));
            AiSettings::default()
        }),
        Err(_) => AiSettings::default(),
    };
    // Backward compatibility: settings written before multi-model support
    // carry only `model` — mirror it into `models` (and vice versa).
    for p in settings.providers.iter_mut() {
        if p.models.is_empty() {
            if !p.model.is_empty() {
                p.models = vec![p.model.clone()];
            }
        } else if p.model.is_empty() || !p.models.contains(&p.model) {
            let Some(first) = p.models.first().cloned() else { continue };
            p.model = first;
        }
    }
    settings
}

/// Set the active model of a provider (adds it to the list when missing).
pub fn set_active_model(provider_id: &str, model: &str) {
    let mut settings = load_ai_settings();
    let Some(p) = settings.providers.iter_mut().find(|p| p.id == provider_id) else {
        return;
    };
    if !p.models.contains(&model.to_string()) {
        p.models.push(model.to_string());
    }
    p.model = model.to_string();
    let _ = save_ai_settings(&settings);
}

/// Persist AI settings atomically (tmp + rename, like workspace.json).
pub fn save_ai_settings(settings: &AiSettings) -> anyhow::Result<()> {
    let dir = crate::state::persistence::data_dir()?;
    let path = ai_settings_path(&dir);
    let json = serde_json::to_string_pretty(settings)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, json)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// API-key vault — reuse the SSH AES-256-GCM vault with `ai` as the "host".
// ---------------------------------------------------------------------------

const VAULT_HOST: &str = "ai";

fn vault_kind(provider_id: &str) -> String {
    format!("{provider_id}:api_key")
}

/// Store (or overwrite) a provider API key in the encrypted vault.
pub fn store_provider_api_key(provider_id: &str, key: &str) {
    if let Err(e) = crate::ssh::credentials::store_secret(VAULT_HOST, &vault_kind(provider_id), key)
    {
        log::error!("store ai api key failed: {e}");
    }
}

/// Load a provider API key from the vault (`None` when never stored).
pub fn load_provider_api_key(provider_id: &str) -> Option<String> {
    crate::ssh::credentials::load_secret(VAULT_HOST, &vault_kind(provider_id))
}

/// Delete a provider API key from the vault (silently succeeds if absent).
pub fn delete_provider_api_key(provider_id: &str) {
    if let Err(e) = crate::ssh::credentials::delete_secret(VAULT_HOST, &vault_kind(provider_id)) {
        log::error!("delete ai api key failed: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::{AgentMode, AiProviderConfig, AiSettings, PriceUnit};

    fn priced() -> AiProviderConfig {
        AiProviderConfig {
            id: "p1".into(),
            name: "t".into(),
            base_url: "https://x/v1".into(),
            model: "m".into(),
            models: vec!["m".into()],
            enabled: true,
            supports_vision: false,
            api_key_set: false,
            input_price: Some(2.0),
            output_price: Some(8.0),
            price_unit: PriceUnit::Per1M,
            currency: Some("¥".into()),
        }
    }

    #[test]
    fn cost_uses_unit_and_both_prices() {
        let p = priced();
        // 1M prompt tokens * 2 + 0.5M completion * 8 = 2 + 4.
        let c = p.cost(1_000_000, 500_000);
        assert!((c.unwrap() - 6.0).abs() < 1e-9);
    }

    #[test]
    fn cost_none_when_price_missing() {
        let mut p = priced();
        p.output_price = None;
        assert!(p.cost(10, 10).is_none());
        p.output_price = Some(1.0);
        p.input_price = None;
        assert!(p.cost(10, 10).is_none());
    }

    #[test]
    fn per_1k_unit_scales() {
        let mut p = priced();
        p.price_unit = PriceUnit::Per1K;
        // 1K * 2 + 1K * 8 = 10.
        let c = p.cost(1_000, 1_000);
        assert!((c.unwrap() - 10.0).abs() < 1e-9);
    }

    #[test]
    fn serde_roundtrip_defaults_and_mode() {
        // Minimal on-disk form (all optional fields absent) must deserialize.
        let raw = r#"{"providers":[{"id":"a","name":"n","base_url":"u","model":"m"}]}"#;
        let s: AiSettings = serde_json::from_str(raw).unwrap();
        let p = s.providers.first().unwrap();
        assert!(p.enabled && !p.supports_vision);
        assert_eq!(s.agent_mode, AgentMode::Plan);
        assert_eq!(s.max_agent_iterations, 16);

        let json = serde_json::to_string(&s).unwrap();
        let back: AiSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(back.providers.len(), 1);
        assert_eq!(back.providers[0].id, "a");
    }

    #[test]
    fn active_provider_requires_enabled_match() {
        let mut s = AiSettings::default();
        assert!(s.active_provider().is_none());
        let mut p = AiProviderConfig::from_preset(&super::presets()[0]);
        p.enabled = false;
        s.active_provider_id = Some(p.id.clone());
        s.providers.push(p.clone());
        assert!(s.active_provider().is_none());
        s.providers[0].enabled = true;
        assert_eq!(s.active_provider().map(|x| x.id.as_str()), Some(p.id.as_str()));
    }

    #[test]
    fn iterations_clamped() {
        let mut s = AiSettings::default();
        s.max_agent_iterations = 0;
        assert_eq!(s.clamped_iterations(), 1);
        s.max_agent_iterations = 500;
        assert_eq!(s.clamped_iterations(), 100);
    }

    #[test]
    fn rate_retries_default_and_clamp() {
        let s = AiSettings::default();
        assert_eq!(s.clamped_rate_retries(), 20);
        let mut s2 = s.clone();
        s2.max_rate_limit_retries = 999;
        assert_eq!(s2.clamped_rate_retries(), 100);
    }

    #[test]
    fn settings_persist_and_reload_echo_providers() {
        // Isolated data dir so the test never touches the real ~/.verve.
        let dir = std::env::temp_dir().join(format!("verve-ai-cfg-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let _guard = crate::state::persistence::set_thread_data_dir(dir.clone());

        let mut s = AiSettings::default();
        let a = AiProviderConfig::from_preset(&super::presets()[0]);
        let b = AiProviderConfig::from_preset(&super::presets()[1]);
        let b_id = b.id.clone();
        s.providers.push(a);
        s.providers.push(b);
        s.active_provider_id = Some(b_id.clone());
        super::save_ai_settings(&s).unwrap();

        let reloaded = super::load_ai_settings();
        assert_eq!(reloaded.providers.len(), 2, "providers persist across reopen");
        assert_eq!(reloaded.active_provider_id.as_deref(), Some(b_id.as_str()));
        assert_eq!(
            reloaded.active_provider().map(|p| p.id.as_str()),
            Some(b_id.as_str()),
            "active selection echoes"
        );

        // Switch active provider, save, reload — echo follows the switch.
        let first_id = {
            let mut r2 = super::load_ai_settings();
            let first = r2.providers[0].id.clone();
            r2.active_provider_id = Some(first.clone());
            super::save_ai_settings(&r2).unwrap();
            first
        };
        let r3 = super::load_ai_settings();
        assert_eq!(r3.active_provider_id.as_deref(), Some(first_id.as_str()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn models_normalize_backfills_legacy() {
        // Legacy shape: model only.
        let json = r#"{"providers":[{"id":"a","name":"A","base_url":"https://a/v1","model":"gpt-x","enabled":true}],"active_provider_id":"a"}"#;
        let mut settings: AiSettings = serde_json::from_str(json).unwrap();
        settings.providers.iter_mut().for_each(|_| {});
        // 直接走 serde 之后再手动跑规范化(与 load_ai_settings 相同逻辑)
        for p in settings.providers.iter_mut() {
            if p.models.is_empty() {
                if !p.model.is_empty() {
                    p.models = vec![p.model.clone()];
                }
            } else if p.model.is_empty() || !p.models.contains(&p.model) {
                let Some(first) = p.models.first().cloned() else { continue };
                p.model = first;
            }
        }
        let p = settings.providers.first().unwrap();
        assert_eq!(p.models, vec!["gpt-x".to_string()]);
        assert_eq!(p.model, "gpt-x");
    }

    #[test]
    fn agnes_preset_is_first_with_models() {
        let presets = super::presets();
        let first = presets.first().unwrap();
        assert!(first.name.contains("Agnes"));
        assert_eq!(first.base_url, "https://apihub.agnes-ai.com/v1");
        assert!(first.models.contains(&"agnes-2.5-flash"));
        assert!(first.models.contains(&"agnes-2.5-pro"));
        assert!(first.models.contains(&"agnes-image-2.1-flash"));
        assert!(first.models.contains(&"agnes-video-2.5"));
        let from = AiProviderConfig::from_preset(first);
        assert_eq!(from.model, "agnes-2.5-flash");
        assert_eq!(from.models.len(), first.models.len());
    }

    #[test]
    fn preset_never_sets_key_or_price() {
        let p = AiProviderConfig::from_preset(&super::presets()[0]);
        assert!(!p.api_key_set);
        assert!(p.input_price.is_none());
        assert!(p.output_price.is_none());
    }
}
