//! 合成编排：请求指纹 → 缓存 → Provider → 结果（WAV 字节 + 元数据）。
//!
//! 网络细节在 Provider（`mimo::client`）；本模块负责幂等与复用：
//! 相同参数（模型 / 音色 / 文本 / 风格 / 格式 / 流式标志）命中缓存时不重复请求。

use serde_json::json;

use crate::audio::wav_info;
use crate::cache::{self, Cache};
use crate::error::Result;
use crate::provider::{AudioFormat, SynthRequest, TtsProvider, VoiceRef};
use crate::script::normalize_text;

#[derive(Debug, Clone)]
pub struct SynthOptions {
    pub model: String,
    pub text: String,
    pub voice: VoiceRef,
    pub instructions: Option<String>,
    pub format: AudioFormat,
    pub stream: bool,
    pub use_cache: bool,
}

#[derive(Debug, Clone)]
pub struct SynthOutcome {
    pub audio: Vec<u8>,
    pub cache_key: String,
    pub cached: bool,
    pub sample_rate: u32,
    pub duration_ms: u64,
    pub latency_ms: u64,
    pub chars: usize,
    pub model: String,
}

/// 请求指纹（缓存键）：对语义等价的参数生成稳定 key。
pub fn request_cache_key(options: &SynthOptions) -> String {
    let instructions = normalize_text(options.instructions.as_deref().unwrap_or(""));
    cache::fingerprint(&[
        "tts-v1",
        &options.model,
        &voice_fingerprint(&options.voice),
        &normalize_text(&options.text),
        &instructions,
        options.format.as_str(),
        if options.stream { "stream" } else { "oneshot" },
    ])
}

/// 音色指纹（克隆样本以哈希参与，避免大字符串入 key）。
pub fn voice_fingerprint(voice: &VoiceRef) -> String {
    match voice {
        VoiceRef::Preset { name } => format!("preset:{}", name.trim()),
        VoiceRef::Design { prompt } => format!("design:{}", normalize_text(prompt)),
        VoiceRef::Clone { sample_data_url } => {
            format!("clone:{}", cache::fingerprint(&[sample_data_url]))
        }
    }
}

