//! Tauri 命令层：全部业务逻辑复用 `tts-core`（与 CLI 同一实现、同一数据目录）。
//!
//! 约定：命令返回 `Result<Value, Value>`——失败时 `Err` 为错误对象
//! `{code, message, hint?, details?}`（与 CLI 的 `error` 字段同构）。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::ipc::Channel;

use tts_core::audit::{self, AuditRecord};
use tts_core::batch::{self, BatchEvent, BatchOptions};
use tts_core::cache::Cache;
use tts_core::config::Config;
use tts_core::doctor;
use tts_core::error::{TtsError, E_IO, E_NOT_FOUND, E_PARTIAL, E_USAGE};
use tts_core::export::{self, ExportFormat, ExportOptions, Layout, DEFAULT_NAME_TEMPLATE};
use tts_core::library::{self, Library, VoiceEntry, KIND_CLONE, KIND_DESIGN};
use tts_core::mimo::MimoClient;
use tts_core::paths::Paths;
use tts_core::project::{Project, STATUS_FAILED, STATUS_OK, STATUS_PENDING, STATUS_REMOVED};
use tts_core::provider::{AudioFormat, VoiceRef};
use tts_core::script::ParseOptions;
use tts_core::synth::{self, SynthOptions};
use tts_core::usage;
use tts_core::util::now_rfc3339;

type CmdResult = Result<Value, Value>;

/// 将 `TtsError` 映射为前端友好的错误对象。
fn err_value(error: TtsError) -> Value {
    json!({
        "code": error.code(),
        "message": error.body.message,
        "hint": error.body.hint,
        "details": error.body.details,
    })
}

trait Cer<T> {
    fn cerr(self) -> Result<T, Value>;
}

impl<T> Cer<T> for Result<T, TtsError> {
    fn cerr(self) -> Result<T, Value> {
        self.map_err(err_value)
    }
}

fn library_of(paths: &Paths) -> Library {
    Library::new(paths.library_voices_dir())
}

// ── 配置 / 自检 ─────────────────────────────────────────────────────

#[tauri::command]
async fn config_get(reveal: bool) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let config = Config::load(&paths.config_file).cerr()?;
    let from_env = std::env::var(tts_core::config::ENV_API_KEY)
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false);
    Ok(json!({
        "config_path": paths.config_file.display().to_string(),
        "workspace_root": paths.root.display().to_string(),
        "api_key_source": if from_env { "env" } else if config.api.api_key.is_some() { "config" } else { "missing" },
        "config": config.to_json_value(reveal),
    }))
}

#[tauri::command]
async fn config_set(key: String, value: String) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let mut config = Config::load(&paths.config_file).cerr()?;
    config.set_field(&key, &value).cerr()?;
    config.save(&paths.config_file).cerr()?;
    paths.ensure_layout().cerr()?;
    Ok(json!({
        "config_path": paths.config_file.display().to_string(),
        "config": config.to_json_value(false),
    }))
}

#[tauri::command]
async fn doctor_run(skip_network: bool) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let report = doctor::run(&paths, skip_network).await;
    Ok(serde_json::to_value(&report).unwrap_or(Value::Null))
}

// ── 项目 ────────────────────────────────────────────────────────────

fn project_counts(project: &Project) -> Result<Value, TtsError> {
    let lines = project.load_lines()?;
    let count = |status: &str| {
        lines
            .lines
            .iter()
            .filter(|line| line.status == status)
            .count()
    };
    Ok(json!({
        "total": lines.lines.len(),
        "pending": count(STATUS_PENDING),
        "ok": count(STATUS_OK),
        "failed": count(STATUS_FAILED),
        "removed": count(STATUS_REMOVED),
    }))
}

#[tauri::command]
async fn project_list() -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let mut projects: Vec<Value> = Vec::new();
    let root = paths.projects_dir();
    if root.is_dir() {
        let mut children: Vec<PathBuf> = std::fs::read_dir(&root)
            .map_err(|e| err_value(TtsError::runtime(E_IO, format!("读取项目目录失败：{e}"))))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join("lines.json").is_file())
            .collect();
        children.sort();
        for dir in children {
            let project = Project::open(&dir).cerr()?;
            let counts = project_counts(&project).cerr()?;
            projects.push(json!({
                "path": project.root.display().to_string(),
                "name": project.meta.name,
                "scripts": project.meta.scripts,
                "counts": counts,
            }));
        }
    }
    Ok(json!({ "projects_dir": root.display().to_string(), "projects": projects }))
}

