//! MiMo API 客户端（OpenAI 兼容 chat.completions）。
//!
//! - 非流式：`format=wav`，直接返回 WAV 字节；
//! - 流式：`format=pcm16`，逐块回调 PCM 增量（24kHz 单声道），结束后拼装 WAV；
//! - 所有网络/协议错误映射为统一的 `TtsError`（错误码见 `error.rs`）。

use std::time::{Duration, Instant};

use async_trait::async_trait;
use base64::Engine as _;
use futures_util::StreamExt;
use reqwest::StatusCode;
use serde_json::Value;

use crate::audio::{pcm16_to_wav, wav_info, DEFAULT_SAMPLE_RATE};
use crate::config::Config;
use crate::error::{
    Result, TtsError, E_API_RESPONSE, E_AUDIO_DECODE, E_CAPABILITY, E_CONFIG_KEY, E_HTTP_NETWORK,
    E_HTTP_RATE_LIMIT, E_HTTP_STATUS,
};
use crate::mimo::types::{
    build_chat_request, ApiErrorResponse, ChatChunk, ChatResponse, ModelsResponse,
};
use crate::provider::{AudioFormat, Capabilities, SynthRequest, SynthResult, TtsProvider};

/// 流式回调类型（显式高阶生命周期，避免嵌套引用下的推断歧义）。
pub type ChunkSink<'a> = &'a mut (dyn for<'b> FnMut(&'b [u8]) + Send);

pub struct MimoClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
}

