//! 音频工具：PCM16 → WAV 组包、WAV 元信息读取。
//!
//! MiMo 流式输出为 24kHz PCM16LE 单声道，需要自行封装为 WAV。

use std::io::Cursor;

use hound::{SampleFormat, WavReader, WavSpec, WavWriter};
use serde::{Deserialize, Serialize};

use crate::error::{Result, TtsError, E_AUDIO_DECODE, E_AUDIO_ENCODE};

pub const DEFAULT_SAMPLE_RATE: u32 = 24_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WavInfo {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub duration_ms: u64,
}

fn encode_err(e: hound::Error) -> TtsError {
    TtsError::runtime(E_AUDIO_ENCODE, format!("WAV 写入失败：{e}"))
}

/// 将 PCM16LE 单声道字节流封装为 WAV。
pub fn pcm16_to_wav(pcm: &[u8], sample_rate: u32) -> Result<Vec<u8>> {
    if !pcm.len().is_multiple_of(2) {
        return Err(TtsError::runtime(
            E_AUDIO_DECODE,
            format!(
                "PCM 数据长度为奇数（{} 字节），无法按 16-bit 解析",
                pcm.len()
            ),
        ));
    }
    let spec = WavSpec {
        channels: 1,
        sample_rate,
        bits_per_sample: 16,
        sample_format: SampleFormat::Int,
    };
    let mut cursor = Cursor::new(Vec::<u8>::new());
    {
        let mut writer = WavWriter::new(&mut cursor, spec).map_err(encode_err)?;
        for chunk in pcm.as_chunks::<2>().0 {
            let sample = i16::from_le_bytes(*chunk);
            writer.write_sample(sample).map_err(encode_err)?;
        }
        writer.finalize().map_err(encode_err)?;
    }
    Ok(cursor.into_inner())
}

/// 读取 WAV 元信息（采样率 / 声道 / 位深 / 时长）。
pub fn wav_info(bytes: &[u8]) -> Result<WavInfo> {
    let reader = WavReader::new(Cursor::new(bytes))
        .map_err(|e| TtsError::runtime(E_AUDIO_DECODE, format!("WAV 解析失败：{e}")))?;
    let spec = reader.spec();
    let samples = reader.len() as u64;
    let frames = if spec.channels == 0 {
        0
    } else {
        samples / u64::from(spec.channels)
    };
    let duration_ms = if spec.sample_rate == 0 {
        0
    } else {
        frames * 1000 / u64::from(spec.sample_rate)
    };
    Ok(WavInfo {
        sample_rate: spec.sample_rate,
        channels: spec.channels,
        bits_per_sample: spec.bits_per_sample,
        duration_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_roundtrip_to_wav() {
        // 1 秒 440Hz 方波（振幅 1000）
        let mut pcm = Vec::new();
        for i in 0..24_000u32 {
            let sample: i16 = if (i / 55) % 2 == 0 { 1000 } else { -1000 };
            pcm.extend_from_slice(&sample.to_le_bytes());
        }
        let wav = pcm16_to_wav(&pcm, DEFAULT_SAMPLE_RATE).unwrap();
        let info = wav_info(&wav).unwrap();
        assert_eq!(info.sample_rate, 24_000);
        assert_eq!(info.channels, 1);
        assert_eq!(info.bits_per_sample, 16);
        assert_eq!(info.duration_ms, 1000);
    }

    #[test]
    fn odd_pcm_length_is_rejected() {
        let err = pcm16_to_wav(&[0u8, 1, 2], DEFAULT_SAMPLE_RATE).unwrap_err();
        assert_eq!(err.code(), E_AUDIO_DECODE);
    }

    #[test]
    fn invalid_wav_is_rejected() {
        let err = wav_info(b"not a wav").unwrap_err();
        assert_eq!(err.code(), E_AUDIO_DECODE);
    }
}