#[tauri::command]
async fn project_init(path: String, name: String) -> CmdResult {
    let project = Project::create(Path::new(&path), &name).cerr()?;
    Ok(json!({
        "path": project.root.display().to_string(),
        "name": project.meta.name,
    }))
}

#[tauri::command]
async fn project_import(
    project: String,
    script: String,
    narrator_fallback: bool,
    join_with_space: bool,
) -> CmdResult {
    let mut project = Project::open(Path::new(&project)).cerr()?;
    let options = ParseOptions {
        narrator_fallback,
        join_with_space,
    };
    let outcome = project.import_script(Path::new(&script), &options).cerr()?;
    Ok(serde_json::to_value(&outcome).unwrap_or(Value::Null))
}

#[tauri::command]
async fn project_detail(project: String) -> CmdResult {
    let project = Project::open(Path::new(&project)).cerr()?;
    let lines = project.load_lines().cerr()?;

    let mut roles: Vec<Value> = Vec::new();
    for line in &lines.lines {
        if line.status == STATUS_REMOVED {
            continue;
        }
        let name_value = Value::String(line.role.clone());
        if roles.iter().any(|role| role["name"] == name_value) {
            continue;
        }
        let binding = project.meta.roles.get(&line.role);
        roles.push(json!({
            "name": line.role,
            "bound": binding.map(|b| b.voice.is_some()).unwrap_or(false),
            "voice": binding.and_then(|b| b.voice.clone()),
            "style": binding.and_then(|b| b.style.clone()),
        }));
    }

    let mut entries: Vec<Value> = Vec::new();
    for line in &lines.lines {
        let takes: Vec<Value> = line
            .takes
            .iter()
            .map(|take| {
                let abs = project.root.join(&take.file);
                json!({
                    "take_id": take.take_id,
                    "file": take.file,
                    "abs_file": abs.display().to_string(),
                    "duration_ms": take.duration_ms,
                    "chars": take.chars,
                    "created_at": take.created_at,
                    "selected": line.selected_take.as_deref() == Some(take.take_id.as_str()),
                })
            })
            .collect();
        entries.push(json!({
            "id": line.id,
            "chapter": line.chapter,
            "scene": line.scene,
            "index": line.index,
            "role": line.role,
            "text": line.text,
            "status": line.status,
            "selected_take": line.selected_take,
            "overrides": {
                "voice": line.overrides.voice,
                "style": line.overrides.style,
            },
            "effective_voice": project.resolve_voice(line),
            "effective_style": project.resolve_style(line),
            "takes": takes,
        }));
    }

    Ok(json!({
        "path": project.root.display().to_string(),
        "name": project.meta.name,
        "scripts": project.meta.scripts,
        "counts": project_counts(&project).cerr()?,
        "roles": roles,
        "lines": entries,
    }))
}

#[tauri::command]
async fn role_bind(
    project: String,
    role: String,
    voice: Option<String>,
    style: Option<String>,
) -> CmdResult {
    if voice.is_none() && style.is_none() {
        return Err(err_value(TtsError::usage(
            E_USAGE,
            "至少需要音色或风格之一",
        )));
    }
    let paths = Paths::resolve().cerr()?;
    if let Some(spec) = voice.as_deref() {
        library_of(&paths).resolve(spec).cerr()?;
    }
    let mut project = Project::open(Path::new(&project)).cerr()?;
    project
        .bind_role(role.trim(), voice.clone(), style.clone())
        .cerr()?;
    let binding = project
        .meta
        .roles
        .get(role.trim())
        .cloned()
        .unwrap_or_default();
    Ok(json!({
        "role": role.trim(),
        "voice": binding.voice,
        "style": binding.style,
    }))
}