/// 合成（幂等）：命中缓存直接复用；否则请求 Provider 并写入缓存。
pub async fn synthesize(
    provider: &dyn TtsProvider,
    cache: &Cache,
    options: &SynthOptions,
) -> Result<SynthOutcome> {
    let key = request_cache_key(options);
    let chars = options.text.chars().count();

    if options.use_cache {
        if let Some(entry) = cache.get(&key)? {
            let info = wav_info(&entry.bytes)?;
            return Ok(SynthOutcome {
                audio: entry.bytes,
                cache_key: key,
                cached: true,
                sample_rate: info.sample_rate,
                duration_ms: info.duration_ms,
                latency_ms: 0,
                chars,
                model: options.model.clone(),
            });
        }
    }

    let request = SynthRequest {
        model: options.model.clone(),
        text: options.text.clone(),
        instructions: options.instructions.clone(),
        voice: options.voice.clone(),
        format: options.format,
        optimize_text_preview: false,
    };
    let result = if options.stream {
        let mut sink = |_: &[u8]| {};
        provider.synthesize_stream(&request, &mut sink).await?
    } else {
        provider.synthesize(&request).await?
    };

    let meta = json!({
        "schema_version": 1,
        "model": options.model,
        "voice": voice_fingerprint(&options.voice),
        "chars": chars,
        "duration_ms": result.duration_ms,
        "sample_rate": result.sample_rate,
        "stream": options.stream,
        "created_at": crate::util::now_rfc3339(),
    });
    cache.put(&key, &result.audio, &meta)?;

    Ok(SynthOutcome {
        audio: result.audio,
        cache_key: key,
        cached: false,
        sample_rate: result.sample_rate,
        duration_ms: result.duration_ms,
        latency_ms: result.latency_ms,
        chars,
        model: options.model.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::audio::pcm16_to_wav;
    use crate::error::Result;
    use crate::provider::{Capabilities, SynthResult};

    struct FakeProvider {
        plain_calls: AtomicUsize,
        stream_calls: AtomicUsize,
        wav: Vec<u8>,
    }

    impl FakeProvider {
        fn new() -> Self {
            Self {
                plain_calls: AtomicUsize::new(0),
                stream_calls: AtomicUsize::new(0),
                wav: pcm16_to_wav(&vec![0u8; 4_800], 24_000).unwrap(),
            }
        }
    }

    #[async_trait]
    impl TtsProvider for FakeProvider {
        fn name(&self) -> &'static str {
            "fake"
        }

        fn capabilities(&self, _model: &str) -> Capabilities {
            Capabilities {
                streaming: true,
                preset_voices: true,
                voice_design: true,
                voice_clone: true,
            }
        }

        async fn list_models(&self) -> Result<Vec<String>> {
            Ok(vec!["fake-tts".to_string()])
        }

        async fn synthesize(&self, _request: &SynthRequest) -> Result<SynthResult> {
            self.plain_calls.fetch_add(1, Ordering::SeqCst);
            Ok(SynthResult {
                audio: self.wav.clone(),
                format: AudioFormat::Wav,
                sample_rate: 24_000,
                duration_ms: 100,
                latency_ms: 7,
                model: "fake-tts".to_string(),
                cache_key: None,
            })
        }

        async fn synthesize_stream(
            &self,
            _request: &SynthRequest,
            _on_chunk: crate::mimo::client::ChunkSink<'_>,
        ) -> Result<SynthResult> {
            self.stream_calls.fetch_add(1, Ordering::SeqCst);
            Ok(SynthResult {
                audio: self.wav.clone(),
                format: AudioFormat::Wav,
                sample_rate: 24_000,
                duration_ms: 100,
                latency_ms: 3,
                model: "fake-tts".to_string(),
                cache_key: None,
            })
        }
    }

    fn options(text: &str) -> SynthOptions {
        SynthOptions {
            model: "fake-tts".to_string(),
            text: text.to_string(),
            voice: VoiceRef::Preset {
                name: "冰糖".to_string(),
            },
            instructions: None,
            format: AudioFormat::Wav,
            stream: false,
            use_cache: true,
        }
    }

    #[tokio::test]
    async fn second_call_hits_cache() {
        let provider = FakeProvider::new();
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());

        let first = synthesize(&provider, &cache, &options("早上好"))
            .await
            .unwrap();
        assert!(!first.cached);
        assert_eq!(provider.plain_calls.load(Ordering::SeqCst), 1);
        assert_eq!(first.duration_ms, 100);

        let second = synthesize(&provider, &cache, &options("早上好"))
            .await
            .unwrap();
        assert!(second.cached);
        assert_eq!(
            provider.plain_calls.load(Ordering::SeqCst),
            1,
            "不应重复请求"
        );
        assert_eq!(second.audio, first.audio);
        assert_eq!(second.cache_key, first.cache_key);
    }

    #[tokio::test]
    async fn no_cache_forces_request() {
        let provider = FakeProvider::new();
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());

        synthesize(&provider, &cache, &options("再来一次"))
            .await
            .unwrap();
        let mut forced = options("再来一次");
        forced.use_cache = false;
        let result = synthesize(&provider, &cache, &forced).await.unwrap();
        assert!(!result.cached);
        assert_eq!(provider.plain_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn stream_flag_uses_stream_path_and_separate_cache_key() {
        let provider = FakeProvider::new();
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());

        let mut streamed = options("流式");
        streamed.stream = true;
        let result = synthesize(&provider, &cache, &streamed).await.unwrap();
        assert_eq!(provider.stream_calls.load(Ordering::SeqCst), 1);
        assert_eq!(provider.plain_calls.load(Ordering::SeqCst), 0);
        assert_ne!(result.cache_key, request_cache_key(&options("流式")));
    }

    #[test]
    fn fingerprint_ignores_whitespace_differences() {
        let a = request_cache_key(&options("你好  世界"));
        let b = request_cache_key(&options("你好 世界"));
        assert_eq!(a, b);
    }
}
