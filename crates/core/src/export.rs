//! 导出：逐行音频 / 目录布局 / 整轨拼接 / 清单（ffmpeg 负责转码与处理）。
//!
//! - 只导出**已选中 take 且文件存在**的行；pending 行默认跳过，显式 `--line` 指定时直接报错；
//! - 格式 `wav`（无处理时直接复制）或 `mp3`（libmp3lame 192k）；
//! - 可选 `--normalize`（loudnorm -16 LUFS）与 `--trim-silence`（silenceremove）；
//! - 输出目录非空且未确认时返回 `E_CONFIRM_REQUIRED`（退出码 4）；
//! - 始终写入 `manifest.json`（供下游工程消费）。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Serialize;

use crate::error::{
    Result, TtsError, E_CONFIRM_REQUIRED, E_FFMPEG_FAILED, E_FFMPEG_MISSING, E_IO, E_NOT_FOUND,
    E_USAGE,
};
use crate::project::{LineEntry, LinesFile, Project, TakeRef, STATUS_REMOVED};
use crate::util::now_rfc3339;

pub const DEFAULT_NAME_TEMPLATE: &str = "{seq}_{role}_{line}";
const TRIM_FILTER: &str =
    "silenceremove=start_periods=1:start_threshold=-45dB:detection=peak:stop_periods=-1:stop_threshold=-45dB";
const LOUDNORM_FILTER: &str = "loudnorm=I=-16:TP=-1.5:LRA=11";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    Wav,
    Mp3,
}

impl ExportFormat {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "wav" => Ok(ExportFormat::Wav),
            "mp3" => Ok(ExportFormat::Mp3),
            other => Err(TtsError::usage(
                E_USAGE,
                format!("不支持的导出格式：{other}（仅 wav / mp3）"),
            )),
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ExportFormat::Wav => "wav",
            ExportFormat::Mp3 => "mp3",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Flat,
    Role,
    Scene,
}