impl MimoClient {
    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        timeout_ms: u64,
    ) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(timeout_ms.max(1_000)))
            // 实测：MiMo 在较大响应（如 voicedesign 的 ~500KB WAV）下 HTTP/2 流会被服务端中断，
            // 强制 HTTP/1.1 可稳定传输（curl --http1.1 验证通过）。
            .http1_only()
            .build()
            .map_err(|e| {
                TtsError::runtime(E_HTTP_NETWORK, format!("HTTP 客户端初始化失败：{e}"))
            })?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
        })
    }

    pub fn from_config(config: &Config) -> Result<Self> {
        Self::new(
            config.effective_base_url(),
            config.effective_api_key(),
            config.api.timeout_ms,
        )
    }

    fn require_key(&self) -> Result<&str> {
        self.api_key
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                TtsError::runtime(E_CONFIG_KEY, "未配置 API Key").with_hint(
                    "运行 `tts config set api.api_key <KEY>`，或设置环境变量 MIMO_API_KEY",
                )
            })
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let url = format!("{}{}", self.base_url, path);
        tracing::debug!(url = %url, "GET");
        let resp = self
            .http
            .get(&url)
            .bearer_auth(self.require_key()?)
            .send()
            .await
            .map_err(network_error)?;
        read_json_response(resp).await
    }

    async fn post_chat(&self, body: &Value) -> Result<reqwest::Response> {
        let url = format!("{}/chat/completions", self.base_url);
        tracing::debug!(url = %url, "POST chat/completions");
        self.http
            .post(&url)
            .bearer_auth(self.require_key()?)
            .json(body)
            .send()
            .await
            .map_err(network_error)
    }

    async fn list_models_impl(&self) -> Result<Vec<String>> {
        let value = self.get_json("/models").await?;
        let parsed: ModelsResponse = serde_json::from_value(value)
            .map_err(|e| TtsError::runtime(E_API_RESPONSE, format!("模型列表解析失败：{e}")))?;
        Ok(parsed.data.into_iter().map(|m| m.id).collect())
    }

    /// 校验能力并构建请求体（不发起网络请求）。
    fn prepare_body(&self, request: &SynthRequest, stream: bool) -> Result<Value> {
        let caps = self.capabilities(&request.model);
        if stream && !caps.streaming {
            return Err(TtsError::usage(
                E_CAPABILITY,
                format!("模型 {} 不支持低延迟流式输出", request.model),
            )
            .with_hint("流式仅支持预置音色模型（如 mimo-v2.5-tts）"));
        }
        let body = build_chat_request(&request.model, request, stream)?;
        serde_json::to_value(&body).map_err(TtsError::from)
    }

    async fn synth_once(&self, request: &SynthRequest) -> Result<SynthResult> {
        let body = self.prepare_body(request, false)?;
        let started = Instant::now();
        let resp = self.post_chat(&body).await?;
        let value = read_json_response(resp).await?;

        let parsed: ChatResponse = serde_json::from_value(value)
            .map_err(|e| TtsError::runtime(E_API_RESPONSE, format!("响应结构解析失败：{e}")))?;
        let audio = parsed
            .choices
            .into_iter()
            .next()
            .and_then(|c| c.message.audio)
            .ok_or_else(|| {
                TtsError::runtime(
                    E_API_RESPONSE,
                    "响应中缺少音频数据（choices[0].message.audio）",
                )
            })?;
        let bytes = decode_audio(&audio.data)?;
        let (wav, sample_rate, duration_ms) = match request.format {
            AudioFormat::Wav => {
                let info = wav_info(&bytes)?;
                (bytes, info.sample_rate, info.duration_ms)
            }
            AudioFormat::Pcm16 => {
                let wav = pcm16_to_wav(&bytes, DEFAULT_SAMPLE_RATE)?;
                let info = wav_info(&wav)?;
                (wav, info.sample_rate, info.duration_ms)
            }
        };
        Ok(SynthResult {
            audio: wav,
            format: AudioFormat::Wav,
            sample_rate,
            duration_ms,
            latency_ms: started.elapsed().as_millis() as u64,
            model: request.model.clone(),
            cache_key: None,
        })
    }

    async fn synth_streamed(
        &self,
        request: &SynthRequest,
        on_chunk: ChunkSink<'_>,
    ) -> Result<SynthResult> {
        let body = self.prepare_body(request, true)?;
        let started = Instant::now();
        let resp = self.post_chat(&body).await?;
        if !resp.status().is_success() {
            // 非 2xx：统一错误处理读取错误体（必定返回 Err）
            let _ = read_json_response(resp).await?;
            return Err(TtsError::runtime(E_API_RESPONSE, "流式请求失败"));
        }

        let mut pcm: Vec<u8> = Vec::new();
        let mut buffer: Vec<u8> = Vec::new();
        let mut done = false;
        let mut stream = resp.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(network_error)?;
            buffer.extend_from_slice(&bytes);
            while let Some(pos) = buffer.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = buffer.drain(..=pos).collect();
                let line = String::from_utf8_lossy(&line);
                let Some(payload) = line.trim().strip_prefix("data:") else {
                    continue;
                };
                let payload = payload.trim();
                if payload.is_empty() {
                    continue;
                }
                if payload == "[DONE]" {
                    done = true;
                    break;
                }
                let parsed: ChatChunk = match serde_json::from_str(payload) {
                    Ok(parsed) => parsed,
                    Err(_) => continue, // 忽略非 JSON 行（保活等）
                };
                for choice in parsed.choices {
                    if let Some(audio) = choice.delta.audio {
                        let bytes = decode_audio(&audio.data)?;
                        on_chunk(&bytes);
                        pcm.extend_from_slice(&bytes);
                    }
                }
            }
            if done {
                break;
            }
        }
        if pcm.is_empty() {
            return Err(TtsError::runtime(E_API_RESPONSE, "流式响应未包含音频数据")
                .with_hint("确认模型支持流式（mimo-v2.5-tts）"));
        }
        let wav = pcm16_to_wav(&pcm, DEFAULT_SAMPLE_RATE)?;
        let info = wav_info(&wav)?;
        Ok(SynthResult {
            audio: wav,
            format: AudioFormat::Wav,
            sample_rate: info.sample_rate,
            duration_ms: info.duration_ms,
            latency_ms: started.elapsed().as_millis() as u64,
            model: request.model.clone(),
            cache_key: None,
        })
    }
}

