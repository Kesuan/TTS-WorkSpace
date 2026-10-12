//! 工作区路径解析（跨平台）。
//!
//! 优先级：
//! 1. 环境变量 `TTS_WORKSPACE_ROOT`（测试与高级用户）
//! 2. 默认目录 `~/.tts-workbench/config.json` 中的 `workspace_root` 字段
//! 3. 默认目录 `~/.tts-workbench`
//!
//! 配置文件路径可用 `TTS_CONFIG` 覆盖（默认 `<root>/config.json`）。

use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use crate::error::{Result, TtsError, E_WORKSPACE};

pub const ENV_WORKSPACE_ROOT: &str = "TTS_WORKSPACE_ROOT";
pub const ENV_CONFIG: &str = "TTS_CONFIG";
pub const DEFAULT_DIR_NAME: &str = ".tts-workbench";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paths {
    pub root: PathBuf,
    pub config_file: PathBuf,
}

impl Paths {
    /// 按环境变量与默认规则解析工作区路径。
    pub fn resolve() -> Result<Self> {
        let root = match env::var(ENV_WORKSPACE_ROOT)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            Some(v) => expand_tilde(&v)?,
            None => default_root_with_override()?,
        };
        let config_file = match env::var(ENV_CONFIG)
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty())
        {
            Some(v) => expand_tilde(&v)?,
            None => root.join("config.json"),
        };
        Ok(Self { root, config_file })
    }

    pub fn library_dir(&self) -> PathBuf {
        self.root.join("library")
    }

    pub fn library_voices_dir(&self) -> PathBuf {
        self.root.join("library").join("voices")
    }

    pub fn cache_dir(&self) -> PathBuf {
        self.root.join("cache")
    }

    pub fn audit_file(&self) -> PathBuf {
        self.root.join("audit").join("audit.jsonl")
    }

    pub fn projects_dir(&self) -> PathBuf {
        self.root.join("projects")
    }

    /// 创建标准目录结构（幂等）。
    pub fn ensure_layout(&self) -> Result<()> {
        for dir in [
            self.root.clone(),
            self.library_voices_dir(),
            self.cache_dir(),
            self.root.join("audit"),
            self.projects_dir(),
        ] {
            fs::create_dir_all(&dir).map_err(|e| {
                TtsError::runtime(E_WORKSPACE, format!("无法创建目录 {}：{e}", dir.display()))
                    .with_hint("检查目录权限，或用 TTS_WORKSPACE_ROOT 指定其他位置")
            })?;
        }
        Ok(())
    }
}

/// 默认工作区目录：`~/.tts-workbench`。
pub fn default_root() -> Result<PathBuf> {
    dirs::home_dir()
        .map(|home| home.join(DEFAULT_DIR_NAME))
        .ok_or_else(|| {
            TtsError::runtime(E_WORKSPACE, "无法确定用户主目录")
                .with_hint("设置环境变量 TTS_WORKSPACE_ROOT 指定数据目录")
        })
}

fn default_root_with_override() -> Result<PathBuf> {
    let default = default_root()?;
    let cfg_path = default.join("config.json");
    if let Some(root) = peek_workspace_root(&cfg_path) {
        return expand_tilde(&root);
    }
    Ok(default)
}

/// 读取配置文件中 `workspace_root` 字段（容错，仅用于路径解析阶段）。
pub fn peek_workspace_root(config_path: &Path) -> Option<String> {
    let raw = fs::read_to_string(config_path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&raw).ok()?;
    value
        .get("workspace_root")
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// 展开开头的 `~` / `~/`。
pub fn expand_tilde(path: &str) -> Result<PathBuf> {
    if path == "~" {
        return default_root().map(|p| p.parent().map(Path::to_path_buf).unwrap_or(p));
    }
    if let Some(rest) = path.strip_prefix("~/") {
        let home = dirs::home_dir().ok_or_else(|| {
            TtsError::runtime(E_WORKSPACE, "无法确定用户主目录")
                .with_hint("使用绝对路径替代 ~ 写法")
        })?;
        return Ok(home.join(rest));
    }
    Ok(PathBuf::from(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_tilde_plain_path() {
        assert_eq!(expand_tilde("/tmp/tts").unwrap(), PathBuf::from("/tmp/tts"));
    }

    #[test]
    fn expand_tilde_home_prefix() {
        let home = dirs::home_dir().unwrap();
        assert_eq!(expand_tilde("~/x/y").unwrap(), home.join("x").join("y"));
    }

    #[test]
    fn peek_workspace_root_reads_field() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.json");
        fs::write(&path, r#"{"workspace_root": "/tmp/other"}"#).unwrap();
        assert_eq!(peek_workspace_root(&path).as_deref(), Some("/tmp/other"));

        fs::write(&path, "{ broken").unwrap();
        assert_eq!(peek_workspace_root(&path), None);

        fs::write(&path, r#"{"workspace_root": "  "}"#).unwrap();
        assert_eq!(peek_workspace_root(&path), None);
    }

    #[test]
    fn ensure_layout_creates_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("ws"),
            config_file: dir.path().join("ws/config.json"),
        };
        paths.ensure_layout().unwrap();
        assert!(paths.library_voices_dir().is_dir());
        assert!(paths.cache_dir().is_dir());
        assert!(paths.projects_dir().is_dir());
        assert!(paths.audit_file().parent().unwrap().is_dir());
    }
}
