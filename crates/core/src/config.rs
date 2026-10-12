//! 配置加载 / 保存 / 字段读写。
//!
//! 配置文件默认位于 `<workspace_root>/config.json`，可用 `TTS_CONFIG` 覆盖。
//! API Key 也可通过环境变量 `MIMO_API_KEY` 提供（优先级高于配置文件）；
//! base_url 可用 `MIMO_BASE_URL` 覆盖（便于测试指向本地 mock）。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, TtsError, E_CONFIG_IO, E_CONFIG_PARSE, E_USAGE};

pub const CONFIG_SCHEMA_VERSION: u32 = 1;
pub const ENV_API_KEY: &str = "MIMO_API_KEY";
pub const ENV_BASE_URL: &str = "MIMO_BASE_URL";
pub const DEFAULT_BASE_URL: &str = "https://api.xiaomimimo.com/v1";

/// 可读写的配置项（`tts config get/set` 的键）。
pub const FIELD_KEYS: &[&str] = &[
    "schema_version",
    "api.base_url",
    "api.api_key",
    "api.timeout_ms",
    "models.tts",
    "models.design",
    "models.clone",
    "generation.concurrency",
    "generation.max_chars_per_request",
    "generation.retry.max",
    "generation.retry.backoff_ms",
    "ffmpeg.path",
    "workspace_root",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub api: ApiConfig,
    pub models: ModelsConfig,
    pub generation: GenerationConfig,
    pub ffmpeg: FfmpegConfig,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_root: Option<PathBuf>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: CONFIG_SCHEMA_VERSION,
            api: ApiConfig::default(),
            models: ModelsConfig::default(),
            generation: GenerationConfig::default(),
            ffmpeg: FfmpegConfig::default(),
            workspace_root: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ApiConfig {
    pub base_url: String,
    pub api_key: Option<String>,
    pub timeout_ms: u64,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: DEFAULT_BASE_URL.to_string(),
            api_key: None,
            timeout_ms: 120_000,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ModelsConfig {
    pub tts: String,
    pub design: String,
    pub clone: String,
}

impl Default for ModelsConfig {
    fn default() -> Self {
        Self {
            tts: crate::mimo::MODEL_TTS.to_string(),
            design: crate::mimo::MODEL_DESIGN.to_string(),
            clone: crate::mimo::MODEL_CLONE.to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GenerationConfig {
    pub concurrency: u32,
    pub max_chars_per_request: u32,
    pub retry: RetryConfig,
}

impl Default for GenerationConfig {
    fn default() -> Self {
        Self {
            concurrency: 3,
            max_chars_per_request: 500,
            retry: RetryConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct RetryConfig {
    pub max: u32,
    pub backoff_ms: u64,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max: 3,
            backoff_ms: 800,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct FfmpegConfig {
    pub path: String,
}

impl Default for FfmpegConfig {
    fn default() -> Self {
        Self {
            path: "auto".to_string(),
        }
    }
}

impl Config {
    /// 读取配置；文件不存在或为空时返回默认值（不视为错误）。
    pub fn load(path: &Path) -> Result<Config> {
        if !path.exists() {
            return Ok(Config::default());
        }
        let raw = fs::read_to_string(path).map_err(|e| {
            TtsError::runtime(E_CONFIG_IO, format!("无法读取配置 {}：{e}", path.display()))
        })?;
        if raw.trim().is_empty() {
            return Ok(Config::default());
        }
        serde_json::from_str(&raw).map_err(|e| {
            TtsError::runtime(
                E_CONFIG_PARSE,
                format!("配置解析失败 {}：{e}", path.display()),
            )
            .with_hint("修复或删除该文件后重试")
        })
    }

    /// 保存配置；Unix 下权限设为 0600（含 API Key）。
    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                TtsError::runtime(
                    crate::error::E_IO,
                    format!("无法创建目录 {}：{e}", parent.display()),
                )
            })?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        fs::write(path, text).map_err(|e| {
            TtsError::runtime(E_CONFIG_IO, format!("无法写入配置 {}：{e}", path.display()))
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = fs::Permissions::from_mode(0o600);
            let _ = fs::set_permissions(path, perms);
        }
        Ok(())
    }

    /// 生效的 API Key：环境变量优先于配置文件。
    pub fn effective_api_key(&self) -> Option<String> {
        env_value(ENV_API_KEY).or_else(|| self.api.api_key.clone().filter(|s| !s.trim().is_empty()))
    }

    /// 生效的 base_url：环境变量优先，去掉末尾斜杠。
    pub fn effective_base_url(&self) -> String {
        env_value(ENV_BASE_URL)
            .unwrap_or_else(|| self.api.base_url.clone())
            .trim_end_matches('/')
            .to_string()
    }

    /// 读取单个配置项（api_key 原样返回，打码由调用方处理）。
    pub fn get_field(&self, key: &str) -> Result<serde_json::Value> {
        let value = match key {
            "schema_version" => serde_json::json!(self.schema_version),
            "api.base_url" => serde_json::json!(self.api.base_url),
            "api.api_key" => serde_json::json!(self.api.api_key),
            "api.timeout_ms" => serde_json::json!(self.api.timeout_ms),
            "models.tts" => serde_json::json!(self.models.tts),
            "models.design" => serde_json::json!(self.models.design),
            "models.clone" => serde_json::json!(self.models.clone),
            "generation.concurrency" => serde_json::json!(self.generation.concurrency),
            "generation.max_chars_per_request" => {
                serde_json::json!(self.generation.max_chars_per_request)
            }
            "generation.retry.max" => serde_json::json!(self.generation.retry.max),
            "generation.retry.backoff_ms" => serde_json::json!(self.generation.retry.backoff_ms),
            "ffmpeg.path" => serde_json::json!(self.ffmpeg.path),
            "workspace_root" => serde_json::json!(self.workspace_root),
            other => return Err(unknown_field(other)),
        };
        Ok(value)
    }

    /// 写入单个配置项（带类型与范围校验）。
    pub fn set_field(&mut self, key: &str, value: &str) -> Result<()> {
        let trimmed = value.trim();
        match key {
            "api.base_url" => {
                if trimmed.is_empty() {
                    return Err(TtsError::usage(E_USAGE, "api.base_url 不能为空"));
                }
                self.api.base_url = trimmed.trim_end_matches('/').to_string();
            }
            "api.api_key" => {
                self.api.api_key = if trimmed.is_empty() {
                    None
                } else {
                    Some(trimmed.to_string())
                };
            }
            "api.timeout_ms" => {
                self.api.timeout_ms = parse_num(key, trimmed, 1_000, 600_000)?;
            }
            "models.tts" => self.models.tts = non_empty(key, trimmed)?,
            "models.design" => self.models.design = non_empty(key, trimmed)?,
            "models.clone" => self.models.clone = non_empty(key, trimmed)?,
            "generation.concurrency" => {
                self.generation.concurrency = parse_num(key, trimmed, 1, 16)? as u32;
            }
            "generation.max_chars_per_request" => {
                self.generation.max_chars_per_request = parse_num(key, trimmed, 1, 8_000)? as u32;
            }
            "generation.retry.max" => {
                self.generation.retry.max = parse_num(key, trimmed, 0, 10)? as u32;
            }
            "generation.retry.backoff_ms" => {
                self.generation.retry.backoff_ms = parse_num(key, trimmed, 0, 60_000)?;
            }
            "ffmpeg.path" => {
                self.ffmpeg.path = if trimmed.is_empty() {
                    "auto".to_string()
                } else {
                    trimmed.to_string()
                };
            }
            "workspace_root" => {
                self.workspace_root = if trimmed.is_empty() {
                    None
                } else {
                    Some(PathBuf::from(trimmed))
                };
            }
            "schema_version" => {
                return Err(TtsError::usage(E_USAGE, "schema_version 为只读字段"));
            }
            other => return Err(unknown_field(other)),
        }
        Ok(())
    }

    /// 生成可输出的 JSON（api_key 默认打码）。
    pub fn to_json_value(&self, reveal: bool) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or(serde_json::Value::Null);
        if !reveal {
            if let (Some(key), Some(api)) = (
                self.api.api_key.as_ref(),
                v.get_mut("api").and_then(|a| a.as_object_mut()),
            ) {
                api.insert(
                    "api_key".to_string(),
                    serde_json::Value::String(mask_secret(key)),
                );
            }
        }
        v
    }
}

fn env_value(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}

fn non_empty(key: &str, value: &str) -> Result<String> {
    if value.is_empty() {
        Err(TtsError::usage(E_USAGE, format!("{key} 不能为空")))
    } else {
        Ok(value.to_string())
    }
}

fn parse_num(key: &str, value: &str, min: u64, max: u64) -> Result<u64> {
    let parsed: u64 = value.parse().map_err(|_| {
        TtsError::usage(E_USAGE, format!("{key} 需要整数，收到：{value}"))
            .with_hint(format!("取值范围 {min}~{max}"))
    })?;
    if parsed < min || parsed > max {
        return Err(TtsError::usage(
            E_USAGE,
            format!("{key} 超出范围：{parsed}（允许 {min}~{max}）"),
        )
        .with_hint(format!("取值范围 {min}~{max}")));
    }
    Ok(parsed)
}

fn unknown_field(key: &str) -> TtsError {
    TtsError::usage(E_USAGE, format!("未知配置项：{key}"))
        .with_hint(format!("可用配置项：{}", FIELD_KEYS.join(", ")))
}

/// 敏感值打码：保留前 4 与后 4 个字符。
pub fn mask_secret(secret: &str) -> String {
    let s = secret.trim();
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= 8 {
        return "****".to_string();
    }
    let head: String = chars[..4].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}****{tail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        let cfg = Config::default();
        assert_eq!(cfg.schema_version, CONFIG_SCHEMA_VERSION);
        assert_eq!(cfg.generation.concurrency, 3);
        assert_eq!(cfg.api.base_url, DEFAULT_BASE_URL);
        cfg.save(&path).unwrap();
        let loaded = Config::load(&path).unwrap();
        assert_eq!(cfg, loaded);
    }

    #[test]
    fn missing_file_returns_default() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config::load(&dir.path().join("nope.json")).unwrap();
        assert_eq!(cfg, Config::default());
    }

    #[test]
    fn invalid_json_is_config_parse_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, "{ not json").unwrap();
        let err = Config::load(&path).unwrap_err();
        assert_eq!(err.code(), E_CONFIG_PARSE);
        assert_eq!(err.exit_code(), 1);
    }

    #[test]
    fn set_and_get_fields() {
        let mut cfg = Config::default();
        cfg.set_field("generation.concurrency", "5").unwrap();
        assert_eq!(cfg.generation.concurrency, 5);
        assert_eq!(
            cfg.get_field("generation.concurrency").unwrap(),
            serde_json::json!(5)
        );

        cfg.set_field("api.api_key", "sk-abcdef123456").unwrap();
        assert_eq!(cfg.effective_api_key().as_deref(), Some("sk-abcdef123456"));

        // 清空 Key
        cfg.set_field("api.api_key", "").unwrap();
        assert!(cfg.api.api_key.is_none());
    }

    #[test]
    fn set_field_rejects_invalid_values() {
        let mut cfg = Config::default();
        for (key, value) in [
            ("generation.concurrency", "abc"),
            ("generation.concurrency", "0"),
            ("generation.concurrency", "999"),
            ("api.base_url", ""),
            ("unknown.key", "1"),
        ] {
            let err = cfg.set_field(key, value).unwrap_err();
            assert_eq!(err.code(), E_USAGE, "key={key} value={value}");
            assert_eq!(err.exit_code(), 2);
        }
        let err = cfg.set_field("schema_version", "2").unwrap_err();
        assert_eq!(err.code(), E_USAGE);
    }

    #[test]
    fn masked_json_hides_key() {
        let mut cfg = Config::default();
        cfg.set_field("api.api_key", "sk-abcdef123456").unwrap();
        let masked = cfg.to_json_value(false);
        assert_eq!(masked["api"]["api_key"], serde_json::json!("sk-a****3456"));
        let revealed = cfg.to_json_value(true);
        assert_eq!(
            revealed["api"]["api_key"],
            serde_json::json!("sk-abcdef123456")
        );
    }

    #[test]
    fn mask_secret_edge_cases() {
        assert_eq!(mask_secret(""), "****");
        assert_eq!(mask_secret("short"), "****");
        assert_eq!(mask_secret("12345678"), "****");
        assert_eq!(mask_secret("123456789"), "1234****6789");
    }
}
