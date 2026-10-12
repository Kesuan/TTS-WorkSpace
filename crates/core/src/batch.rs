//! 批量生成：并发限流、退避重试、幂等续跑、事件流。
//!
//! 行为契约（详见 `docs/cli.md`）：
//! - 默认目标：状态非 `ok` 的非 removed 行（`force` 时包含 `ok` 行）；可按场景 / 行过滤；
//! - **fail-fast**：任一目标行未解析出音色 → `E_LINE_UNBOUND`（退出码 2），不做任何生成；
//! - 幂等：已完成（`ok`）的行跳过；相同参数命中缓存；
//! - 重试：网络 / 限流 / 5xx 错误按指数退避重试（上限见配置）；
//! - 部分失败：失败行标记 `failed`，命令以退出码 3（`E_PARTIAL`）结束。

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::{Mutex, Semaphore};
use tokio::task::JoinSet;

use crate::audit::{self, AuditRecord};
use crate::cache::Cache;
use crate::error::{
    Result, TtsError, E_HTTP_NETWORK, E_HTTP_RATE_LIMIT, E_HTTP_STATUS, E_IO, E_LINE_UNBOUND,
    E_NOT_FOUND,
};
use crate::project::{LinesFile, Project, STATUS_FAILED, STATUS_OK, STATUS_REMOVED};
use crate::provider::{AudioFormat, TtsProvider, VoiceRef};
use crate::synth::{self, SynthOptions, SynthOutcome};
use crate::voices::parse_voice_ref;

#[derive(Debug, Clone)]
pub struct BatchOptions {
    pub concurrency: usize,
    pub retry_max: u32,
    pub backoff_ms: u64,
    pub model: String,
    pub use_cache: bool,
    pub force: bool,
    pub scene: Option<String>,
    pub line_ids: Vec<String>,
}

/// 批量事件（JSONL 输出；`event` 字段为类型标签）。
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum BatchEvent {
    Started {
        total: usize,
        concurrency: usize,
        skipped: usize,
    },
    LineStarted {
        line_id: String,
        role: String,
    },
    LineSkipped {
        line_id: String,
        role: String,
        reason: String,
    },
    LineSucceeded {
        line_id: String,
        role: String,
        take_id: String,
        duration_ms: u64,
        latency_ms: u64,
        cached: bool,
    },
    LineFailed {
        line_id: String,
        role: String,
        code: String,
        message: String,
        attempts: u32,
    },
    Finished {
        succeeded: usize,
        failed: usize,
        skipped: usize,
        cached: usize,
        total_duration_ms: u64,
    },
}

pub type EventSink<'a> = &'a (dyn Fn(BatchEvent) + Send + Sync);

