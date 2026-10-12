//! 内容寻址缓存：请求指纹 → `cache/<key16>/blob.wav` + `meta.json`。
//!
//! 相同参数（模型 / 音色 / 文本 / 风格 / 格式）的请求命中缓存后直接复用，
//! Agent 重试与批量重跑零成本（不重复计费、不重复等待）。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{Result, TtsError, E_CACHE_IO};

/// 指纹取 sha256 前 16 个十六进制字符（作为目录名）。
pub const KEY_LEN: usize = 16;

/// 计算请求指纹；各部分用 0x1f 分隔，避免拼接歧义。
pub fn fingerprint(parts: &[&str]) -> String {
    let mut hasher = Sha256::new();
    for (i, part) in parts.iter().enumerate() {
        if i > 0 {
            hasher.update([0x1f]);
        }
        hasher.update(part.as_bytes());
    }
    let digest = hasher.finalize();
    hex::encode(digest)[..KEY_LEN].to_string()
}

#[derive(Debug, Clone)]
pub struct CacheEntry {
    pub key: String,
    pub bytes: Vec<u8>,
    pub meta: Value,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct GcReport {
    pub removed: u64,
    pub freed_bytes: u64,
    pub kept: u64,
}

#[derive(Debug, Clone)]
pub struct Cache {
    root: PathBuf,
}

impl Cache {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn dir_for(&self, key: &str) -> PathBuf {
        self.root.join(safe_component(key))
    }

    pub fn blob_path(&self, key: &str) -> PathBuf {
        self.dir_for(key).join("blob.wav")
    }

    pub fn meta_path(&self, key: &str) -> PathBuf {
        self.dir_for(key).join("meta.json")
    }

    pub fn get(&self, key: &str) -> Result<Option<CacheEntry>> {
        let blob = self.blob_path(key);
        if !blob.exists() {
            return Ok(None);
        }
        let bytes = fs::read(&blob).map_err(|e| cache_io("读取缓存失败", &blob, e))?;
        let meta = fs::read_to_string(self.meta_path(key))
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(&t).ok())
            .unwrap_or(Value::Null);
        Ok(Some(CacheEntry {
            key: key.to_string(),
            bytes,
            meta,
        }))
    }

    pub fn put(&self, key: &str, bytes: &[u8], meta: &Value) -> Result<PathBuf> {
        let dir = self.dir_for(key);
        fs::create_dir_all(&dir).map_err(|e| cache_io("创建缓存目录失败", &dir, e))?;
        let blob = self.blob_path(key);
        fs::write(&blob, bytes).map_err(|e| cache_io("写入缓存失败", &blob, e))?;
        let meta_path = self.meta_path(key);
        let text = serde_json::to_string_pretty(meta).unwrap_or_else(|_| "null".to_string());
        fs::write(&meta_path, text).map_err(|e| cache_io("写入缓存元数据失败", &meta_path, e))?;
        Ok(blob)
    }

    /// 列出缓存目录中的 key（目录名，排序后返回）。
    pub fn keys(&self) -> Result<Vec<String>> {
        let mut keys = Vec::new();
        if !self.root.exists() {
            return Ok(keys);
        }
        let entries =
            fs::read_dir(&self.root).map_err(|e| cache_io("读取缓存目录失败", &self.root, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| cache_io("读取缓存目录失败", &self.root, e))?;
            if entry.path().is_dir() {
                keys.push(entry.file_name().to_string_lossy().to_string());
            }
        }
        keys.sort();
        Ok(keys)
    }

    /// 删除未被引用的缓存目录；`referenced` 为仍被使用的 key 集合。
    pub fn gc(&self, referenced: &HashSet<String>) -> Result<GcReport> {
        let mut report = GcReport {
            removed: 0,
            freed_bytes: 0,
            kept: 0,
        };
        if !self.root.exists() {
            return Ok(report);
        }
        let entries =
            fs::read_dir(&self.root).map_err(|e| cache_io("读取缓存目录失败", &self.root, e))?;
        for entry in entries {
            let entry = entry.map_err(|e| cache_io("读取缓存目录失败", &self.root, e))?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if referenced.contains(&name) {
                report.kept += 1;
                continue;
            }
            let size = directory_size(&path);
            fs::remove_dir_all(&path).map_err(|e| cache_io("删除缓存目录失败", &path, e))?;
            report.removed += 1;
            report.freed_bytes += size;
        }
        Ok(report)
    }
}

fn cache_io(message: &str, path: &Path, e: std::io::Error) -> TtsError {
    TtsError::runtime(E_CACHE_IO, format!("{message} {}：{e}", path.display()))
}

/// key 仅允许 16 位十六进制；非法输入（防路径穿越）退化为其指纹。
fn safe_component(key: &str) -> String {
    if key.len() == KEY_LEN && key.chars().all(|c| c.is_ascii_hexdigit()) {
        key.to_string()
    } else {
        fingerprint(&[key])
    }
}

/// 目录内文件大小合计（不递归子目录；缓存目录结构为单层文件）。
pub fn directory_size(path: &Path) -> u64 {
    let mut total = 0;
    if let Ok(entries) = fs::read_dir(path) {
        for entry in entries.flatten() {
            if let Ok(meta) = entry.metadata() {
                if meta.is_file() {
                    total += meta.len();
                }
            }
        }
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fingerprint_is_stable_and_distinct() {
        let a = fingerprint(&["m", "v", "text", "style", "wav"]);
        let b = fingerprint(&["m", "v", "text", "style", "wav"]);
        let c = fingerprint(&["m", "v", "text", "style", "pcm16"]);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), KEY_LEN);
        // 分隔符避免拼接歧义
        assert_ne!(fingerprint(&["ab", "c"]), fingerprint(&["a", "bc"]));
    }

    #[test]
    fn put_get_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let key = fingerprint(&["m", "v", "hello", "", "wav"]);
        assert!(cache.get(&key).unwrap().is_none());

        let meta = serde_json::json!({"model": "m", "chars": 5});
        cache.put(&key, b"RIFFDATA", &meta).unwrap();

        let entry = cache.get(&key).unwrap().unwrap();
        assert_eq!(entry.bytes, b"RIFFDATA");
        assert_eq!(entry.meta["chars"], serde_json::json!(5));
    }

    #[test]
    fn gc_removes_unreferenced() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let keep = fingerprint(&["keep"]);
        let drop_key = fingerprint(&["drop"]);
        cache.put(&keep, b"a", &serde_json::json!({})).unwrap();
        cache.put(&drop_key, b"bb", &serde_json::json!({})).unwrap();

        let mut referenced = HashSet::new();
        referenced.insert(keep.clone());
        let report = cache.gc(&referenced).unwrap();
        assert_eq!(report.removed, 1);
        assert_eq!(report.kept, 1);
        assert!(report.freed_bytes >= 2, "至少应统计 blob 字节数");
        assert!(cache.get(&keep).unwrap().is_some());
        assert!(cache.get(&drop_key).unwrap().is_none());
    }

    #[test]
    fn illegal_key_is_sanitized() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::new(dir.path());
        let path = cache.dir_for("../../etc");
        assert!(path.starts_with(dir.path()));
        assert_ne!(path, dir.path().to_path_buf());
    }
}