#[tauri::command]
async fn line_set(
    project: String,
    line_id: String,
    voice: Option<String>,
    style: Option<String>,
    clear_voice: bool,
    clear_style: bool,
) -> CmdResult {
    if voice.is_none() && style.is_none() && !clear_voice && !clear_style {
        return Err(err_value(TtsError::usage(E_USAGE, "未指定任何修改")));
    }
    let paths = Paths::resolve().cerr()?;
    if let Some(spec) = voice.as_deref() {
        library_of(&paths).resolve(spec).cerr()?;
    }
    let project = Project::open(Path::new(&project)).cerr()?;
    let mut lines = project.load_lines().cerr()?;
    {
        let entry = lines.find_mut(&line_id).ok_or_else(|| {
            err_value(TtsError::runtime(
                E_NOT_FOUND,
                format!("未找到行：{line_id}"),
            ))
        })?;
        if entry.status == STATUS_REMOVED {
            return Err(err_value(TtsError::usage(
                E_USAGE,
                format!("行已被标记 removed：{line_id}"),
            )));
        }
        if clear_voice {
            entry.overrides.voice = None;
        }
        if clear_style {
            entry.overrides.style = None;
        }
        if let Some(value) = voice {
            entry.overrides.voice = Some(value);
        }
        if let Some(value) = style {
            entry.overrides.style = Some(value);
        }
    }
    project.save_lines(&lines).cerr()?;
    let entry = lines.find(&line_id).cloned();
    Ok(json!({
        "line_id": line_id,
        "overrides": {
            "voice": entry.as_ref().and_then(|e| e.overrides.voice.clone()),
            "style": entry.as_ref().and_then(|e| e.overrides.style.clone()),
        },
    }))
}

#[tauri::command]
async fn takes_select(project: String, line_id: String, take_id: String) -> CmdResult {
    let project = Project::open(Path::new(&project)).cerr()?;
    let mut lines = project.load_lines().cerr()?;
    {
        let entry = lines.find_mut(&line_id).ok_or_else(|| {
            err_value(TtsError::runtime(
                E_NOT_FOUND,
                format!("未找到行：{line_id}"),
            ))
        })?;
        if !entry.takes.iter().any(|take| take.take_id == take_id) {
            return Err(err_value(TtsError::runtime(
                E_NOT_FOUND,
                format!("行 {line_id} 没有 take {take_id}"),
            )));
        }
        entry.selected_take = Some(take_id.clone());
    }
    project.save_lines(&lines).cerr()?;
    Ok(json!({ "line_id": line_id, "take_id": take_id }))
}

// ── 生成 ────────────────────────────────────────────────────────────

/// 单行生成（GUI 的“重新生成”按钮；等价 CLI 的 `synth --line`）。
#[tauri::command]
async fn synth_line(
    project: String,
    line_id: String,
    voice: Option<String>,
    style: Option<String>,
    no_cache: bool,
) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let config = Config::load(&paths.config_file).cerr()?;
    paths.ensure_layout().cerr()?;

    let project = Project::open(Path::new(&project)).cerr()?;
    let mut lines = project.load_lines().cerr()?;
    let (text, voice_spec, resolved_style) = {
        let entry = lines.find(&line_id).ok_or_else(|| {
            err_value(TtsError::runtime(
                E_NOT_FOUND,
                format!("未找到行：{line_id}"),
            ))
        })?;
        if entry.status == STATUS_REMOVED {
            return Err(err_value(TtsError::usage(
                E_USAGE,
                format!("行已被标记 removed：{line_id}"),
            )));
        }
        let voice_spec = voice
            .clone()
            .or_else(|| project.resolve_voice(entry))
            .ok_or_else(|| {
                err_value(
                    TtsError::usage(E_USAGE, "该行未绑定音色")
                        .with_hint("先在「角色绑定」或行级覆盖中指定音色"),
                )
            })?;
        (
            entry.text.clone(),
            voice_spec,
            style.clone().or_else(|| project.resolve_style(entry)),
        )
    };

    let library = library_of(&paths);
    let voice_ref = library.resolve(&voice_spec).cerr()?;
    let model = config.models.for_voice(&voice_ref);
    let options = SynthOptions {
        model: model.clone(),
        text: text.clone(),
        voice: voice_ref,
        instructions: resolved_style.clone(),
        format: AudioFormat::Wav,
        stream: false,
        use_cache: !no_cache,
        optimize_text_preview: false,
    };

    let client = MimoClient::from_config(&config).cerr()?;
    let cache = Cache::new(paths.cache_dir());
    let outcome = match synth::synthesize(&client, &cache, &options).await {
        Ok(outcome) => outcome,
        Err(error) => {
            let mut record = AuditRecord::ok("synth", &model).into_failed(error.code());
            record.project = Some(project.root.display().to_string());
            record.line_id = Some(line_id.clone());
            record.voice = Some(voice_spec.clone());
            record.chars = text.chars().count();
            let _ = audit::append(&paths.audit_file(), &record);
            return Err(err_value(error));
        }
    };

    let (_, take_id) = project
        .append_take(
            &mut lines,
            &line_id,
            &outcome.audio,
            &outcome.cache_key,
            outcome.chars,
            outcome.duration_ms,
        )
        .cerr()?;
    project.save_lines(&lines).cerr()?;

    let mut record = AuditRecord::ok("synth", &outcome.model);
    record.project = Some(project.root.display().to_string());
    record.line_id = Some(line_id.clone());
    record.voice = Some(voice_spec.clone());
    record.chars = outcome.chars;
    record.duration_ms = outcome.duration_ms;
    record.latency_ms = outcome.latency_ms;
    record.cached = outcome.cached;
    record.cost_note = Some("tts:free".to_string());
    audit::append(&paths.audit_file(), &record).cerr()?;

    Ok(json!({
        "line_id": line_id,
        "take_id": take_id,
        "voice": voice_spec,
        "model": outcome.model,
        "duration_ms": outcome.duration_ms,
        "latency_ms": outcome.latency_ms,
        "cached": outcome.cached,
    }))
}

