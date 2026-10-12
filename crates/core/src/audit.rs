//! 审计日志：`<root>/audit/audit.jsonl`（append-only，每行一条 JSON）。
//!
//! 生成 / 导出 / 删除都会写入；`tts usage` 由该文件聚合（用量与成本核算的数据源）。

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{Result, TtsError, E_IO};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AuditRecord {
    pub ts: String,
    /// 产生该记录的命令（如 `synth`、`batch`、`export`）。
    pub cmd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line_id: Option<String>,
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(default)]
    pub chars: usize,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub latency_ms: u64,
    #[serde(default)]
    pub cached: bool,
    /// `ok` | `failed`
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_note: Option<String>,
}

impl AuditRecord {
    pub fn ok(cmd: &str, model: &str) -> Self {
        Self {
            ts: crate::util::now_rfc3339(),
            cmd: cmd.to_string(),
            project: None,
            line_id: None,
            model: model.to_string(),
            voice: None,
            chars: 0,
            duration_ms: 0,
            latency_ms: 0,
            cached: false,
            status: "ok".to_string(),
            error_code: None,
            cost_note: None,
        }
    }

    pub fn into_failed(mut self, error_code: &str) -> Self {
        self.status = "failed".to_string();
        self.error_code = Some(error_code.to_string());
        self
    }
}

/// 追加一条审计记录（文件与父目录自动创建）。
pub fn append(path: &Path, record: &AuditRecord) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            TtsError::runtime(E_IO, format!("无法创建审计目录 {}：{e}", parent.display()))
        })?;
    }
    let mut line = serde_json::to_string(record)?;
    line.push('\n');
    use std::io::Write as _;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| {
            TtsError::runtime(E_IO, format!("无法打开审计日志 {}：{e}", path.display()))
        })?;
    file.write_all(line.as_bytes()).map_err(|e| {
        TtsError::runtime(E_IO, format!("写入审计日志失败 {}：{e}", path.display()))
    })?;
    Ok(())
}

/// 读取全部审计记录（文件不存在返回空表；坏行跳过）。
pub fn read_all(path: &Path) -> Result<Vec<AuditRecord>> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).map_err(|e| {
        TtsError::runtime(E_IO, format!("无法读取审计日志 {}：{e}", path.display()))
    })?;
    let mut records = Vec::new();
    for line in raw.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<AuditRecord>(line) {
            records.push(record);
        }
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn append_and_read_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit/audit.jsonl");

        let mut ok = AuditRecord::ok("synth", "mimo-v2.5-tts");
        ok.chars = 12;
        ok.duration_ms = 1000;
        append(&path, &ok).unwrap();

        let failed = AuditRecord::ok("synth", "mimo-v2.5-tts").into_failed("E_HTTP_STATUS");
        append(&path, &failed).unwrap();

        let records = read_all(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].status, "ok");
        assert_eq!(records[0].chars, 12);
        assert_eq!(records[1].status, "failed");
        assert_eq!(records[1].error_code.as_deref(), Some("E_HTTP_STATUS"));
        assert!(records[0].ts.ends_with('Z'));
    }

    #[test]
    fn read_missing_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(read_all(&dir.path().join("nope.jsonl")).unwrap().is_empty());
    }

    #[test]
    fn bad_lines_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("audit.jsonl");
        std::fs::write(&path, "{ broken\n\n{\"ts\":\"x\"}\n").unwrap();
        assert!(read_all(&path).unwrap().is_empty(), "坏行与空行都应跳过");
    }
}
