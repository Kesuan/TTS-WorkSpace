//! 音色工具：预置音色表、CLI 音色引用语法、克隆样本编码。

use std::path::Path;

use base64::Engine as _;
use serde::Serialize;

use crate::error::{Result, TtsError, E_IO, E_NOT_FOUND, E_USAGE};
use crate::provider::VoiceRef;

/// 克隆样本 base64 后的字节上限（MiMo 平台限制 10MB）。
pub const MAX_SAMPLE_BASE64_BYTES: usize = 10 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct PresetVoice {
    pub id: &'static str,
    pub name: &'static str,
    pub language: &'static str,
    pub gender: &'static str,
}

/// 预置音色表（来源：MiMo TTS 文档）。
pub const PRESET_VOICES: &[PresetVoice] = &[
    PresetVoice {
        id: "mimo_default",
        name: "MiMo-默认",
        language: "自动",
        gender: "-",
    },
    PresetVoice {
        id: "冰糖",
        name: "冰糖",
        language: "中文",
        gender: "女性",
    },
    PresetVoice {
        id: "茉莉",
        name: "茉莉",
        language: "中文",
        gender: "女性",
    },
    PresetVoice {
        id: "苏打",
        name: "苏打",
        language: "中文",
        gender: "男性",
    },
    PresetVoice {
        id: "白桦",
        name: "白桦",
        language: "中文",
        gender: "男性",
    },
    PresetVoice {
        id: "Mia",
        name: "Mia",
        language: "英文",
        gender: "女性",
    },
    PresetVoice {
        id: "Chloe",
        name: "Chloe",
        language: "英文",
        gender: "女性",
    },
    PresetVoice {
        id: "Milo",
        name: "Milo",
        language: "英文",
        gender: "男性",
    },
    PresetVoice {
        id: "Dean",
        name: "Dean",
        language: "英文",
        gender: "男性",
    },
];

pub fn preset_by_name(name: &str) -> Option<&'static PresetVoice> {
    let name = name.trim();
    PRESET_VOICES
        .iter()
        .find(|voice| voice.id == name || voice.name == name)
}

/// 解析 CLI 音色引用：
///
/// - `<预置名或 ID>` / `preset:<名>` → 预置音色；
/// - `design:<音色描述>` → 文本设计音色（voicedesign 模型）；
/// - `clone:<音频文件路径>` → 克隆音色（读取文件并编码为 data URL）。
pub fn parse_voice_ref(spec: &str) -> Result<VoiceRef> {
    let spec = spec.trim();
    if spec.is_empty() {
        return Err(TtsError::usage(E_USAGE, "音色不能为空"));
    }
    if let Some(rest) = spec.strip_prefix("design:") {
        let prompt = rest.trim();
        if prompt.is_empty() {
            return Err(TtsError::usage(E_USAGE, "design: 后需要音色描述"));
        }
        return Ok(VoiceRef::Design {
            prompt: prompt.to_string(),
        });
    }
    if let Some(rest) = spec.strip_prefix("clone:") {
        let path = rest.trim();
        if path.is_empty() {
            return Err(TtsError::usage(E_USAGE, "clone: 后需要音频文件路径"));
        }
        return Ok(VoiceRef::Clone {
            sample_data_url: sample_data_url(Path::new(path))?,
        });
    }
    let name = spec.strip_prefix("preset:").unwrap_or(spec).trim();
    if name.is_empty() {
        return Err(TtsError::usage(E_USAGE, "预置音色名不能为空"));
    }
    Ok(VoiceRef::Preset {
        name: name.to_string(),
    })
}