/// 批量生成；进度通过 `on_event` 通道推送（事件与 CLI 的 `--events jsonl` 同构）。
#[tauri::command]
async fn batch_run(
    project: String,
    concurrency: Option<u32>,
    scene: Option<String>,
    line_ids: Vec<String>,
    force: bool,
    on_event: Channel<BatchEvent>,
) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let config = Config::load(&paths.config_file).cerr()?;
    paths.ensure_layout().cerr()?;

    let options = BatchOptions {
        concurrency: concurrency.unwrap_or(config.generation.concurrency).max(1) as usize,
        retry_max: config.generation.retry.max,
        backoff_ms: config.generation.retry.backoff_ms,
        model: config.models.tts.clone(),
        use_cache: true,
        force,
        scene,
        line_ids,
        library_dir: Some(paths.library_voices_dir()),
        models: Some(config.models.clone()),
    };
    if options.concurrency == 0 {
        return Err(err_value(TtsError::usage(E_USAGE, "并发数至少为 1")));
    }

    let project = Project::open(Path::new(&project)).cerr()?;
    let client = Arc::new(MimoClient::from_config(&config).cerr()?);
    let cache = Cache::new(paths.cache_dir());
    let sink = move |event: BatchEvent| {
        let _ = on_event.send(event);
    };

    let outcome = batch::run(
        client,
        cache,
        &project.root,
        &paths.audit_file(),
        &options,
        &sink,
    )
    .await;

    match outcome {
        Ok(outcome) => Ok(json!({
            "summary": outcome.summary,
            "failed_lines": outcome.failed_lines,
        })),
        Err(error) => {
            let code = error.code().to_string();
            let mut value = err_value(error);
            if code == E_PARTIAL {
                // 部分失败在 CLI 中也是错误信封（退出码 3）；GUI 用 code 判断即可
                value["partial"] = json!(true);
            }
            Err(value)
        }
    }
}

// ── 导出 / 用量 / 缓存 ──────────────────────────────────────────────