impl Layout {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "flat" => Ok(Layout::Flat),
            "role" => Ok(Layout::Role),
            "scene" => Ok(Layout::Scene),
            other => Err(TtsError::usage(
                E_USAGE,
                format!("不支持的布局：{other}（仅 flat / role / scene）"),
            )),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::Flat => "flat",
            Layout::Role => "role",
            Layout::Scene => "scene",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub out_dir: PathBuf,
    pub format: ExportFormat,
    pub layout: Layout,
    pub name_template: String,
    pub concat: bool,
    pub normalize: bool,
    pub trim_silence: bool,
    pub overwrite: bool,
    pub line_ids: Vec<String>,
    pub scene: Option<String>,
    pub ffmpeg: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportItem {
    pub line_id: String,
    pub role: String,
    pub scene: Option<String>,
    pub chapter: Option<String>,
    pub index: usize,
    pub take_id: String,
    /// 相对输出目录的路径。
    pub file: String,
    pub duration_ms: u64,
    pub chars: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExportOutcome {
    pub out_dir: PathBuf,
    pub format: String,
    pub layout: String,
    pub manifest: PathBuf,
    pub concat: Option<PathBuf>,
    pub items: Vec<ExportItem>,
    pub ffmpeg_used: bool,
}

/// 选择导出条目（非 removed、命中过滤、存在 selected_take）。
pub fn select_lines<'a>(
    lines: &'a LinesFile,
    line_ids: &[String],
    scene: Option<&str>,
) -> Result<Vec<(&'a LineEntry, &'a TakeRef)>> {
    let explicit: HashSet<&str> = line_ids.iter().map(|id| id.as_str()).collect();
    if !explicit.is_empty() {
        let unknown: Vec<String> = explicit
            .iter()
            .filter(|id| lines.find(id).is_none())
            .map(|id| (*id).to_string())
            .collect();
        if !unknown.is_empty() {
            return Err(TtsError::runtime(
                E_NOT_FOUND,
                format!("以下行不存在：{}", unknown.join(", ")),
            ));
        }
    }

    let mut selected = Vec::new();
    for line in &lines.lines {
        if line.status == STATUS_REMOVED {
            continue;
        }
        if !explicit.is_empty() && !explicit.contains(line.id.as_str()) {
            continue;
        }
        if let Some(scene) = scene {
            if line.scene.as_deref() != Some(scene) {
                continue;
            }
        }
        let take_id = match line.selected_take.as_deref() {
            Some(take_id) => take_id,
            None => {
                if !explicit.is_empty() {
                    return Err(TtsError::usage(
                        E_USAGE,
                        format!("行 {} 尚未生成（无 selected_take）", line.id),
                    )
                    .with_hint("先运行 `tts batch --line <ID>`"));
                }
                continue;
            }
        };
        let take = match line.takes.iter().find(|take| take.take_id == take_id) {
            Some(take) => take,
            None => {
                return Err(TtsError::runtime(
                    E_NOT_FOUND,
                    format!("行 {} 的 selected_take（{take_id}）不存在", line.id),
                ))
            }
        };
        selected.push((line, take));
    }

    if selected.is_empty() {
        return Err(TtsError::runtime(E_NOT_FOUND, "没有可导出的条目")
            .with_hint("先运行 `tts batch` 生成音频，或检查 --line / --scene 过滤"));
    }
    Ok(selected)
}

/// 渲染文件名（占位符：`{seq}` `{role}` `{line}` `{take}` `{scene}` `{chapter}`）。
pub fn render_name(template: &str, line: &LineEntry, take: &TakeRef, sequence: usize) -> String {
    let rendered = template
        .replace("{seq}", &format!("{sequence:04}"))
        .replace("{role}", line.role.as_str())
        .replace("{line}", line.id.as_str())
        .replace("{take}", take.take_id.as_str())
        .replace("{scene}", line.scene.as_deref().unwrap_or(""))
        .replace("{chapter}", line.chapter.as_deref().unwrap_or(""));
    let sanitized = sanitize_component(&rendered);
    if sanitized == "unnamed" {
        format!("{sequence:04}_{}", line.id)
    } else {
        sanitized
    }
}

/// 清理路径组件：替换不安全字符、折叠下划线并去首尾下划线。
pub fn sanitize_component(raw: &str) -> String {
    let mut out = String::new();
    for ch in raw.chars() {
        if ch.is_whitespace() || "/\\:*?\"<>|".contains(ch) {
            out.push('_');
        } else {
            out.push(ch);
        }
    }
    let mut collapsed = String::new();
    let mut last_underscore = false;
    for ch in out.chars() {
        if ch == '_' {
            if !last_underscore {
                collapsed.push('_');
            }
            last_underscore = true;
        } else {
            collapsed.push(ch);
            last_underscore = false;
        }
    }
    let trimmed = collapsed.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "unnamed".to_string()
    } else {
        trimmed
    }
}

fn layout_dir(layout: Layout, line: &LineEntry) -> Option<String> {
    match layout {
        Layout::Flat => None,
        Layout::Role => Some(sanitize_component(&line.role)),
        Layout::Scene => Some(sanitize_component(
            line.scene.as_deref().unwrap_or("no-scene"),
        )),
    }
}

fn filters(options: &ExportOptions) -> Option<String> {
    let mut list = Vec::new();
    if options.trim_silence {
        list.push(TRIM_FILTER.to_string());
    }
    if options.normalize {
        list.push(LOUDNORM_FILTER.to_string());
    }
    if list.is_empty() {
        None
    } else {
        Some(list.join(","))
    }
}

/// 校验 ffmpeg 输出非空（防止全静音被裁剪后产生空文件）。
fn ensure_non_empty(path: &Path) -> Result<()> {
    let size = fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);
    if size == 0 {
        return Err(TtsError::runtime(
            E_FFMPEG_FAILED,
            format!("输出文件为 0 字节：{}", path.display()),
        )
        .with_hint("音频可能全为静音（--trim-silence 会将其完全剪除），或检查 ffmpeg 参数"));
    }
    Ok(())
}

