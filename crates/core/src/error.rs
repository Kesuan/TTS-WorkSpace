//! 错误类型与错误码契约。
//!
//! 错误码表与退出码是 CLI 契约的一部分（见 `docs/cli.md`），
//! 新增错误码时必须同步文档与 `schemas/common/envelope-error.schema.json`。

use serde::Serialize;

// ── 错误码（E_* 全大写，schema 以 ^E_[A-Z0-9_]+$ 校验）──────────────
pub const E_USAGE: &str = "E_USAGE";
pub const E_IO: &str = "E_IO";
pub const E_JSON: &str = "E_JSON";
pub const E_WORKSPACE: &str = "E_WORKSPACE";
pub const E_CONFIG_IO: &str = "E_CONFIG_IO";
pub const E_CONFIG_PARSE: &str = "E_CONFIG_PARSE";
pub const E_CONFIG_KEY: &str = "E_CONFIG_KEY";
pub const E_HTTP_NETWORK: &str = "E_HTTP_NETWORK";
pub const E_HTTP_STATUS: &str = "E_HTTP_STATUS";
pub const E_HTTP_RATE_LIMIT: &str = "E_HTTP_RATE_LIMIT";
pub const E_API_RESPONSE: &str = "E_API_RESPONSE";
pub const E_AUDIO_DECODE: &str = "E_AUDIO_DECODE";
pub const E_AUDIO_ENCODE: &str = "E_AUDIO_ENCODE";
pub const E_CACHE_IO: &str = "E_CACHE_IO";
pub const E_FFMPEG_MISSING: &str = "E_FFMPEG_MISSING";
pub const E_FFMPEG_FAILED: &str = "E_FFMPEG_FAILED";
pub const E_NOT_FOUND: &str = "E_NOT_FOUND";
pub const E_PARSE_SCRIPT: &str = "E_PARSE_SCRIPT";
pub const E_CAPABILITY: &str = "E_CAPABILITY";
pub const E_CONFIRM_REQUIRED: &str = "E_CONFIRM_REQUIRED";
pub const E_DOCTOR_UNHEALTHY: &str = "E_DOCTOR_UNHEALTHY";
pub const E_NOT_IMPLEMENTED: &str = "E_NOT_IMPLEMENTED";

/// 退出码契约：0 成功 · 1 运行时失败 · 2 用法/校验错误 · 3 部分失败 · 4 需要确认。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Runtime,
    Usage,
    Partial,
    ConfirmRequired,
}

impl ErrorKind {
    pub fn exit_code(self) -> i32 {
        match self {
            ErrorKind::Runtime => 1,
            ErrorKind::Usage => 2,
            ErrorKind::Partial => 3,
            ErrorKind::ConfirmRequired => 4,
        }
    }
}

/// 错误信封的 `error` 字段（与 `schemas/common/envelope-error.schema.json` 对应）。
#[derive(Debug, Clone, Serialize)]
pub struct ErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

/// 统一的错误类型。
#[derive(Debug, Clone)]
pub struct TtsError {
    pub kind: ErrorKind,
    pub body: ErrorBody,
}

impl TtsError {
    pub fn runtime(code: &str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Runtime, code, message)
    }

    pub fn usage(code: &str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Usage, code, message)
    }

    pub fn partial(code: &str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Partial, code, message)
    }

    pub fn confirm_required(code: &str, message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ConfirmRequired, code, message)
    }

    fn new(kind: ErrorKind, code: &str, message: impl Into<String>) -> Self {
        Self {
            kind,
            body: ErrorBody {
                code: code.to_string(),
                message: message.into(),
                hint: None,
                details: None,
            },
        }
    }

    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.body.hint = Some(hint.into());
        self
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.body.details = Some(details);
        self
    }

    pub fn exit_code(&self) -> i32 {
        self.kind.exit_code()
    }

    pub fn code(&self) -> &str {
        &self.body.code
    }

    /// 失败信封：`{"ok": false, "error": {...}}`。
    pub fn envelope(&self) -> serde_json::Value {
        serde_json::json!({ "ok": false, "error": self.body })
    }
}

impl std::fmt::Display for TtsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.body.code, self.body.message)?;
        if let Some(hint) = &self.body.hint {
            write!(f, "（提示：{hint}）")?;
        }
        Ok(())
    }
}

impl std::error::Error for TtsError {}

impl From<std::io::Error> for TtsError {
    fn from(e: std::io::Error) -> Self {
        TtsError::runtime(E_IO, e.to_string())
    }
}

impl From<serde_json::Error> for TtsError {
    fn from(e: serde_json::Error) -> Self {
        TtsError::runtime(E_JSON, e.to_string())
    }
}

pub type Result<T> = std::result::Result<T, TtsError>;

/// 成功信封：`{"ok": true, "data": {...}}`。
pub fn success_envelope(data: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "ok": true, "data": data })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_codes_follow_contract() {
        assert_eq!(ErrorKind::Runtime.exit_code(), 1);
        assert_eq!(ErrorKind::Usage.exit_code(), 2);
        assert_eq!(ErrorKind::Partial.exit_code(), 3);
        assert_eq!(ErrorKind::ConfirmRequired.exit_code(), 4);
    }

    #[test]
    fn envelope_shape_is_stable() {
        let err = TtsError::usage(E_USAGE, "未知参数 --foo")
            .with_hint("运行 tts --help")
            .with_details(serde_json::json!({ "arg": "--foo" }));
        let v = err.envelope();
        assert_eq!(v["ok"], serde_json::json!(false));
        assert_eq!(v["error"]["code"], serde_json::json!("E_USAGE"));
        assert_eq!(v["error"]["message"], serde_json::json!("未知参数 --foo"));
        assert_eq!(v["error"]["hint"], serde_json::json!("运行 tts --help"));
        assert_eq!(v["error"]["details"]["arg"], serde_json::json!("--foo"));
        assert!(v["error"].get("details").is_some());
    }

    #[test]
    fn success_envelope_shape() {
        let v = success_envelope(&serde_json::json!({ "a": 1 }));
        assert_eq!(v["ok"], serde_json::json!(true));
        assert_eq!(v["data"]["a"], serde_json::json!(1));
    }
}
