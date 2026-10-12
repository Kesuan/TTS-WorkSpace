//! Provider 抽象（v0.1 仅实现 MiMo，接口留多供应商扩展位）。

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use crate::error::Result;

/// 音色引用：预置音色 / 文本设计音色 / 克隆样本。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VoiceRef {
    /// 预置音色（`mimo-v2.5-tts`）：voice = 音色名，如 `冰糖`。
    Preset { name: String },
    /// 文本设计音色（`mimo-v2.5-tts-voicedesign`）：prompt 即音色描述。
    Design { prompt: String },
    /// 克隆音色（`mimo-v2.5-tts-voiceclone`）：sample_data_url 为 data URL。
    Clone { sample_data_url: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AudioFormat {
    Wav,
    Pcm16,
}

impl AudioFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            AudioFormat::Wav => "wav",
            AudioFormat::Pcm16 => "pcm16",
        }
    }
}

/// 模型能力声明：驱动 CLI 参数校验与 GUI 控件可用性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    pub streaming: bool,
    pub preset_voices: bool,
    pub voice_design: bool,
    pub voice_clone: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthRequest {
    pub model: String,
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instructions: Option<String>,
    pub voice: VoiceRef,
    pub format: AudioFormat,
    #[serde(default)]
    pub optimize_text_preview: bool,
}

/// 合成结果（audio 统一为 WAV 字节；流式 PCM 已在客户端拼装）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SynthResult {
    pub audio: Vec<u8>,
    pub format: AudioFormat,
    pub sample_rate: u32,
    pub duration_ms: u64,
    pub latency_ms: u64,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
}

#[async_trait]
pub trait TtsProvider: Send + Sync {
    fn name(&self) -> &'static str;

    fn capabilities(&self, model: &str) -> Capabilities;

    async fn list_models(&self) -> Result<Vec<String>>;

    async fn synthesize(&self, request: &SynthRequest) -> Result<SynthResult>;

    /// 低延迟流式合成；`on_chunk` 收到的是 PCM16（24kHz 单声道）增量。
    ///
    /// 回调类型经 `ChunkSink` 显式声明高阶生命周期（见 `mimo::client`）。
    async fn synthesize_stream(
        &self,
        request: &SynthRequest,
        on_chunk: crate::mimo::client::ChunkSink<'_>,
    ) -> Result<SynthResult>;
}
