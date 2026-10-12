//! 全局音色库：`<workspace>/library/voices/<voice_id>/voice.json` + 样本文件。
//!
//! 三种来源：
//! - `preset`：MiMo 预置音色（仅记录引用，不占样本）；
//! - `design`：文本设计音色（`design_prompt` 为定义，样本为一次试听留档）；
//! - `clone`：克隆音色（样本文件为定义；可由 design 固化而来：`source_voice`）。
//!
//! `Library::resolve` 供 synth / batch / cast 统一解析音色引用：
//! `preset:<名>` / `<预置名或库内名>` / `design:<描述>` / `clone:<文件>`。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Result, TtsError, E_NOT_FOUND, E_USAGE};
use crate::provider::VoiceRef;
use crate::util::now_rfc3339;
use crate::voices::{base64_encoded_len, parse_voice_ref, preset_by_name, sample_data_url};

pub const LIBRARY_SCHEMA_VERSION: u32 = 1;
pub const KIND_PRESET: &str = "preset";
pub const KIND_DESIGN: &str = "design";
pub const KIND_CLONE: &str = "clone";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceEntry {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    /// `preset` | `design` | `clone`
    pub kind: String,
    /// 定义 / 使用该音色时的模型。
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub design_prompt: Option<String>,
    /// design 固化来源（design 音色 id）。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    pub created_at: String,
}

impl Default for VoiceEntry {
    fn default() -> Self {
        Self {
            schema_version: LIBRARY_SCHEMA_VERSION,
            id: String::new(),
            name: String::new(),
            kind: KIND_PRESET.to_string(),
            model: String::new(),
            preset_name: None,
            design_prompt: None,
            source_voice: None,
            sample_file: None,
            sample_sha256: None,
            tags: Vec::new(),
            notes: String::new(),
            created_at: now_rfc3339(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct VoiceRecord {
    pub entry: VoiceEntry,
    pub dir: PathBuf,
}

impl VoiceRecord {
    pub fn sample_path(&self) -> Option<PathBuf> {
        self.entry
            .sample_file
            .as_ref()
            .map(|file| self.dir.join(file))
            .filter(|path| path.is_file())
    }
}

#[derive(Debug, Clone)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// 列出全部音色（按创建时间倒序，其次名称）。
    pub fn list(&self) -> Result<Vec<VoiceEntry>> {
        let mut entries = Vec::new();
        if !self.root.is_dir() {
            return Ok(entries);
        }
        let dirs = fs::read_dir(&self.root).map_err(|e| lib_io("读取音色库失败", &self.root, e))?;
        for dir in dirs.flatten() {
            let path = dir.path().join("voice.json");
            if !path.is_file() {
                continue;
            }
            let raw =
                fs::read_to_string(&path).map_err(|e| lib_io("读取音色档案失败", &path, e))?;
            if let Ok(entry) = serde_json::from_str::<VoiceEntry>(&raw) {
                entries.push(entry);
            }
        }
        entries.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok(entries)
    }

    /// 按 id 或名称查找。
    pub fn find(&self, id_or_name: &str) -> Result<Option<VoiceRecord>> {
        let needle = id_or_name.trim();
        for entry in self.list()? {
            if entry.id == needle || entry.name == needle {
                let dir = self.root.join(&entry.id);
                return Ok(Some(VoiceRecord { entry, dir }));
            }
        }
        Ok(None)
    }

    /// 创建音色；`sample` 为要归档的样本文件（会复制到音色目录）。
    pub fn create(&self, entry: VoiceEntry, sample: Option<&Path>) -> Result<VoiceRecord> {
        let dir = self.root.join(&entry.id);
        fs::create_dir_all(&dir).map_err(|e| lib_io("创建音色目录失败", &dir, e))?;

        let mut entry = entry;
        if let Some(sample_path) = sample {
            let extension = sample_path
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("wav")
                .to_ascii_lowercase();
            let target_name = format!("sample.{extension}");
            let target = dir.join(&target_name);
            fs::copy(sample_path, &target).map_err(|e| {
                lib_io(
                    &format!("复制样本失败（{}）", sample_path.display()),
                    &target,
                    e,
                )
            })?;
            entry.sample_file = Some(target_name);
            entry.sample_sha256 = Some(sha256_file(&target)?);
        }

        let mut text = serde_json::to_string_pretty(&entry)?;
        text.push('\n');
        let entry_path = dir.join("voice.json");
        fs::write(&entry_path, text).map_err(|e| lib_io("写入音色档案失败", &entry_path, e))?;
        Ok(VoiceRecord { entry, dir })
    }

    /// 删除音色目录（调用方负责 `--yes` 确认）。
    pub fn remove(&self, id_or_name: &str) -> Result<VoiceEntry> {
        let record = self.find(id_or_name)?.ok_or_else(|| {
            TtsError::runtime(E_NOT_FOUND, format!("未找到音色：{id_or_name}"))
                .with_hint("用 `tts voice list` 查看可用音色")
        })?;
        fs::remove_dir_all(&record.dir).map_err(|e| lib_io("删除音色目录失败", &record.dir, e))?;
        Ok(record.entry)
    }

    /// 统一解析音色引用（预置 → 库内 → 内联语法）。
    pub fn resolve(&self, spec: &str) -> Result<VoiceRef> {
        let spec = spec.trim();
        if spec.is_empty() {
            return Err(TtsError::usage(E_USAGE, "音色不能为空"));
        }
        if spec.starts_with("design:") || spec.starts_with("clone:") || spec.starts_with("preset:")
        {
            return parse_voice_ref(spec);
        }
        if preset_by_name(spec).is_some() {
            return Ok(VoiceRef::Preset {
                name: spec.to_string(),
            });
        }
        if let Some(record) = self.find(spec)? {
            return record_to_ref(&record);
        }
        Err(TtsError::runtime(
            E_NOT_FOUND,
            format!("未找到音色：{spec}（既不是预置音色，也不在音色库）"),
        )
        .with_hint("用 `tts voice list` 查看可用音色；若为 MiMo 新增的预置音色，可用 `preset:<名称>` 显式指定"))
    }
}

fn record_to_ref(record: &VoiceRecord) -> Result<VoiceRef> {
    let entry = &record.entry;
    match entry.kind.as_str() {
        KIND_DESIGN => {
            let prompt = entry.design_prompt.clone().ok_or_else(|| {
                TtsError::runtime(E_USAGE, format!("音色 {} 缺少 design_prompt", entry.id))
            })?;
            Ok(VoiceRef::Design { prompt })
        }
        KIND_CLONE => {
            let sample = record.sample_path().ok_or_else(|| {
                TtsError::runtime(E_NOT_FOUND, format!("音色 {} 的样本文件缺失", entry.id))
            })?;
            Ok(VoiceRef::Clone {
                sample_data_url: sample_data_url(&sample)?,
            })
        }
        _ => Ok(VoiceRef::Preset {
            name: entry
                .preset_name
                .clone()
                .unwrap_or_else(|| entry.name.clone()),
        }),
    }
}

/// 生成可读且稳定的音色 id：`<ascii 前缀>-<内容哈希前 6 位>`。
pub fn make_voice_id(name: &str, kind: &str, seed: &str) -> String {
    let mut prefix = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            prefix.push(ch.to_ascii_lowercase());
        } else if !prefix.is_empty() && !prefix.ends_with('-') {
            prefix.push('-');
        }
    }
    let prefix = prefix.trim_matches('-').to_string();
    let mut hasher = Sha256::new();
    hasher.update(kind.as_bytes());
    hasher.update([0x1f]);
    hasher.update(name.as_bytes());
    hasher.update([0x1f]);
    hasher.update(seed.as_bytes());
    let digest = hex::encode(hasher.finalize());
    let short = &digest[..6];
    if prefix.is_empty() {
        format!("voice-{short}")
    } else {
        format!("{prefix}-{short}")
    }
}

pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).map_err(|e| lib_io("读取文件失败", path, e))?;
    Ok(hex::encode(Sha256::digest(&bytes)))
}

