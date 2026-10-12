//! 环境自检（`tts doctor`）：配置 / Key / ffmpeg / 网络连通性。
//!
//! 输出结构对应 `schemas/cli/doctor.schema.json`；
//! 任一检查 `fail` 时 CLI 以 `E_DOCTOR_UNHEALTHY`（退出码 1）结束，`fail` 明细在 `details` 中。

use std::path::PathBuf;

use serde::Serialize;

use crate::config::{mask_secret, Config, ENV_API_KEY};
use crate::mimo::MimoClient;
use crate::paths::Paths;
use crate::provider::TtsProvider;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CheckStatus {
    Ok,
    Warn,
    Fail,
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct Check {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Check {
    fn new(name: &str, status: CheckStatus, detail: impl Into<String>) -> Self {
        Self {
            name: name.to_string(),
            status,
            detail: detail.into(),
            hint: None,
        }
    }

    fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct Summary {
    pub ok: u32,
    pub warn: u32,
    pub fail: u32,
    pub skipped: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct DoctorReport {
    pub version: String,
    pub workspace_root: String,
    pub config_path: String,
    pub checks: Vec<Check>,
    pub summary: Summary,
}

impl DoctorReport {
    pub fn has_failure(&self) -> bool {
        self.summary.fail > 0
    }

    pub fn failed_names(&self) -> Vec<&str> {
        self.checks
            .iter()
            .filter(|c| c.status == CheckStatus::Fail)
            .map(|c| c.name.as_str())
            .collect()
    }
}

/// 执行全部检查（网络检查可用 `skip_network` 跳过）。
pub async fn run(paths: &Paths, skip_network: bool) -> DoctorReport {
    let mut checks = Vec::with_capacity(5);
    checks.push(check_workspace(paths));

    let (config_check, config) = check_config(paths);
    checks.push(config_check);
    let config = config.unwrap_or_default();

    checks.push(check_api_key(&config));
    checks.push(check_ffmpeg(&config));
    checks.push(check_network(&config, skip_network).await);

    let summary = Summary {
        ok: count(&checks, CheckStatus::Ok),
        warn: count(&checks, CheckStatus::Warn),
        fail: count(&checks, CheckStatus::Fail),
        skipped: count(&checks, CheckStatus::Skipped),
    };

    DoctorReport {
        version: crate::VERSION.to_string(),
        workspace_root: paths.root.display().to_string(),
        config_path: paths.config_file.display().to_string(),
        checks,
        summary,
    }
}

fn count(checks: &[Check], status: CheckStatus) -> u32 {
    checks.iter().filter(|c| c.status == status).count() as u32
}

fn check_workspace(paths: &Paths) -> Check {
    if let Err(e) = paths.ensure_layout() {
        return Check::new("workspace", CheckStatus::Fail, format!("工作区不可用：{e}"))
            .with_hint("检查目录权限，或用 TTS_WORKSPACE_ROOT 指定其他位置");
    }
    let probe = paths.root.join(".doctor-probe");
    match std::fs::write(&probe, b"ok").and_then(|_| std::fs::remove_file(&probe)) {
        Ok(()) => Check::new(
            "workspace",
            CheckStatus::Ok,
            format!("已就绪：{}", paths.root.display()),
        ),
        Err(e) => Check::new("workspace", CheckStatus::Fail, format!("目录不可写：{e}"))
            .with_hint("检查目录权限"),
    }
}

fn check_config(paths: &Paths) -> (Check, Option<Config>) {
    if !paths.config_file.exists() {
        return (
            Check::new(
                "config",
                CheckStatus::Warn,
                format!("未找到配置（使用默认值）：{}", paths.config_file.display()),
            )
            .with_hint("运行 `tts config set api.api_key <KEY>` 写入配置"),
            None,
        );
    }
    match Config::load(&paths.config_file) {
        Ok(cfg) => (
            Check::new(
                "config",
                CheckStatus::Ok,
                format!("已加载：{}", paths.config_file.display()),
            ),
            Some(cfg),
        ),
        Err(e) => (
            Check::new("config", CheckStatus::Fail, format!("加载失败：{e}"))
                .with_hint("修复或删除该文件后重试"),
            None,
        ),
    }
}

fn check_api_key(config: &Config) -> Check {
    match config.effective_api_key() {
        Some(key) => {
            let from_env = std::env::var(ENV_API_KEY)
                .map(|v| !v.trim().is_empty())
                .unwrap_or(false);
            let source = if from_env {
                "环境变量 MIMO_API_KEY"
            } else {
                "配置文件"
            };
            Check::new(
                "api_key",
                CheckStatus::Ok,
                format!("已配置（{source}，{}）", mask_secret(&key)),
            )
        }
        None => Check::new("api_key", CheckStatus::Warn, "未配置 API Key")
            .with_hint("运行 `tts config set api.api_key <KEY>`，或设置环境变量 MIMO_API_KEY"),
    }
}

fn check_ffmpeg(config: &Config) -> Check {
    match resolve_ffmpeg(config) {
        Some(path) => Check::new(
            "ffmpeg",
            CheckStatus::Ok,
            format!("已找到：{}", path.display()),
        ),
        None => Check::new(
            "ffmpeg",
            CheckStatus::Warn,
            "未找到 ffmpeg（mp3 转码 / 拼接 / 响度处理需要）",
        )
        .with_hint(
            "macOS: brew install ffmpeg；也可用 `tts config set ffmpeg.path <绝对路径>` 指定",
        ),
    }
}

async fn check_network(config: &Config, skip_network: bool) -> Check {
    if skip_network {
        return Check::new("network", CheckStatus::Skipped, "已跳过（--skip-network）");
    }
    if config.effective_api_key().is_none() {
        return Check::new(
            "network",
            CheckStatus::Skipped,
            "未配置 API Key，跳过连通性检查",
        );
    }
    match MimoClient::from_config(config) {
        Err(e) => Check::new(
            "network",
            CheckStatus::Fail,
            format!("客户端初始化失败：{e}"),
        ),
        Ok(client) => match client.list_models().await {
            Ok(models) => Check::new(
                "network",
                CheckStatus::Ok,
                format!("连通正常（{} 个可用模型）", models.len()),
            ),
            Err(e) => Check::new("network", CheckStatus::Fail, e.body.message.clone()).with_hint(
                e.body
                    .hint
                    .clone()
                    .unwrap_or_else(|| "检查网络与 `api.base_url` 配置".to_string()),
            ),
        },
    }
}

/// 解析 ffmpeg 路径：FFMPEG_PATH 环境变量 > 配置项 > PATH 查找。
pub fn resolve_ffmpeg(config: &Config) -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("FFMPEG_PATH") {
        let candidate = PathBuf::from(path);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    let configured = config.ffmpeg.path.trim();
    if !configured.is_empty() && configured != "auto" {
        let candidate = PathBuf::from(configured);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    which("ffmpeg")
}

fn which(bin: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let names = if cfg!(windows) {
        vec![format!("{bin}.exe"), bin.to_string()]
    } else {
        vec![bin.to_string()]
    };
    for dir in std::env::split_paths(&path) {
        for name in &names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> (tempfile::TempDir, Paths) {
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            root: dir.path().join("ws"),
            config_file: dir.path().join("ws").join("config.json"),
        };
        (dir, paths)
    }

    #[tokio::test]
    async fn report_without_config_has_no_failures() {
        let (_dir, paths) = temp_paths();
        let report = run(&paths, true).await;
        assert!(!report.has_failure());
        assert!(report.summary.warn >= 1, "缺少配置与 Key 应产生 warn");
        assert_eq!(report.summary.skipped, 1, "跳过网络检查");
        assert!(report.checks.iter().any(|c| c.name == "workspace"));
    }

    #[tokio::test]
    async fn broken_config_is_failure() {
        let (_dir, paths) = temp_paths();
        paths.ensure_layout().unwrap();
        std::fs::write(&paths.config_file, "{ broken").unwrap();
        let report = run(&paths, true).await;
        assert!(report.has_failure());
        assert_eq!(report.failed_names(), vec!["config"]);
    }

    #[test]
    fn ffmpeg_config_path_is_used() {
        let dir = tempfile::tempdir().unwrap();
        let fake = dir.path().join("ffmpeg");
        std::fs::write(&fake, b"#!/bin/sh\n").unwrap();
        let mut config = Config::default();
        config
            .set_field("ffmpeg.path", &fake.display().to_string())
            .unwrap();
        assert_eq!(resolve_ffmpeg(&config), Some(fake));
    }

    #[test]
    fn missing_ffmpeg_config_falls_back_to_path_lookup() {
        let mut config = Config::default();
        config
            .set_field("ffmpeg.path", "/nonexistent/ffmpeg")
            .unwrap();
        // PATH 中可能找得到也可能找不到——只要不 panic 即可；这里验证“不存在路径”不会直接返回它
        if let Some(found) = resolve_ffmpeg(&config) {
            assert_ne!(found, PathBuf::from("/nonexistent/ffmpeg"));
        }
    }
}
