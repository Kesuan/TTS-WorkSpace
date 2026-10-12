//! 通用小工具。

use chrono::{SecondsFormat, Utc};

/// UTC RFC 3339 时间戳（秒精度，形如 `2026-10-12T09:00:00Z`）。
pub fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}