#[tauri::command]
#[allow(clippy::too_many_arguments)]
async fn export_run(
    project: String,
    format: String,
    layout: String,
    out: Option<String>,
    name_template: Option<String>,
    concat: bool,
    normalize: bool,
    trim_silence: bool,
    overwrite: bool,
) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let config = Config::load(&paths.config_file).cerr()?;
    let project = Project::open(Path::new(&project)).cerr()?;

    let out_dir = match out {
        Some(path) if !path.trim().is_empty() => PathBuf::from(path),
        _ => unique_export_dir(&project.exports_dir()),
    };

    let options = ExportOptions {
        out_dir,
        format: ExportFormat::parse(&format).cerr()?,
        layout: Layout::parse(&layout).cerr()?,
        name_template: name_template.unwrap_or_else(|| DEFAULT_NAME_TEMPLATE.to_string()),
        concat,
        normalize,
        trim_silence,
        overwrite,
        line_ids: Vec::new(),
        scene: None,
        ffmpeg: doctor::resolve_ffmpeg(&config),
    };

    let outcome = export::run(&project, &options).cerr()?;
    let mut record = AuditRecord::ok("export", "export");
    record.project = Some(project.root.display().to_string());
    record.chars = outcome.items.iter().map(|item| item.chars).sum();
    record.duration_ms = outcome.items.iter().map(|item| item.duration_ms).sum();
    record.cost_note = Some("export:local".to_string());
    audit::append(&paths.audit_file(), &record).cerr()?;

    Ok(json!({
        "out_dir": outcome.out_dir.display().to_string(),
        "format": outcome.format,
        "layout": outcome.layout,
        "manifest": outcome.manifest.display().to_string(),
        "concat": outcome.concat.as_ref().map(|path| path.display().to_string()),
        "ffmpeg_used": outcome.ffmpeg_used,
        "count": outcome.items.len(),
        "items": outcome.items,
    }))
}

fn unique_export_dir(base: &Path) -> PathBuf {
    let raw = now_rfc3339();
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    let stamp = if digits.len() >= 14 {
        format!("{}-{}", &digits[..8], &digits[8..14])
    } else {
        digits
    };
    let candidate = base.join(&stamp);
    if !candidate.exists() {
        return candidate;
    }
    for index in 2..1000 {
        let next = base.join(format!("{stamp}-{index}"));
        if !next.exists() {
            return next;
        }
    }
    candidate
}

#[tauri::command]
async fn usage_get(since: Option<String>) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let records = audit::read_all(&paths.audit_file()).cerr()?;
    let summary = usage::summarize(&records, since.as_deref());
    Ok(json!({
        "audit_file": paths.audit_file().display().to_string(),
        "since": since,
        "summary": summary,
    }))
}

#[tauri::command]
async fn gc_run(dry_run: bool) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    paths.ensure_layout().cerr()?;

    let mut project_dirs: Vec<PathBuf> = Vec::new();
    let default_root = paths.projects_dir();
    if default_root.is_dir() {
        let mut children: Vec<PathBuf> = std::fs::read_dir(&default_root)
            .map_err(|e| err_value(TtsError::runtime(E_IO, format!("读取项目目录失败：{e}"))))?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join("lines.json").is_file())
            .collect();
        children.sort();
        project_dirs.extend(children);
    }

    let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut scanned = 0usize;
    for dir in &project_dirs {
        let project = Project::open(dir).cerr()?;
        let lines = project.load_lines().cerr()?;
        referenced.extend(usage::referenced_cache_keys(&lines));
        scanned += 1;
    }

    let cache = Cache::new(paths.cache_dir());
    let (removed, freed_bytes, kept) = if dry_run {
        let mut removed = 0u64;
        let mut freed = 0u64;
        let mut kept = 0u64;
        for key in cache.keys().cerr()? {
            if referenced.contains(&key) {
                kept += 1;
            } else {
                removed += 1;
                freed += tts_core::cache::directory_size(&cache.dir_for(&key));
            }
        }
        (removed, freed, kept)
    } else {
        let report = cache.gc(&referenced).cerr()?;
        (report.removed, report.freed_bytes, report.kept)
    };

    Ok(json!({
        "cache_dir": cache.root().display().to_string(),
        "scanned_projects": scanned,
        "referenced_keys": referenced.len(),
        "removed": removed,
        "freed_bytes": freed_bytes,
        "kept": kept,
        "dry_run": dry_run,
    }))
}

// ── 音色库 ──────────────────────────────────────────────────────────

#[tauri::command]
async fn voice_list(kind: Option<String>) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    paths.ensure_layout().cerr()?;
    library::list_json(&library_of(&paths), kind.as_deref()).cerr()
}

#[tauri::command]
async fn voice_remove(name: String) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let library = library_of(&paths);
    let entry = library.remove(&name).cerr()?;
    Ok(json!({ "id": entry.id, "name": entry.name, "kind": entry.kind }))
}

