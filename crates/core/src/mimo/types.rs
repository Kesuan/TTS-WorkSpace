//! MiMo API 请求/响应模型（OpenAI 兼容 chat.completions）。
//!
//! 约定（见 MiMo 文档）：
//! - 目标文本放在 `role: assistant` 的消息中；
//! - `role: user` 为可选的风格指令（voicedesign 模型时必填 = 音色描述）；
//! - 非流式 `format: wav`，流式必须 `format: pcm16`（24kHz PCM16LE 单声道）。

use serde::{Deserialize, Serialize};

use crate::error::{Result, TtsError, E_USAGE};
use crate::mimo::MODEL_DESIGN;
use crate::provider::{AudioFormat, SynthRequest, VoiceRef};

#[derive(Debug, Clone, Serialize)]
pub struct ChatRequest {
    pub model: String,
    pub messages: Vec<ChatMessage>,
    pub audio: AudioParams,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stream: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioParams {
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimize_text_preview: Option<bool>,
}

/// 由内核请求构建 API 请求体（纯函数，便于契约测试与 golden）。
pub fn build_chat_request(
    model: &str,
    request: &SynthRequest,
    stream: bool,
) -> Result<ChatRequest> {
    let text = request.text.trim();
    if text.is_empty() {
        return Err(TtsError::usage(
            E_USAGE,
            "合成文本不能为空（assistant 消息）",
        ));
    }

    let (user_content, voice) = match &request.voice {
        VoiceRef::Preset { name } => {
            if name.trim().is_empty() {
                return Err(TtsError::usage(E_USAGE, "预置音色名不能为空"));
            }
            (request.instructions.clone(), Some(name.clone()))
        }
        VoiceRef::Design { prompt } => {
            let mut p = prompt.trim().to_string();
            if p.is_empty() {
                return Err(TtsError::usage(
                    E_USAGE,
                    "音色设计描述不能为空（voicedesign 模型必需）",
                ));
            }
            if let Some(extra) = request
                .instructions
                .as_ref()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
            {
                p.push('\n');
                p.push_str(extra);
            }
            (Some(p), None)
        }
        VoiceRef::Clone { sample_data_url } => {
            if !sample_data_url.starts_with("data:") {
                return Err(TtsError::usage(
                    E_USAGE,
                    "克隆样本必须是 data URL（data:audio/...;base64,...）",
                ));
            }
            (request.instructions.clone(), Some(sample_data_url.clone()))
        }
    };

    if request.optimize_text_preview && model != MODEL_DESIGN {
        return Err(TtsError::usage(
            E_USAGE,
            "optimize_text_preview 仅支持 voicedesign 模型",
        ));
    }

    let mut messages = Vec::with_capacity(2);
    if let Some(content) = user_content.filter(|c| !c.trim().is_empty()) {
        messages.push(ChatMessage {
            role: "user".to_string(),
            content,
        });
    }
    messages.push(ChatMessage {
        role: "assistant".to_string(),
        content: text.to_string(),
    });

    if model == MODEL_DESIGN && !messages.iter().any(|m| m.role == "user") {
        return Err(
            TtsError::usage(E_USAGE, "voicedesign 模型需要音色描述（user 消息）")
                .with_hint("使用文本设计音色（VoiceRef::Design）提供音色描述"),
        );
    }

    let format = if stream {
        AudioFormat::Pcm16
    } else {
        request.format
    };

    Ok(ChatRequest {
        model: model.to_string(),
        messages,
        audio: AudioParams {
            format: format.as_str().to_string(),
            voice,
            optimize_text_preview: request.optimize_text_preview.then_some(true),
        },
        stream: stream.then_some(true),
    })
}

// ── 响应 ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Deserialize)]
pub struct ChatResponse {
    #[serde(default)]
    pub choices: Vec<Choice>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Choice {
    pub message: ResponseMessage,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseMessage {
    #[serde(default)]
    pub audio: Option<ResponseAudio>,
    #[serde(default)]
    pub content: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ResponseAudio {
    pub data: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub sample_rate: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChatChunk {
    #[serde(default)]
    pub choices: Vec<ChunkChoice>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkChoice {
    pub delta: Delta,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Delta {
    #[serde(default)]
    pub audio: Option<ChunkAudio>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChunkAudio {
    pub data: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelsResponse {
    #[serde(default)]
    pub data: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModelInfo {
    pub id: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorResponse {
    #[serde(default)]
    pub error: Option<ApiErrorBody>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ApiErrorBody {
    #[serde(default)]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mimo::{MODEL_CLONE, MODEL_TTS};

    fn request(voice: VoiceRef, instructions: Option<&str>) -> SynthRequest {
        SynthRequest {
            model: MODEL_TTS.to_string(),
            text: "早上好！".to_string(),
            instructions: instructions.map(|s| s.to_string()),
            voice,
            format: AudioFormat::Wav,
            optimize_text_preview: false,
        }
    }

    #[test]
    fn preset_request_shape() {
        let req = request(
            VoiceRef::Preset {
                name: "冰糖".to_string(),
            },
            Some("轻快一点"),
        );
        let built = build_chat_request(MODEL_TTS, &req, false).unwrap();
        let v = serde_json::to_value(&built).unwrap();
        assert_eq!(v["model"], serde_json::json!(MODEL_TTS));
        assert_eq!(v["messages"][0]["role"], serde_json::json!("user"));
        assert_eq!(v["messages"][0]["content"], serde_json::json!("轻快一点"));
        assert_eq!(v["messages"][1]["role"], serde_json::json!("assistant"));
        assert_eq!(v["messages"][1]["content"], serde_json::json!("早上好！"));
        assert_eq!(v["audio"]["format"], serde_json::json!("wav"));
        assert_eq!(v["audio"]["voice"], serde_json::json!("冰糖"));
        assert!(v.get("stream").is_none());
    }

    #[test]
    fn stream_forces_pcm16_and_stream_flag() {
        let req = request(
            VoiceRef::Preset {
                name: "苏打".into(),
            },
            None,
        );
        let built = build_chat_request(MODEL_TTS, &req, true).unwrap();
        let v = serde_json::to_value(&built).unwrap();
        assert_eq!(v["audio"]["format"], serde_json::json!("pcm16"));
        assert_eq!(v["stream"], serde_json::json!(true));
        // 无指令时省略 user 消息
        assert_eq!(v["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn design_requires_prompt_and_omits_voice() {
        let req = request(
            VoiceRef::Design {
                prompt: "温柔的女声".to_string(),
            },
            Some("语速放慢"),
        );
        let built = build_chat_request(MODEL_DESIGN, &req, false).unwrap();
        let v = serde_json::to_value(&built).unwrap();
        assert_eq!(
            v["messages"][0]["content"],
            serde_json::json!("温柔的女声\n语速放慢")
        );
        assert!(v["audio"].get("voice").is_none());

        let empty = request(
            VoiceRef::Design {
                prompt: "  ".to_string(),
            },
            None,
        );
        assert!(build_chat_request(MODEL_DESIGN, &empty, false).is_err());
    }

    #[test]
    fn clone_requires_data_url() {
        let ok = request(
            VoiceRef::Clone {
                sample_data_url: "data:audio/wav;base64,AAAA".into(),
            },
            None,
        );
        let built = build_chat_request(MODEL_CLONE, &ok, false).unwrap();
        let v = serde_json::to_value(&built).unwrap();
        assert_eq!(
            v["audio"]["voice"],
            serde_json::json!("data:audio/wav;base64,AAAA")
        );

        let bad = request(
            VoiceRef::Clone {
                sample_data_url: "voice.wav".into(),
            },
            None,
        );
        assert!(build_chat_request(MODEL_CLONE, &bad, false).is_err());
    }

    #[test]
    fn optimize_text_preview_is_design_only() {
        let mut req = request(
            VoiceRef::Preset {
                name: "茉莉".into(),
            },
            None,
        );
        req.optimize_text_preview = true;
        let err = build_chat_request(MODEL_TTS, &req, false).unwrap_err();
        assert_eq!(err.code(), E_USAGE);

        let mut design = request(
            VoiceRef::Design {
                prompt: "年轻男声".into(),
            },
            None,
        );
        design.optimize_text_preview = true;
        let built = build_chat_request(MODEL_DESIGN, &design, false).unwrap();
        let v = serde_json::to_value(&built).unwrap();
        assert_eq!(v["audio"]["optimize_text_preview"], serde_json::json!(true));
    }

    #[test]
    fn empty_text_is_rejected() {
        let mut req = request(
            VoiceRef::Preset {
                name: "冰糖".into(),
            },
            None,
        );
        req.text = "   ".to_string();
        assert!(build_chat_request(MODEL_TTS, &req, false).is_err());
    }
}