/// 读取样本文件并编码为 `data:` URL（仅 wav/mp3；base64 后 ≤10MB）。
pub fn sample_data_url(path: &Path) -> Result<String> {
    if !path.is_file() {
        return Err(TtsError::runtime(
            E_NOT_FOUND,
            format!("克隆样本不存在：{}", path.display()),
        ));
    }
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    let mime = match extension.as_str() {
        "wav" => "audio/wav",
        "mp3" => "audio/mpeg",
        _ => {
            return Err(TtsError::usage(
                E_USAGE,
                format!("克隆样本仅支持 wav / mp3，收到 .{extension}"),
            )
            .with_hint("可用 ffmpeg 转码，例如：ffmpeg -i in.m4a -b:a 128k out.mp3"));
        }
    };
    let bytes = std::fs::read(path)
        .map_err(|e| TtsError::runtime(E_IO, format!("读取样本失败（{}）：{e}", path.display())))?;
    let encoded_len = base64_encoded_len(bytes.len());
    if encoded_len > MAX_SAMPLE_BASE64_BYTES {
        return Err(TtsError::usage(
            E_USAGE,
            format!(
                "样本 base64 后约 {} MB，超过 10MB 上限",
                encoded_len / 1024 / 1024
            ),
        )
        .with_hint("裁剪样本或转成 mp3 后重试"));
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{encoded}"))
}

/// base64 编码后的长度（不实际编码）。
pub fn base64_encoded_len(bytes: usize) -> usize {
    bytes.div_ceil(3) * 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preset_lookup_by_id_and_name() {
        assert_eq!(preset_by_name("冰糖").unwrap().language, "中文");
        assert_eq!(preset_by_name("mimo_default").unwrap().gender, "-");
        assert_eq!(preset_by_name("Mia").unwrap().language, "英文");
        assert!(preset_by_name("不存在").is_none());
        assert_eq!(PRESET_VOICES.len(), 9);
    }

    #[test]
    fn parse_voice_ref_variants() {
        assert_eq!(
            parse_voice_ref("冰糖").unwrap(),
            VoiceRef::Preset {
                name: "冰糖".into()
            }
        );
        assert_eq!(
            parse_voice_ref("preset:苏打").unwrap(),
            VoiceRef::Preset {
                name: "苏打".into()
            }
        );
        assert_eq!(
            parse_voice_ref("design:温柔的女声").unwrap(),
            VoiceRef::Design {
                prompt: "温柔的女声".into()
            }
        );
        assert!(parse_voice_ref("").is_err());
        assert!(parse_voice_ref("design:").is_err());
        assert!(parse_voice_ref("clone:").is_err());
    }

    #[test]
    fn clone_sample_is_validated_and_encoded() {
        let dir = tempfile::tempdir().unwrap();
        let sample = dir.path().join("voice.mp3");
        std::fs::write(&sample, b"ID3fake").unwrap();

        let spec = format!("clone:{}", sample.display());
        match parse_voice_ref(&spec).unwrap() {
            VoiceRef::Clone { sample_data_url } => {
                assert!(sample_data_url.starts_with("data:audio/mpeg;base64,"));
            }
            other => panic!("应为克隆引用，得到 {other:?}"),
        }

        // 不支持的扩展名
        let txt = dir.path().join("voice.txt");
        std::fs::write(&txt, b"x").unwrap();
        let err = sample_data_url(&txt).unwrap_err();
        assert_eq!(err.code(), E_USAGE);

        // 文件不存在
        let err = sample_data_url(&dir.path().join("nope.wav")).unwrap_err();
        assert_eq!(err.code(), E_NOT_FOUND);
    }

    #[test]
    fn base64_length_math() {
        assert_eq!(base64_encoded_len(0), 0);
        assert_eq!(base64_encoded_len(1), 4);
        assert_eq!(base64_encoded_len(3), 4);
        assert_eq!(base64_encoded_len(4), 8);
        // 7.6MB 的 wav 会超过 10MB 上限
        assert!(base64_encoded_len(8 * 1024 * 1024) > MAX_SAMPLE_BASE64_BYTES);
        assert!(base64_encoded_len(7 * 1024 * 1024) <= MAX_SAMPLE_BASE64_BYTES);
    }
}