#[tauri::command]
async fn design_create(
    prompt: String,
    text: String,
    name: Option<String>,
    optimize_text_preview: bool,
) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let config = Config::load(&paths.config_file).cerr()?;
    paths.ensure_layout().cerr()?;

    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return Err(err_value(TtsError::usage(E_USAGE, "音色描述不能为空")));
    }
    let model = config.models.design.clone();
    let options = SynthOptions {
        model: model.clone(),
        text: text.clone(),
        voice: VoiceRef::Design {
            prompt: prompt.clone(),
        },
        instructions: None,
        format: AudioFormat::Wav,
        stream: false,
        use_cache: true,
        optimize_text_preview,
    };
    let client = MimoClient::from_config(&config).cerr()?;
    let cache = Cache::new(paths.cache_dir());
    let outcome = synth::synthesize(&client, &cache, &options).await.cerr()?;

    let output = paths
        .root
        .join("synth")
        .join(format!("{}.wav", outcome.cache_key));
    if let Some(parent) = output.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| err_value(TtsError::runtime(E_IO, format!("创建目录失败：{e}"))))?;
    }
    std::fs::write(&output, &outcome.audio)
        .map_err(|e| err_value(TtsError::runtime(E_IO, format!("写入音频失败：{e}"))))?;

    let library = library_of(&paths);
    let saved = match name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
    {
        Some(name) => {
            if library.find(name).cerr()?.is_some() {
                return Err(err_value(TtsError::usage(
                    E_USAGE,
                    format!("音色库已存在同名音色：{name}"),
                )));
            }
            let entry = VoiceEntry {
                id: library::make_voice_id(name, KIND_DESIGN, &prompt),
                name: name.to_string(),
                kind: KIND_DESIGN.to_string(),
                model: model.clone(),
                design_prompt: Some(prompt.clone()),
                ..VoiceEntry::default()
            };
            let record = library.create(entry, Some(&output)).cerr()?;
            Some(json!({ "id": record.entry.id, "name": record.entry.name }))
        }
        None => None,
    };

    let mut record = AuditRecord::ok("design", &outcome.model);
    record.voice = saved
        .as_ref()
        .and_then(|voice| voice["id"].as_str().map(str::to_string));
    record.chars = outcome.chars;
    record.duration_ms = outcome.duration_ms;
    record.latency_ms = outcome.latency_ms;
    record.cached = outcome.cached;
    record.cost_note = Some("tts:free".to_string());
    audit::append(&paths.audit_file(), &record).cerr()?;

    Ok(json!({
        "output": output.display().to_string(),
        "duration_ms": outcome.duration_ms,
        "latency_ms": outcome.latency_ms,
        "cached": outcome.cached,
        "voice": saved,
    }))
}

#[tauri::command]
async fn design_solidify(from: String, name: String) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let config = Config::load(&paths.config_file).cerr()?;
    paths.ensure_layout().cerr()?;
    let library = library_of(&paths);

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(err_value(TtsError::usage(E_USAGE, "音色名不能为空")));
    }
    if library.find(&name).cerr()?.is_some() {
        return Err(err_value(TtsError::usage(
            E_USAGE,
            format!("音色库已存在同名音色：{name}"),
        )));
    }

    let (sample_path, source_voice) = match library.find(&from).cerr()? {
        Some(record) => {
            if record.entry.kind != KIND_DESIGN {
                return Err(err_value(TtsError::usage(
                    E_USAGE,
                    format!("「{}」不是 design 音色", record.entry.name),
                )));
            }
            let sample = record.sample_path().ok_or_else(|| {
                err_value(TtsError::runtime(
                    E_NOT_FOUND,
                    format!("design 音色「{}」没有留档样本", record.entry.name),
                ))
            })?;
            (sample, Some(record.entry.id.clone()))
        }
        None => {
            let path = PathBuf::from(&from);
            if !path.is_file() {
                return Err(err_value(TtsError::runtime(
                    E_NOT_FOUND,
                    format!("未找到音色或音频文件：{from}"),
                )));
            }
            (path, None)
        }
    };

    library::validate_sample(&sample_path).cerr()?;
    let entry = VoiceEntry {
        id: library::make_voice_id(&name, KIND_CLONE, &sample_path.display().to_string()),
        name: name.clone(),
        kind: KIND_CLONE.to_string(),
        model: config.models.clone.clone(),
        source_voice: source_voice.clone(),
        ..VoiceEntry::default()
    };
    let record = library.create(entry, Some(&sample_path)).cerr()?;

    let mut audit_record = AuditRecord::ok("solidify", &record.entry.model);
    audit_record.voice = Some(record.entry.id.clone());
    audit_record.cost_note = Some("local".to_string());
    audit::append(&paths.audit_file(), &audit_record).cerr()?;

    Ok(json!({
        "id": record.entry.id,
        "name": record.entry.name,
        "kind": record.entry.kind,
        "source_voice": source_voice,
    }))
}