fn run_ffmpeg(ffmpeg: &Path, args: &[String]) -> Result<()> {
    let output = Command::new(ffmpeg)
        .args(args)
        .output()
        .map_err(|e| TtsError::runtime(E_FFMPEG_FAILED, format!("执行 ffmpeg 失败：{e}")))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: String = stderr
            .chars()
            .rev()
            .take(600)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        return Err(TtsError::runtime(
            E_FFMPEG_FAILED,
            format!(
                "ffmpeg 处理失败（退出码 {:?}）：{}",
                output.status.code(),
                tail.trim()
            ),
        ));
    }
    Ok(())
}

/// 执行导出。
pub fn run(project: &Project, options: &ExportOptions) -> Result<ExportOutcome> {
    let lines = project.load_lines()?;
    let selected = select_lines(&lines, &options.line_ids, options.scene.as_deref())?;
    let filter = filters(options);
    let needs_ffmpeg = options.format == ExportFormat::Mp3 || filter.is_some() || options.concat;

    if needs_ffmpeg && options.ffmpeg.is_none() {
        return Err(TtsError::runtime(
            E_FFMPEG_MISSING,
            "导出需要 ffmpeg 但未找到",
        )
        .with_hint("安装 ffmpeg（macOS: brew install ffmpeg），或用 `tts config set ffmpeg.path <绝对路径>` 指定"));
    }

    if options.out_dir.exists() {
        let non_empty = fs::read_dir(&options.out_dir)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false);
        if non_empty && !options.overwrite {
            return Err(TtsError::confirm_required(
                E_CONFIRM_REQUIRED,
                format!("输出目录非空：{}", options.out_dir.display()),
            )
            .with_hint("确认覆盖请追加 --yes，或指定其他 --out 目录"));
        }
    }
    fs::create_dir_all(&options.out_dir).map_err(|e| {
        TtsError::runtime(
            E_IO,
            format!("无法创建输出目录 {}：{e}", options.out_dir.display()),
        )
    })?;

    let mut items: Vec<ExportItem> = Vec::new();
    let mut concat_inputs: Vec<PathBuf> = Vec::new();

    for (position, (line, take)) in selected.iter().enumerate() {
        let sequence = position + 1;
        let source = project.root.join(&take.file);
        if !source.is_file() {
            return Err(TtsError::runtime(
                E_NOT_FOUND,
                format!("take 文件缺失：{}", source.display()),
            )
            .with_hint("重新生成该行（`tts batch --line <ID>`）后重试"));
        }

        let dir = match layout_dir(options.layout, line) {
            Some(sub) => options.out_dir.join(sub),
            None => options.out_dir.clone(),
        };
        fs::create_dir_all(&dir)
            .map_err(|e| TtsError::runtime(E_IO, format!("无法创建目录 {}：{e}", dir.display())))?;

        let name = render_name(&options.name_template, line, take, sequence);
        let target = dir.join(format!("{}.{}", name, options.format.extension()));

        let needs_conversion = match options.format {
            ExportFormat::Wav => filter.is_some(),
            ExportFormat::Mp3 => true,
        };
        if needs_conversion {
            let ffmpeg = options.ffmpeg.as_ref().expect("已校验 ffmpeg 存在");
            let mut args: Vec<String> = vec![
                "-hide_banner".to_string(),
                "-loglevel".to_string(),
                "error".to_string(),
                "-y".to_string(),
                "-i".to_string(),
                source.display().to_string(),
            ];
            if let Some(filter) = &filter {
                args.push("-af".to_string());
                args.push(filter.clone());
            }
            match options.format {
                ExportFormat::Wav => {
                    args.push("-c:a".to_string());
                    args.push("pcm_s16le".to_string());
                }
                ExportFormat::Mp3 => {
                    args.push("-c:a".to_string());
                    args.push("libmp3lame".to_string());
                    args.push("-b:a".to_string());
                    args.push("192k".to_string());
                }
            }
            args.push(target.display().to_string());
            run_ffmpeg(ffmpeg, &args)?;
        } else {
            fs::copy(&source, &target).map_err(|e| {
                TtsError::runtime(
                    E_IO,
                    format!(
                        "复制音频失败（{} → {}）：{e}",
                        source.display(),
                        target.display()
                    ),
                )
            })?;
        }
        ensure_non_empty(&target)?;
        concat_inputs.push(target.clone());

        let relative = target
            .strip_prefix(&options.out_dir)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| target.clone())
            .display()
            .to_string();
        items.push(ExportItem {
            line_id: line.id.clone(),
            role: line.role.clone(),
            scene: line.scene.clone(),
            chapter: line.chapter.clone(),
            index: line.index,
            take_id: take.take_id.clone(),
            file: relative,
            duration_ms: take.duration_ms,
            chars: take.chars,
        });
    }

    let concat_path = if options.concat {
        let ffmpeg = options.ffmpeg.as_ref().expect("已校验 ffmpeg 存在");
        let list_path = options.out_dir.join(".concat.txt");
        let mut list = String::new();
        for path in &concat_inputs {
            let escaped = path.display().to_string().replace('\'', "'\\''");
            list.push_str(&format!("file '{escaped}'\n"));
        }
        fs::write(&list_path, list).map_err(|e| {
            TtsError::runtime(
                E_IO,
                format!("无法写入拼接清单 {}：{e}", list_path.display()),
            )
        })?;

        let target = options
            .out_dir
            .join(format!("full.{}", options.format.extension()));
        let mut args: Vec<String> = vec![
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "error".to_string(),
            "-y".to_string(),
            "-f".to_string(),
            "concat".to_string(),
            "-safe".to_string(),
            "0".to_string(),
            "-i".to_string(),
            list_path.display().to_string(),
        ];
        match options.format {
            ExportFormat::Wav => {
                args.push("-c:a".to_string());
                args.push("pcm_s16le".to_string());
            }
            ExportFormat::Mp3 => {
                args.push("-c:a".to_string());
                args.push("libmp3lame".to_string());
                args.push("-b:a".to_string());
                args.push("192k".to_string());
            }
        }
        args.push(target.display().to_string());
        run_ffmpeg(ffmpeg, &args)?;
        ensure_non_empty(&target)?;
        let _ = fs::remove_file(&list_path);
        Some(target)
    } else {
        None
    };

    let manifest_path = options.out_dir.join("manifest.json");
    let manifest = serde_json::json!({
        "schema_version": 1,
        "project": project.root.display().to_string(),
        "name": project.meta.name,
        "created_at": now_rfc3339(),
        "format": options.format.extension(),
        "layout": options.layout.name(),
        "concat": concat_path
            .as_ref()
            .map(|path| path.strip_prefix(&options.out_dir).unwrap_or(path).display().to_string()),
        "items": items,
    });
    let mut text = serde_json::to_string_pretty(&manifest)?;
    text.push('\n');
    fs::write(&manifest_path, text).map_err(|e| {
        TtsError::runtime(
            E_IO,
            format!("无法写入清单 {}：{e}", manifest_path.display()),
        )
    })?;

    Ok(ExportOutcome {
        out_dir: options.out_dir.clone(),
        format: options.format.extension().to_string(),
        layout: options.layout.name().to_string(),
        manifest: manifest_path,
        concat: concat_path,
        items,
        ffmpeg_used: needs_ffmpeg,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project::Overrides;

    fn line(id: &str, role: &str, text: &str, status: &str) -> LineEntry {
        LineEntry {
            id: id.to_string(),
            chapter: Some("第一章".to_string()),
            scene: Some("雨夜".to_string()),
            index: 1,
            role: role.to_string(),
            text: text.to_string(),
            status: status.to_string(),
            selected_take: None,
            overrides: Overrides::default(),
            takes: Vec::new(),
        }
    }

    fn take(id: &str) -> TakeRef {
        TakeRef {
            take_id: id.to_string(),
            file: format!("takes/x/{id}_abcd1234.wav"),
            params_hash: "abcd1234".to_string(),
            cache_key: Some("0123456789abcdef".to_string()),
            chars: 5,
            duration_ms: 1000,
            created_at: "2026-10-12T00:00:00Z".to_string(),
        }
    }

    fn lines_file(lines: Vec<LineEntry>) -> LinesFile {
        LinesFile {
            schema_version: crate::project::LINES_SCHEMA_VERSION,
            lines,
        }
    }

    #[test]
    fn format_and_layout_parse() {
        assert_eq!(ExportFormat::parse("mp3").unwrap(), ExportFormat::Mp3);
        assert_eq!(Layout::parse("role").unwrap(), Layout::Role);
        assert_eq!(ExportFormat::parse("flac").unwrap_err().exit_code(), 2);
        assert_eq!(Layout::parse("tree").unwrap_err().exit_code(), 2);
    }

    #[test]
    fn name_template_renders_and_sanitizes() {
        let mut l = line("L1234abcd", "阿岚", "台词", "ok");
        l.scene = None;
        let t = take("0001");
        assert_eq!(
            render_name(DEFAULT_NAME_TEMPLATE, &l, &t, 3),
            "0003_阿岚_L1234abcd"
        );
        assert_eq!(render_name("{role} {scene} {take}", &l, &t, 1), "阿岚_0001");
        assert_eq!(
            render_name("{role}/{line}", &l, &t, 1),
            "阿岚_L1234abcd",
            "斜杠应被清理"
        );
        assert_eq!(render_name("", &l, &t, 7), "0007_L1234abcd");
    }

    #[test]
    fn select_skips_pending_but_errors_when_explicit() {
        let mut ok_line = line("L1", "阿岚", "一", "ok");
        ok_line.selected_take = Some("0001".to_string());
        ok_line.takes.push(take("0001"));
        let pending = line("L2", "小满", "二", "pending");

        let lines = lines_file(vec![ok_line, pending]);

        let selected = select_lines(&lines, &[], None).unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].0.id, "L1");

        let err = select_lines(&lines, &["L2".to_string()], None).unwrap_err();
        assert_eq!(err.code(), E_USAGE);

        let err = select_lines(&lines, &["NOPE".to_string()], None).unwrap_err();
        assert_eq!(err.code(), E_NOT_FOUND);
    }

    #[test]
    fn select_filters_scene_and_skips_removed() {
        let mut a = line("L1", "阿岚", "一", "ok");
        a.selected_take = Some("0001".to_string());
        a.takes.push(take("0001"));
        let mut b = line("L2", "小满", "二", "ok");
        b.scene = Some("清晨".to_string());
        b.selected_take = Some("0001".to_string());
        b.takes.push(take("0001"));
        let mut removed = line("L3", "阿岚", "三", "removed");
        removed.selected_take = Some("0001".to_string());
        removed.takes.push(take("0001"));

        let lines = lines_file(vec![a, b, removed]);
        assert_eq!(select_lines(&lines, &[], None).unwrap().len(), 2);
        assert_eq!(select_lines(&lines, &[], Some("清晨")).unwrap().len(), 1);

        let empty = lines_file(vec![line("L9", "阿岚", "九", "pending")]);
        assert_eq!(
            select_lines(&empty, &[], None).unwrap_err().code(),
            E_NOT_FOUND
        );
    }

    #[test]
    fn filters_combine() {
        let base = ExportOptions {
            out_dir: PathBuf::from("/tmp/out"),
            format: ExportFormat::Wav,
            layout: Layout::Flat,
            name_template: DEFAULT_NAME_TEMPLATE.to_string(),
            concat: false,
            normalize: false,
            trim_silence: false,
            overwrite: false,
            line_ids: Vec::new(),
            scene: None,
            ffmpeg: None,
        };
        assert!(filters(&base).is_none());

        let trim = ExportOptions {
            trim_silence: true,
            ..base.clone()
        };
        assert!(filters(&trim).unwrap().contains("silenceremove"));

        let both = ExportOptions {
            trim_silence: true,
            normalize: true,
            ..base.clone()
        };
        let combined = filters(&both).unwrap();
        assert!(combined.contains("silenceremove") && combined.contains("loudnorm"));
    }
}
