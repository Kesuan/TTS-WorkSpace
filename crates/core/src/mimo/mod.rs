//! MiMo Provider：OpenAI 兼容 chat.completions 封装。

pub mod client;
pub mod types;

pub use client::MimoClient;

pub const PROVIDER_NAME: &str = "mimo";
pub const MODEL_TTS: &str = "mimo-v2.5-tts";
pub const MODEL_DESIGN: &str = "mimo-v2.5-tts-voicedesign";
pub const MODEL_CLONE: &str = "mimo-v2.5-tts-voiceclone";

/// 能力声明：流式仅预置音色模型支持；设计/克隆模型用于音色来源，不支持流式。
pub fn capabilities_for(model: &str) -> crate::provider::Capabilities {
    use crate::provider::Capabilities;
    match model {
        MODEL_TTS => Capabilities {
            streaming: true,
            preset_voices: true,
            voice_design: false,
            voice_clone: false,
        },
        MODEL_DESIGN => Capabilities {
            streaming: false,
            preset_voices: false,
            voice_design: true,
            voice_clone: false,
        },
        MODEL_CLONE => Capabilities {
            streaming: false,
            preset_voices: false,
            voice_design: false,
            voice_clone: true,
        },
        _ => Capabilities {
            streaming: false,
            preset_voices: false,
            voice_design: false,
            voice_clone: false,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capabilities_match_documented_behavior() {
        assert!(capabilities_for(MODEL_TTS).streaming);
        assert!(capabilities_for(MODEL_TTS).preset_voices);
        assert!(!capabilities_for(MODEL_DESIGN).streaming);
        assert!(capabilities_for(MODEL_DESIGN).voice_design);
        assert!(capabilities_for(MODEL_CLONE).voice_clone);
        assert!(!capabilities_for("unknown-model").streaming);
    }
}