#[derive(Debug, Clone, Serialize)]
pub struct BatchSummary {
    pub total: usize,
    pub succeeded: usize,
    pub failed: usize,
    pub skipped: usize,
    pub cached: usize,
    pub concurrency: usize,
    pub total_duration_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct FailedLine {
    pub line_id: String,
    pub role: String,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct BatchOutcome {
    pub summary: BatchSummary,
    pub failed_lines: Vec<FailedLine>,
}

#[derive(Debug, Clone)]
struct Target {
    id: String,
    role: String,
    text: String,
    voice: VoiceRef,
    voice_spec: String,
    style: Option<String>,
}

#[derive(Debug, Default)]
pub struct Selection {
    targets: Vec<Target>,
    /// (line_id, role, reason)
    skipped: Vec<(String, String, String)>,
}

/// 选择目标行；未绑定音色的行会使整体 fail-fast。
pub fn select_targets(
    project: &Project,
    lines: &LinesFile,
    options: &BatchOptions,
) -> Result<Selection> {
    let explicit: Option<HashMap<&str, ()>> = if options.line_ids.is_empty() {
        None
    } else {
        Some(
            options
                .line_ids
                .iter()
                .map(|id| (id.as_str(), ()))
                .collect(),
        )
    };

    if let Some(explicit) = &explicit {
        let unknown: Vec<String> = explicit
            .keys()
            .filter(|id| {
                lines
                    .find(id)
                    .map(|line| line.status == STATUS_REMOVED)
                    .unwrap_or(true)
            })
            .map(|id| (*id).to_string())
            .collect();
        if !unknown.is_empty() {
            return Err(TtsError::runtime(
                E_NOT_FOUND,
                format!("以下行不存在或已移除：{}", unknown.join(", ")),
            )
            .with_hint("用 `tts project status` 查看有效行 ID"));
        }
    }

    let mut selection = Selection::default();
    let mut unbound: Vec<String> = Vec::new();
    let mut voice_cache: HashMap<String, VoiceRef> = HashMap::new();

    for line in &lines.lines {
        if line.status == STATUS_REMOVED {
            continue;
        }
        if let Some(explicit) = &explicit {
            if !explicit.contains_key(line.id.as_str()) {
                continue;
            }
        } else if let Some(scene) = &options.scene {
            if line.scene.as_deref() != Some(scene.as_str()) {
                continue;
            }
        }
        if !options.force && explicit.is_none() && line.status == STATUS_OK {
            selection
                .skipped
                .push((line.id.clone(), line.role.clone(), "already_ok".to_string()));
            continue;
        }
        let voice_spec = match project.resolve_voice(line) {
            Some(spec) if !spec.trim().is_empty() => spec,
            _ => {
                unbound.push(line.id.clone());
                continue;
            }
        };
        let voice = match voice_cache.get(&voice_spec) {
            Some(voice) => voice.clone(),
            None => {
                let parsed = parse_voice_ref(&voice_spec)?;
                voice_cache.insert(voice_spec.clone(), parsed.clone());
                parsed
            }
        };
        selection.targets.push(Target {
            id: line.id.clone(),
            role: line.role.clone(),
            text: line.text.clone(),
            voice,
            voice_spec,
            style: project.resolve_style(line),
        });
    }

    if !unbound.is_empty() {
        return Err(TtsError::usage(
            E_LINE_UNBOUND,
            format!("{} 条台词未绑定音色，无法批量生成", unbound.len()),
        )
        .with_hint("用 `tts role bind --role <角色> --voice <音色>` 绑定角色，或 `tts line set --line <ID> --voice <音色>` 逐行指定")
        .with_details(serde_json::json!({ "line_ids": unbound })));
    }

    Ok(selection)
}

/// 是否可重试：网络 / 限流 / 5xx。
pub fn is_retryable(error: &TtsError) -> bool {
    match error.code() {
        E_HTTP_NETWORK | E_HTTP_RATE_LIMIT => true,
        E_HTTP_STATUS => error
            .body
            .details
            .as_ref()
            .and_then(|details| details.get("status"))
            .and_then(|status| status.as_u64())
            .map(|status| status >= 500)
            .unwrap_or(false),
        _ => false,
    }
}

/// 指数退避：base × 2^(attempt-1)，封顶 30s。
pub fn backoff_delay(base_ms: u64, attempt: u32) -> Duration {
    let base = base_ms.max(1);
    let factor = 1u64 << attempt.saturating_sub(1).min(6);
    Duration::from_millis((base.saturating_mul(factor)).min(30_000))
}

struct TaskResult {
    target: Target,
    attempts: u32,
    result: Result<SynthOutcome>,
}

/// 执行批量生成。
pub async fn run(
    provider: Arc<dyn TtsProvider>,
    cache: Cache,
    project_root: &Path,
    audit_path: &Path,
    options: &BatchOptions,
    events: EventSink<'_>,
) -> Result<BatchOutcome> {
    let project = Project::open(project_root)?;
    let lines_file = project.load_lines()?;
    let Selection { targets, skipped } = select_targets(&project, &lines_file, options)?;
    let concurrency = options.concurrency.max(1);
    let started = Instant::now();

    events(BatchEvent::Started {
        total: targets.len(),
        concurrency,
        skipped: skipped.len(),
    });
    for (line_id, role, reason) in &skipped {
        events(BatchEvent::LineSkipped {
            line_id: line_id.clone(),
            role: role.clone(),
            reason: reason.clone(),
        });
    }

    let lines_state = Arc::new(Mutex::new(lines_file));
    let semaphore = Arc::new(Semaphore::new(concurrency));
    let mut join_set: JoinSet<TaskResult> = JoinSet::new();

    for target in targets {
        let permit = Arc::clone(&semaphore)
            .acquire_owned()
            .await
            .map_err(|e| TtsError::runtime(E_IO, format!("并发信号量异常：{e}")))?;
        events(BatchEvent::LineStarted {
            line_id: target.id.clone(),
            role: target.role.clone(),
        });
        let provider = Arc::clone(&provider);
        let cache = cache.clone();
        let options = options.clone();
        join_set.spawn(async move {
            let _permit = permit;
            let mut attempts = 0u32;
            loop {
                attempts += 1;
                let synth_options = SynthOptions {
                    model: options.model.clone(),
                    text: target.text.clone(),
                    voice: target.voice.clone(),
                    instructions: target.style.clone(),
                    format: AudioFormat::Wav,
                    stream: false,
                    use_cache: options.use_cache,
                };
                match synth::synthesize(provider.as_ref(), &cache, &synth_options).await {
                    Ok(outcome) => {
                        return TaskResult {
                            target,
                            attempts,
                            result: Ok(outcome),
                        };
                    }
                    Err(error) => {
                        if attempts <= options.retry_max && is_retryable(&error) {
                            tokio::time::sleep(backoff_delay(options.backoff_ms, attempts)).await;
                            continue;
                        }
                        return TaskResult {
                            target,
                            attempts,
                            result: Err(error),
                        };
                    }
                }
            }
        });
    }

    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut cached = 0usize;
    let mut failed_lines: Vec<FailedLine> = Vec::new();

    while let Some(joined) = join_set.join_next().await {
        let task = joined.map_err(|e| TtsError::runtime(E_IO, format!("批量任务异常：{e}")))?;
        let Target {
            id: line_id,
            role,
            voice_spec,
            text,
            ..
        } = &task.target;

        match &task.result {
            Ok(outcome) => {
                let take_id = {
                    let mut lines = lines_state.lock().await;
                    let (_, take_id) = project.append_take(
                        &mut lines,
                        line_id,
                        &outcome.audio,
                        &outcome.cache_key,
                        outcome.chars,
                        outcome.duration_ms,
                    )?;
                    project.save_lines(&lines)?;
                    take_id
                };
                if outcome.cached {
                    cached += 1;
                }
                succeeded += 1;

                let mut record = AuditRecord::ok("batch", &options.model);
                record.project = Some(project.root.display().to_string());
                record.line_id = Some(line_id.clone());
                record.voice = Some(voice_spec.clone());
                record.chars = outcome.chars;
                record.duration_ms = outcome.duration_ms;
                record.latency_ms = outcome.latency_ms;
                record.cached = outcome.cached;
                record.cost_note = Some("tts:free".to_string());
                audit::append(audit_path, &record)?;

                events(BatchEvent::LineSucceeded {
                    line_id: line_id.clone(),
                    role: role.clone(),
                    take_id,
                    duration_ms: outcome.duration_ms,
                    latency_ms: outcome.latency_ms,
                    cached: outcome.cached,
                });
            }
            Err(error) => {
                {
                    let mut lines = lines_state.lock().await;
                    if let Some(entry) = lines.find_mut(line_id) {
                        entry.status = STATUS_FAILED.to_string();
                    }
                    project.save_lines(&lines)?;
                }
                failed += 1;

                let mut record = AuditRecord::ok("batch", &options.model).into_failed(error.code());
                record.project = Some(project.root.display().to_string());
                record.line_id = Some(line_id.clone());
                record.voice = Some(voice_spec.clone());
                record.chars = text.chars().count();
                audit::append(audit_path, &record)?;

                failed_lines.push(FailedLine {
                    line_id: line_id.clone(),
                    role: role.clone(),
                    code: error.code().to_string(),
                    message: error.body.message.clone(),
                });
                events(BatchEvent::LineFailed {
                    line_id: line_id.clone(),
                    role: role.clone(),
                    code: error.code().to_string(),
                    message: error.body.message.clone(),
                    attempts: task.attempts,
                });
            }
        }
    }

    let summary = BatchSummary {
        total: succeeded + failed,
        succeeded,
        failed,
        skipped: skipped.len(),
        cached,
        concurrency,
        total_duration_ms: started.elapsed().as_millis() as u64,
    };
    events(BatchEvent::Finished {
        succeeded,
        failed,
        skipped: summary.skipped,
        cached,
        total_duration_ms: summary.total_duration_ms,
    });
    Ok(BatchOutcome {
        summary,
        failed_lines,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::E_USAGE;
    use crate::project::{LineEntry, Overrides, LINES_SCHEMA_VERSION, STATUS_PENDING};
    use crate::testing::FakeProvider;
    use std::sync::Mutex;

    fn line(id: &str, role: &str, text: &str, index: usize, status: &str) -> LineEntry {
        LineEntry {
            id: id.to_string(),
            chapter: None,
            scene: Some("雨夜".to_string()),
            index,
            role: role.to_string(),
            text: text.to_string(),
            status: status.to_string(),
            selected_take: None,
            overrides: Overrides::default(),
            takes: Vec::new(),
        }
    }

    struct Fixture {
        dir: tempfile::TempDir,
        project: Project,
        audit: std::path::PathBuf,
    }

    fn fixture(lines: Vec<LineEntry>, bindings: &[(&str, &str)]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let mut project = Project::create(&root, "批量测试").unwrap();
        for (role, voice) in bindings {
            project
                .bind_role(role, Some((*voice).to_string()), None)
                .unwrap();
        }
        project
            .save_lines(&LinesFile {
                schema_version: LINES_SCHEMA_VERSION,
                lines,
            })
            .unwrap();
        let audit = dir.path().join("audit/audit.jsonl");
        Fixture {
            dir,
            project,
            audit,
        }
    }

    fn options() -> BatchOptions {
        BatchOptions {
            concurrency: 2,
            retry_max: 0,
            backoff_ms: 0,
            model: "fake-tts".to_string(),
            use_cache: true,
            force: false,
            scene: None,
            line_ids: Vec::new(),
        }
    }

    #[test]
    fn select_fails_fast_on_unbound_lines() {
        let fx = fixture(vec![line("L1", "阿岚", "一", 1, STATUS_PENDING)], &[]);
        let lines = fx.project.load_lines().unwrap();
        let err = select_targets(&fx.project, &lines, &options()).unwrap_err();
        assert_eq!(err.code(), E_LINE_UNBOUND);
        assert_eq!(err.exit_code(), 2);
        assert_eq!(
            err.body.details.as_ref().unwrap()["line_ids"][0],
            serde_json::json!("L1")
        );
    }

    #[test]
    fn select_skips_ok_and_supports_filters() {
        let fx = fixture(
            vec![
                line("L1", "阿岚", "一", 1, STATUS_OK),
                line("L2", "阿岚", "二", 2, STATUS_PENDING),
                line("L3", "小满", "三", 3, STATUS_REMOVED),
            ],
            &[("阿岚", "冰糖"), ("小满", "苏打")],
        );
        let lines = fx.project.load_lines().unwrap();

        let selection = select_targets(&fx.project, &lines, &options()).unwrap();
        assert_eq!(selection.targets.len(), 1);
        assert_eq!(selection.targets[0].id, "L2");
        assert_eq!(selection.skipped.len(), 1);
        assert_eq!(selection.skipped[0].2, "already_ok");

        let mut forced = options();
        forced.force = true;
        assert_eq!(
            select_targets(&fx.project, &lines, &forced)
                .unwrap()
                .targets
                .len(),
            2
        );

        let mut by_line = options();
        by_line.line_ids = vec!["L1".to_string(), "L2".to_string()];
        assert_eq!(
            select_targets(&fx.project, &lines, &by_line)
                .unwrap()
                .targets
                .len(),
            2
        );

        by_line.line_ids = vec!["NOPE".to_string()];
        assert_eq!(
            select_targets(&fx.project, &lines, &by_line)
                .unwrap_err()
                .code(),
            E_NOT_FOUND
        );
    }

    #[tokio::test]
    async fn run_retries_then_succeeds_and_emits_events() {
        let fx = fixture(
            vec![
                line("L1", "阿岚", "一", 1, STATUS_PENDING),
                line("L2", "小满", "二", 2, STATUS_PENDING),
            ],
            &[("阿岚", "冰糖"), ("小满", "苏打")],
        );
        let provider = Arc::new(FakeProvider::failing_first(1, E_HTTP_RATE_LIMIT));
        let cache = Cache::new(fx.dir.path().join("cache"));
        let mut opts = options();
        opts.retry_max = 2;
        opts.concurrency = 1;

        let events = Mutex::new(Vec::<String>::new());
        let sink = |event: BatchEvent| {
            events
                .lock()
                .unwrap()
                .push(serde_json::to_string(&event).unwrap());
        };

        let outcome = run(
            provider.clone(),
            cache,
            &fx.project.root,
            &fx.audit,
            &opts,
            &sink,
        )
        .await
        .unwrap();

        assert_eq!(outcome.summary.succeeded, 2);
        assert_eq!(outcome.summary.failed, 0);
        assert_eq!(provider.plain_calls(), 3, "一次重试后成功");

        let lines = fx.project.load_lines().unwrap();
        assert!(lines
            .lines
            .iter()
            .all(|line| line.status == STATUS_OK && line.takes.len() == 1));

        let audit = crate::audit::read_all(&fx.audit).unwrap();
        assert_eq!(audit.len(), 2);
        assert!(audit.iter().all(|record| record.status == "ok"));

        let events = events.into_inner().unwrap();
        assert!(events.iter().any(|e| e.contains("\"event\":\"started\"")));
        assert_eq!(
            events
                .iter()
                .filter(|e| e.contains("line_succeeded"))
                .count(),
            2
        );
        assert!(events.last().unwrap().contains("\"event\":\"finished\""));
    }

    #[tokio::test]
    async fn run_marks_failed_line_and_second_run_skips_ok() {
        let fx = fixture(
            vec![
                line("L1", "阿岚", "一", 1, STATUS_PENDING),
                line("L2", "小满", "二", 2, STATUS_PENDING),
            ],
            &[("阿岚", "冰糖"), ("小满", "苏打")],
        );
        let cache = Cache::new(fx.dir.path().join("cache"));
        let mut opts = options();
        opts.concurrency = 1; // 顺序执行 → 第一条（L1）失败

        let sink = |_event: BatchEvent| {};
        let provider = Arc::new(FakeProvider::failing_first(1, E_HTTP_RATE_LIMIT));
        let outcome = run(
            provider,
            cache.clone(),
            &fx.project.root,
            &fx.audit,
            &opts,
            &sink,
        )
        .await
        .unwrap();

        assert_eq!(outcome.summary.failed, 1);
        assert_eq!(outcome.summary.succeeded, 1);
        assert_eq!(outcome.failed_lines.len(), 1);
        assert_eq!(outcome.failed_lines[0].code, E_HTTP_RATE_LIMIT);

        let lines = fx.project.load_lines().unwrap();
        assert_eq!(lines.lines[0].status, STATUS_FAILED);
        assert_eq!(lines.lines[1].status, STATUS_OK);

        // 第二次：跳过已 ok，只有失败行重新生成
        let provider = Arc::new(FakeProvider::new());
        let outcome = run(provider, cache, &fx.project.root, &fx.audit, &opts, &sink)
            .await
            .unwrap();
        assert_eq!(outcome.summary.succeeded, 1);
        assert_eq!(outcome.summary.skipped, 1);
    }

    #[tokio::test]
    async fn identical_lines_hit_cache() {
        let fx = fixture(
            vec![
                line("L1", "阿岚", "同一句话", 1, STATUS_PENDING),
                line("L2", "阿岚", "同一句话", 2, STATUS_PENDING),
            ],
            &[("阿岚", "冰糖")],
        );
        let provider = Arc::new(FakeProvider::new());
        let cache = Cache::new(fx.dir.path().join("cache"));
        let mut opts = options();
        opts.concurrency = 1;
        let sink = |_event: BatchEvent| {};

        let outcome = run(
            provider.clone(),
            cache,
            &fx.project.root,
            &fx.audit,
            &opts,
            &sink,
        )
        .await
        .unwrap();
        assert_eq!(outcome.summary.succeeded, 2);
        assert_eq!(outcome.summary.cached, 1);
        assert_eq!(provider.plain_calls(), 1, "第二条命中缓存");
    }

    #[test]
    fn retry_policy_is_sane() {
        assert_eq!(backoff_delay(100, 1), Duration::from_millis(100));
        assert_eq!(backoff_delay(100, 2), Duration::from_millis(200));
        assert_eq!(backoff_delay(100, 3), Duration::from_millis(400));
        assert_eq!(backoff_delay(1_000, 10), Duration::from_millis(30_000));

        assert!(is_retryable(&TtsError::runtime(E_HTTP_NETWORK, "x")));
        assert!(is_retryable(&TtsError::runtime(E_HTTP_RATE_LIMIT, "x")));
        assert!(is_retryable(
            &TtsError::runtime(E_HTTP_STATUS, "x").with_details(serde_json::json!({"status": 503}))
        ));
        assert!(!is_retryable(
            &TtsError::runtime(E_HTTP_STATUS, "x").with_details(serde_json::json!({"status": 400}))
        ));
        assert!(!is_retryable(&TtsError::usage(E_USAGE, "x")));
    }
}
