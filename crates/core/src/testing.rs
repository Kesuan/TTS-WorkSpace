//! 仅测试使用的公共 Fake（供 synth / batch 单测复用）。

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use crate::audio::pcm16_to_wav;
use crate::error::{Result, TtsError, E_HTTP_RATE_LIMIT};
use crate::provider::{AudioFormat, Capabilities, SynthRequest, SynthResult, TtsProvider};

/// 可配置失败的假 Provider：前 N 次调用返回可重试错误，之后成功。
pub struct FakeProvider {
    plain_calls: AtomicUsize,
    stream_calls: AtomicUsize,
    fail_first: Mutex<usize>,
    fail_code: &'static str,
    wav: Vec<u8>,
}

impl FakeProvider {
    pub fn new() -> Self {
        Self::failing_first(0, E_HTTP_RATE_LIMIT)
    }

    pub fn failing_first(n: usize, fail_code: &'static str) -> Self {
        Self {
            plain_calls: AtomicUsize::new(0),
            stream_calls: AtomicUsize::new(0),
            fail_first: Mutex::new(n),
            fail_code,
            // 100ms 静音 @24kHz
            wav: pcm16_to_wav(&vec![0u8; 4_800], 24_000).expect("生成测试 WAV 失败"),
        }
    }

    pub fn plain_calls(&self) -> usize {
        self.plain_calls.load(Ordering::SeqCst)
    }

    pub fn stream_calls(&self) -> usize {
        self.stream_calls.load(Ordering::SeqCst)
    }

    fn maybe_fail(&self) -> Result<()> {
        let mut remaining = self.fail_first.lock().expect("锁失败");
        if *remaining > 0 {
            *remaining -= 1;
            return Err(TtsError::runtime(
                self.fail_code,
                format!("注入的模拟失败（{}）", self.fail_code),
            ));
        }
        Ok(())
    }
}

impl Default for FakeProvider {
    fn default() -> Self {
        Self::new()
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
        self.maybe_fail()?;
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
        self.maybe_fail()?;
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
