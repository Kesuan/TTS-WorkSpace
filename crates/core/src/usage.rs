//! 用量统计：聚合审计日志（请求数 / 字符数 / 音频时长 / 缓存命中率）。

use std::collections::BTreeMap;

use serde::Serialize;

use crate::audit::AuditRecord;

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageSummary {
    pub requests: u64,
    pub ok: u64,
    pub failed: u64,
    pub cached: u64,
    pub chars: u64,
    /// 生成音频总时长（毫秒；缓存命中的记录同样计入，作为“交付音频量”参考）。
    pub audio_duration_ms: u64,
    pub latency_ms_total: u64,
    pub cache_hit_rate: f64,
    pub by_cmd: BTreeMap<String, u64>,
    pub by_model: BTreeMap<String, u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_ts: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_ts: Option<String>,
    pub cost_note: String,
}

/// 聚合审计记录；`since` 为时间戳前缀过滤（如 `2026-10`、`2026-10-12`）。
pub fn summarize(records: &[AuditRecord], since: Option<&str>) -> UsageSummary {
    let mut summary = UsageSummary {
        cost_note: "tts:free（MiMo TTS 限时免费；ASR 计费未启用）".to_string(),
        ..UsageSummary::default()
    };

    for record in records {
        if let Some(prefix) = since {
            if !record.ts.starts_with(prefix) {
                continue;
            }
        }
        summary.requests += 1;
        match record.status.as_str() {
            "ok" => {
                summary.ok += 1;
                if record.cached {
                    summary.cached += 1;
                }
            }
            _ => summary.failed += 1,
        }
        summary.chars += record.chars as u64;
        summary.audio_duration_ms += record.duration_ms;
        summary.latency_ms_total += record.latency_ms;
        *summary.by_cmd.entry(record.cmd.clone()).or_insert(0) += 1;
        *summary.by_model.entry(record.model.clone()).or_insert(0) += 1;
        summary.first_ts = Some(match &summary.first_ts {
            Some(first) if first <= &record.ts => first.clone(),
            _ => record.ts.clone(),
        });
        summary.last_ts = Some(match &summary.last_ts {
            Some(last) if last >= &record.ts => last.clone(),
            _ => record.ts.clone(),
        });
    }

    if summary.ok > 0 {
        summary.cache_hit_rate = (summary.cached as f64) / (summary.ok as f64);
    }
    // 小数点保留三位，便于阅读
    summary.cache_hit_rate = (summary.cache_hit_rate * 1000.0).round() / 1000.0;
    summary
}

/// 统计项目 take 中引用的缓存键（供 `tts gc` 使用）。
pub fn referenced_cache_keys(
    lines: &crate::project::LinesFile,
) -> std::collections::HashSet<String> {
    lines
        .lines
        .iter()
        .flat_map(|line| line.takes.iter())
        .filter_map(|take| take.cache_key.clone())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::AuditRecord;

    fn record(status_ok: bool, cached: bool, chars: usize, ts: &str) -> AuditRecord {
        let mut record = AuditRecord::ok("batch", "mimo-v2.5-tts");
        record.ts = ts.to_string();
        record.cached = cached;
        record.chars = chars;
        record.duration_ms = 1_000;
        record.latency_ms = 100;
        if !status_ok {
            record = record.into_failed("E_HTTP_STATUS");
        }
        record
    }

    #[test]
    fn summarize_counts_and_rates() {
        let records = vec![
            record(true, false, 10, "2026-10-11T10:00:00Z"),
            record(true, true, 10, "2026-10-12T10:00:00Z"),
            record(false, false, 5, "2026-10-12T11:00:00Z"),
        ];
        let summary = summarize(&records, None);
        assert_eq!(summary.requests, 3);
        assert_eq!(summary.ok, 2);
        assert_eq!(summary.failed, 1);
        assert_eq!(summary.cached, 1);
        assert_eq!(summary.chars, 25);
        assert_eq!(summary.audio_duration_ms, 3_000);
        assert_eq!(summary.cache_hit_rate, 0.5);
        assert_eq!(summary.by_cmd.get("batch"), Some(&3));
        assert_eq!(summary.first_ts.as_deref(), Some("2026-10-11T10:00:00Z"));
        assert_eq!(summary.last_ts.as_deref(), Some("2026-10-12T11:00:00Z"));
    }

    #[test]
    fn since_filters_by_prefix() {
        let records = vec![
            record(true, false, 1, "2026-09-30T23:00:00Z"),
            record(true, false, 2, "2026-10-01T00:00:00Z"),
        ];
        let summary = summarize(&records, Some("2026-10"));
        assert_eq!(summary.requests, 1);
        assert_eq!(summary.chars, 2);
    }

    #[test]
    fn empty_records_yield_zero_summary() {
        let summary = summarize(&[], Some("2026-10"));
        assert_eq!(summary.requests, 0);
        assert_eq!(summary.cache_hit_rate, 0.0);
        assert!(summary.first_ts.is_none());
    }

    #[test]
    fn referenced_keys_are_collected() {
        use crate::project::{LineEntry, Overrides, TakeRef};
        let mut line = LineEntry {
            id: "L1".into(),
            chapter: None,
            scene: None,
            index: 1,
            role: "阿岚".into(),
            text: "x".into(),
            status: "ok".into(),
            selected_take: None,
            overrides: Overrides::default(),
            takes: Vec::new(),
        };
        line.takes.push(TakeRef {
            take_id: "0001".into(),
            file: "takes/L1/0001_abcd1234.wav".into(),
            params_hash: "abcd1234".into(),
            cache_key: Some("0123456789abcdef".into()),
            chars: 1,
            duration_ms: 1,
            created_at: "2026-10-12T00:00:00Z".into(),
        });
        line.takes.push(TakeRef {
            take_id: "0002".into(),
            file: "takes/L1/0002_deadbeef.wav".into(),
            params_hash: "deadbeef".into(),
            cache_key: None,
            chars: 1,
            duration_ms: 1,
            created_at: "2026-10-12T00:00:00Z".into(),
        });
        let lines = crate::project::LinesFile {
            schema_version: crate::project::LINES_SCHEMA_VERSION,
            lines: vec![line],
        };
        let keys = referenced_cache_keys(&lines);
        assert_eq!(keys.len(), 1);
        assert!(keys.contains("0123456789abcdef"));
    }
}