/// 校验样本文件可用（扩展名 wav/mp3、base64 后 ≤10MB）。
pub fn validate_sample(path: &Path) -> Result<u64> {
    if !path.is_file() {
        return Err(TtsError::runtime(
            E_NOT_FOUND,
            format!("样本文件不存在：{}", path.display()),
        ));
    }
    let extension = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if extension != "wav" && extension != "mp3" {
        return Err(
            TtsError::usage(E_USAGE, format!("样本仅支持 wav / mp3，收到 .{extension}"))
                .with_hint("可用 ffmpeg 转码，例如：ffmpeg -i in.m4a -b:a 128k out.mp3"),
        );
    }
    let size = fs::metadata(path)
        .map_err(|e| lib_io("读取样本信息失败", path, e))?
        .len();
    if base64_encoded_len(size as usize) > crate::voices::MAX_SAMPLE_BASE64_BYTES {
        return Err(TtsError::usage(
            E_USAGE,
            format!(
                "样本 base64 后约 {} MB，超过 10MB 上限",
                base64_encoded_len(size as usize) / 1024 / 1024
            ),
        )
        .with_hint("裁剪样本或转成 mp3 后重试"));
    }
    Ok(size)
}

fn lib_io(message: &str, path: &Path, e: std::io::Error) -> TtsError {
    TtsError::runtime(
        crate::error::E_IO,
        format!("{message}（{}）：{e}", path.display()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::pcm16_to_wav;

    fn library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        let library = Library::new(dir.path().join("voices"));
        (dir, library)
    }

    fn write_sample(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        fs::write(&path, pcm16_to_wav(&vec![0u8; 480], 24_000).unwrap()).unwrap();
        path
    }

    #[test]
    fn make_id_is_stable_and_readable() {
        let id = make_voice_id("Yuki", KIND_CLONE, "abc");
        assert!(id.starts_with("yuki-"));
        assert_eq!(id.len(), "yuki-".len() + 6);
        assert_eq!(id, make_voice_id("Yuki", KIND_CLONE, "abc"));
        let cn = make_voice_id("阿岚", KIND_DESIGN, "prompt");
        assert!(cn.starts_with("voice-"));
    }

    #[test]
    fn create_find_list_and_remove() {
        let (dir, library) = library();
        let sample = write_sample(dir.path(), "sample.wav");

        let entry = VoiceEntry {
            id: make_voice_id("Yuki", KIND_CLONE, "x"),
            name: "Yuki".to_string(),
            kind: KIND_CLONE.to_string(),
            model: "mimo-v2.5-tts-voiceclone".to_string(),
            ..VoiceEntry::default()
        };
        let record = library.create(entry, Some(&sample)).unwrap();
        assert!(record.dir.join("sample.wav").is_file());
        assert!(record.entry.sample_sha256.is_some());

        let found = library.find("Yuki").unwrap().unwrap();
        assert_eq!(found.entry.id, record.entry.id);
        assert!(found.sample_path().is_some());

        assert_eq!(library.list().unwrap().len(), 1);

        let removed = library.remove("Yuki").unwrap();
        assert_eq!(removed.name, "Yuki");
        assert!(library.list().unwrap().is_empty());
        assert!(library.remove("Yuki").is_err());
    }

    #[test]
    fn resolve_prefers_preset_then_library() {
        let (dir, library) = library();
        // 预置优先
        assert_eq!(
            library.resolve("冰糖").unwrap(),
            VoiceRef::Preset {
                name: "冰糖".into()
            }
        );
        // 未知名称报错（预置音色可用 `preset:<名称>` 显式透传）
        let err = library.resolve("未来的音色").unwrap_err();
        assert_eq!(err.code(), E_NOT_FOUND);
        assert_eq!(
            library.resolve("preset:未来的音色").unwrap(),
            VoiceRef::Preset {
                name: "未来的音色".into()
            }
        );
        // 内联语法
        assert_eq!(
            library.resolve("design:温柔").unwrap(),
            VoiceRef::Design {
                prompt: "温柔".into()
            }
        );
        // 库内 design
        let entry = VoiceEntry {
            id: make_voice_id("设计音色", KIND_DESIGN, "温柔"),
            name: "设计音色".to_string(),
            kind: KIND_DESIGN.to_string(),
            model: "mimo-v2.5-tts-voicedesign".to_string(),
            design_prompt: Some("温柔".to_string()),
            ..VoiceEntry::default()
        };
        library.create(entry, None).unwrap();
        assert_eq!(
            library.resolve("设计音色").unwrap(),
            VoiceRef::Design {
                prompt: "温柔".into()
            }
        );
        // 库内 clone → data URL
        let sample = write_sample(dir.path(), "s.wav");
        let clone = VoiceEntry {
            id: make_voice_id("克隆音色", KIND_CLONE, "s"),
            name: "克隆音色".to_string(),
            kind: KIND_CLONE.to_string(),
            model: "mimo-v2.5-tts-voiceclone".to_string(),
            ..VoiceEntry::default()
        };
        library.create(clone, Some(&sample)).unwrap();
        match library.resolve("克隆音色").unwrap() {
            VoiceRef::Clone { sample_data_url } => {
                assert!(sample_data_url.starts_with("data:audio/wav;base64,"));
            }
            other => panic!("应为克隆引用：{other:?}"),
        }
    }

    #[test]
    fn validate_sample_checks_format_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let wav = write_sample(dir.path(), "ok.wav");
        assert!(validate_sample(&wav).unwrap() > 0);

        let txt = dir.path().join("bad.txt");
        fs::write(&txt, b"x").unwrap();
        assert_eq!(validate_sample(&txt).unwrap_err().code(), E_USAGE);

        let missing = dir.path().join("nope.wav");
        assert_eq!(validate_sample(&missing).unwrap_err().code(), E_NOT_FOUND);
    }

    #[test]
    fn broken_entry_is_skipped_in_list() {
        let (dir, library) = library();
        let dir_path = library.root().join("broken-id");
        fs::create_dir_all(&dir_path).unwrap();
        fs::write(dir_path.join("voice.json"), "{ broken").unwrap();
        let _ = dir;
        assert!(library.list().unwrap().is_empty());
    }
}