#[async_trait]
impl TtsProvider for MimoClient {
    fn name(&self) -> &'static str {
        crate::mimo::PROVIDER_NAME
    }

    fn capabilities(&self, model: &str) -> Capabilities {
        crate::mimo::capabilities_for(model)
    }

    async fn list_models(&self) -> Result<Vec<String>> {
        self.list_models_impl().await
    }

    async fn synthesize(&self, request: &SynthRequest) -> Result<SynthResult> {
        self.synth_once(request).await
    }

    async fn synthesize_stream(
        &self,
        request: &SynthRequest,
        on_chunk: ChunkSink<'_>,
    ) -> Result<SynthResult> {
        self.synth_streamed(request, on_chunk).await
    }
}

fn decode_audio(data: &str) -> Result<Vec<u8>> {
    base64::engine::general_purpose::STANDARD
        .decode(data.as_bytes())
        .map_err(|e| TtsError::runtime(E_AUDIO_DECODE, format!("音频 base64 解码失败：{e}")))
}

fn network_error(e: reqwest::Error) -> TtsError {
    let details = serde_json::json!({
        "is_timeout": e.is_timeout(),
        "is_connect": e.is_connect(),
    });
    TtsError::runtime(E_HTTP_NETWORK, format!("网络请求失败：{e}")).with_details(details)
}

async fn read_json_response(resp: reqwest::Response) -> Result<Value> {
    let status = resp.status();
    let text = resp.text().await.map_err(network_error)?;
    if !status.is_success() {
        let api_message = serde_json::from_str::<ApiErrorResponse>(&text)
            .ok()
            .and_then(|r| r.error)
            .and_then(|b| b.message)
            .filter(|m| !m.trim().is_empty());
        let summary = api_message.unwrap_or_else(|| text.chars().take(400).collect());
        let code = if status == StatusCode::TOO_MANY_REQUESTS {
            E_HTTP_RATE_LIMIT
        } else {
            E_HTTP_STATUS
        };
        return Err(TtsError::runtime(
            code,
            format!("API 请求失败（HTTP {}）：{summary}", status.as_u16()),
        )
        .with_details(serde_json::json!({
            "status": status.as_u16(),
            "body": text.chars().take(2_000).collect::<String>(),
        })));
    }
    serde_json::from_str::<Value>(&text)
        .map_err(|e| TtsError::runtime(E_API_RESPONSE, format!("响应不是合法 JSON：{e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mimo::MODEL_DESIGN;
    use crate::provider::VoiceRef;

    #[test]
    fn decode_audio_valid_and_invalid() {
        let encoded = base64::engine::general_purpose::STANDARD.encode([1u8, 2, 3]);
        assert_eq!(decode_audio(&encoded).unwrap(), vec![1, 2, 3]);
        let err = decode_audio("!!!not-base64!!!").unwrap_err();
        assert_eq!(err.code(), E_AUDIO_DECODE);
    }

    #[tokio::test]
    async fn stream_rejected_for_non_streaming_model() {
        let client = MimoClient::new("http://127.0.0.1:9", Some("test-key".into()), 1_000).unwrap();
        let request = SynthRequest {
            model: MODEL_DESIGN.to_string(),
            text: "你好".to_string(),
            instructions: None,
            voice: VoiceRef::Design {
                prompt: "温柔女声".to_string(),
            },
            format: AudioFormat::Wav,
            optimize_text_preview: false,
        };
        let mut sink = |_: &[u8]| {};
        let err = client
            .synthesize_stream(&request, &mut sink)
            .await
            .unwrap_err();
        assert_eq!(err.code(), E_CAPABILITY);
        assert_eq!(err.exit_code(), 2);
    }

    #[tokio::test]
    async fn missing_key_is_reported_before_network() {
        let client = MimoClient::new("http://127.0.0.1:9", None, 1_000).unwrap();
        let err = client.list_models().await.unwrap_err();
        assert_eq!(err.code(), E_CONFIG_KEY);
    }
}