#[tauri::command]
async fn clone_create(sample: String, name: String) -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    let config = Config::load(&paths.config_file).cerr()?;
    paths.ensure_layout().cerr()?;
    let library = library_of(&paths);

    let name = name.trim().to_string();
    if name.is_empty() {
        return Err(err_value(TtsError::usage(E_USAGE, "音色名不能为空")));
    }
    if library.find(&name).cerr()?.is_some() {
        return Err(err_value(TtsError::usage(
            E_USAGE,
            format!("音色库已存在同名音色：{name}"),
        )));
    }
    let sample_path = PathBuf::from(&sample);
    library::validate_sample(&sample_path).cerr()?;

    let entry = VoiceEntry {
        id: library::make_voice_id(&name, KIND_CLONE, &sample_path.display().to_string()),
        name: name.clone(),
        kind: KIND_CLONE.to_string(),
        model: config.models.clone.clone(),
        ..VoiceEntry::default()
    };
    let record = library.create(entry, Some(&sample_path)).cerr()?;

    let mut audit_record = AuditRecord::ok("clone", &record.entry.model);
    audit_record.voice = Some(record.entry.id.clone());
    audit_record.cost_note = Some("local".to_string());
    audit::append(&paths.audit_file(), &audit_record).cerr()?;

    Ok(json!({ "id": record.entry.id, "name": record.entry.name }))
}

// ── 工具 ────────────────────────────────────────────────────────────

/// 在系统文件管理器中显示路径（macOS: Finder）。
#[tauri::command]
async fn reveal_path(path: String) -> CmdResult {
    let target = PathBuf::from(&path);
    if !target.exists() {
        return Err(err_value(TtsError::runtime(
            E_NOT_FOUND,
            format!("路径不存在：{path}"),
        )));
    }
    #[cfg(target_os = "macos")]
    let status = std::process::Command::new("open")
        .args(["-R", &path])
        .status();
    #[cfg(target_os = "windows")]
    let status = std::process::Command::new("explorer").arg(&path).status();
    #[cfg(all(unix, not(target_os = "macos")))]
    let status = std::process::Command::new("xdg-open").arg(&path).status();

    match status {
        Ok(_) => Ok(json!({ "path": path })),
        Err(e) => Err(err_value(TtsError::runtime(
            E_IO,
            format!("无法打开文件管理器：{e}"),
        ))),
    }
}

/// 播放兜底：返回可被前端 `<audio>` 使用的文件路径（配合 asset 协议）。
#[tauri::command]
async fn resolve_audio_path(path: String) -> CmdResult {
    if !Path::new(&path).is_file() {
        return Err(err_value(TtsError::runtime(
            E_NOT_FOUND,
            format!("音频文件不存在：{path}"),
        )));
    }
    Ok(json!({ "path": path }))
}

/// 环境自检 + 版本信息（GUI 启动时调用）。
#[tauri::command]
async fn app_info() -> CmdResult {
    let paths = Paths::resolve().cerr()?;
    Ok(json!({
        "version": tts_core::VERSION,
        "workspace_root": paths.root.display().to_string(),
        "config_path": paths.config_file.display().to_string(),
    }))
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            app_info,
            config_get,
            config_set,
            doctor_run,
            project_list,
            project_init,
            project_import,
            project_detail,
            role_bind,
            line_set,
            takes_select,
            synth_line,
            batch_run,
            export_run,
            usage_get,
            gc_run,
            voice_list,
            voice_remove,
            design_create,
            design_solidify,
            clone_create,
            reveal_path,
            resolve_audio_path,
        ])
        .run(tauri::generate_context!())
        .expect("Tauri 应用启动失败");
}
