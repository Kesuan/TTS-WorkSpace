//! 对白脚本解析（格式 v1，规范见 `docs/script-format.md`）。
//!
//! - 章节 `#` / 场景 `##` / 注释 `//` / 缩进续行 / 显式与隐式旁白；
//! - 行内标签 `(风格)` 与 `[音频标签]` 原样保留（透传 API）；
//! - **稳定行 ID**：`L` + sha256(章节␟场景␟角色␟规范化文本)[0:8]，同文本重复行加 `-2`/`-3` 后缀。

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::error::{Result, TtsError, E_NOT_FOUND, E_PARSE_SCRIPT};

pub const MAX_ROLE_NAME_CHARS: usize = 24;
pub const NARRATOR: &str = "旁白";

/// 解析选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseOptions {
    /// 无角色前缀且未缩进的段落归入「旁白」（否则忽略并告警）。
    pub narrator_fallback: bool,
    /// 续行之间插入空格（英文文稿可用；中文默认直接拼接）。
    pub join_with_space: bool,
}

impl Default for ParseOptions {
    fn default() -> Self {
        Self {
            narrator_fallback: true,
            join_with_space: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParseWarning {
    pub line: usize,
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParsedLine {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    pub index: usize,
    pub role: String,
    pub text: String,
    /// 源文件行号（1-based，首行）。
    pub line_no: usize,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ParseCounts {
    pub lines: usize,
    pub roles: usize,
    pub chapters: usize,
    pub scenes: usize,
    pub narrator_lines: usize,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ParseReport {
    pub lines: Vec<ParsedLine>,
    pub roles: Vec<String>,
    pub chapters: Vec<String>,
    pub scenes: Vec<String>,
    pub counts: ParseCounts,
    pub warnings: Vec<ParseWarning>,
}

/// 读取并解析脚本文件。
pub fn parse_file(path: &Path, options: &ParseOptions) -> Result<ParseReport> {
    if !path.exists() {
        return Err(TtsError::runtime(
            E_NOT_FOUND,
            format!("脚本文件不存在：{}", path.display()),
        ));
    }
    let raw = std::fs::read_to_string(path).map_err(|e| {
        TtsError::runtime(
            E_PARSE_SCRIPT,
            format!("无法读取脚本 {}：{e}", path.display()),
        )
    })?;
    parse_script(&raw, options, &path.display().to_string())
}

/// 解析脚本内容；`source_name` 仅用于错误定位信息。
pub fn parse_script(
    source: &str,
    options: &ParseOptions,
    source_name: &str,
) -> Result<ParseReport> {
    let mut report = ParseReport {
        lines: Vec::new(),
        roles: Vec::new(),
        chapters: Vec::new(),
        scenes: Vec::new(),
        counts: ParseCounts::default(),
        warnings: Vec::new(),
    };
    let mut chapter: Option<String> = None;
    let mut scene: Option<String> = None;
    let mut pending: Option<usize> = None;
    let mut id_counts: HashMap<String, usize> = HashMap::new();

    for (idx, raw_line) in source.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();

        if trimmed.is_empty() {
            pending = None; // 空行结束续行
            continue;
        }
        if trimmed.starts_with("//") {
            continue; // 注释
        }
        if trimmed.starts_with('#') {
            let level = trimmed.chars().take_while(|c| *c == '#').count();
            let title = trimmed[level..].trim().to_string();
            pending = None;
            match level {
                1 => {
                    chapter = (!title.is_empty()).then(|| title.clone());
                    scene = None;
                    if let Some(value) = &chapter {
                        push_unique(&mut report.chapters, value);
                    }
                }
                2 => {
                    scene = (!title.is_empty()).then(|| title.clone());
                    if let Some(value) = &scene {
                        push_unique(&mut report.scenes, value);
                    }
                }
                _ => {} // ### 及以上视为注释
            }
            continue;
        }

        if let Some((role, text)) = split_role_line(trimmed) {
            validate_role(&role, source_name, line_no, trimmed)?;
            let line = make_line(
                &mut id_counts,
                &mut report,
                chapter.clone(),
                scene.clone(),
                role,
                text,
                line_no,
            );
            if line.role == NARRATOR {
                report.counts.narrator_lines += 1;
            }
            report.lines.push(line);
            pending = Some(report.lines.len() - 1);
            continue;
        }

        let indented = line.starts_with(|c: char| c.is_whitespace());
        if indented {
            if let Some(last) = pending {
                let target = &mut report.lines[last];
                if options.join_with_space && !target.text.is_empty() {
                    target.text.push(' ');
                }
                target.text.push_str(trimmed);
            } else {
                report.warnings.push(ParseWarning {
                    line: line_no,
                    code: "W_ORPHAN_CONTINUATION".to_string(),
                    message: "缩进行没有可续接的台词，已忽略".to_string(),
                });
            }
            continue;
        }

        // 未缩进且无角色前缀：旁白段落 或 忽略
        if options.narrator_fallback {
            let line = make_line(
                &mut id_counts,
                &mut report,
                chapter.clone(),
                scene.clone(),
                NARRATOR.to_string(),
                trimmed.to_string(),
                line_no,
            );
            report.counts.narrator_lines += 1;
            report.lines.push(line);
            pending = Some(report.lines.len() - 1);
        } else {
            report.warnings.push(ParseWarning {
                line: line_no,
                code: "W_TEXT_IGNORED".to_string(),
                message: "无角色前缀的段落（旁白回退已关闭），已忽略".to_string(),
            });
            pending = None;
        }
    }

    report.counts.lines = report.lines.len();
    report.counts.roles = report.roles.len();
    report.counts.chapters = report.chapters.len();
    report.counts.scenes = report.scenes.len();
    Ok(report)
}

fn make_line(
    id_counts: &mut HashMap<String, usize>,
    report: &mut ParseReport,
    chapter: Option<String>,
    scene: Option<String>,
    role: String,
    text: String,
    line_no: usize,
) -> ParsedLine {
    push_unique(&mut report.roles, &role);
    let id = stable_id(
        chapter.as_deref(),
        scene.as_deref(),
        &role,
        &text,
        id_counts,
    );
    ParsedLine {
        id,
        chapter,
        scene,
        index: report.lines.len() + 1,
        role,
        text,
        line_no,
    }
}

/// 稳定行 ID：`L` + sha256(章节␟场景␟角色␟规范化文本) 前 8 位十六进制。
pub fn stable_id(
    chapter: Option<&str>,
    scene: Option<&str>,
    role: &str,
    text: &str,
    counts: &mut HashMap<String, usize>,
) -> String {
    let key = format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        chapter.unwrap_or(""),
        scene.unwrap_or(""),
        role,
        normalize_text(text)
    );
    let digest = Sha256::digest(key.as_bytes());
    let base = format!("L{}", &hex::encode(digest)[..8]);
    let counter = counts.entry(base.clone()).or_insert(0);
    *counter += 1;
    if *counter == 1 {
        base
    } else {
        format!("{base}-{counter}")
    }
}

/// 规范化文本：合并连续空白并去除首尾（行内标签保留原样）。
pub fn normalize_text(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn push_unique(list: &mut Vec<String>, value: &str) {
    if !list.iter().any(|v| v == value) {
        list.push(value.to_string());
    }
}

/// 尝试按 `角色: 台词` 切分（兼容全角冒号）。
///
/// 以下情况不视为角色行：前缀为空、前缀含 `[` `]` `/`、冒号后紧跟 `/`（URL 防误判）。
fn split_role_line(trimmed: &str) -> Option<(String, String)> {
    let (byte_pos, char_len) = first_colon(trimmed)?;
    let role_raw = &trimmed[..byte_pos];
    let role = role_raw.trim();
    if role.is_empty() || role.contains(['[', ']', '/']) {
        return None;
    }
    let text = trimmed[byte_pos + char_len..].trim();
    if text.starts_with('/') {
        return None;
    }
    Some((role.to_string(), text.to_string()))
}

fn first_colon(text: &str) -> Option<(usize, usize)> {
    for (idx, ch) in text.char_indices() {
        if ch == ':' || ch == '：' {
            return Some((idx, ch.len_utf8()));
        }
    }
    None
}

fn validate_role(role: &str, source_name: &str, line_no: usize, line: &str) -> Result<()> {
    let len = role.chars().count();
    if len > MAX_ROLE_NAME_CHARS {
        let col = line
            .chars()
            .position(|c| c == ':' || c == '：')
            .unwrap_or(0)
            + 1;
        return Err(TtsError::runtime(
            E_PARSE_SCRIPT,
            format!("角色名超过 {MAX_ROLE_NAME_CHARS} 字符"),
        )
        .with_hint("检查角色行写法；角色名不可含 `:` `：` `[` `]` `/`")
        .with_details(serde_json::json!({
            "file": source_name,
            "line": line_no,
            "col": col,
            "role": role,
        })));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(source: &str) -> ParseReport {
        parse_script(source, &ParseOptions::default(), "test.txt").unwrap()
    }

    #[test]
    fn parses_roles_chapters_scenes_and_narrator() {
        let report = parse(
            "# 第一章\n## 雨夜\n旁白: 站台的灯闪了两下。\n阿岚: (低沉)[叹气] 又是这样的雨……\n没有角色前缀的段落。\n",
        );
        assert_eq!(report.counts.lines, 3);
        assert_eq!(report.counts.narrator_lines, 2);
        assert_eq!(report.roles, vec!["旁白", "阿岚"]);
        assert_eq!(report.chapters, vec!["第一章"]);
        assert_eq!(report.scenes, vec!["雨夜"]);
        let third = &report.lines[2];
        assert_eq!(third.role, NARRATOR);
        assert_eq!(third.text, "没有角色前缀的段落。");
        assert_eq!(third.chapter.as_deref(), Some("第一章"));
        assert_eq!(third.scene.as_deref(), Some("雨夜"));
        assert_eq!(third.index, 3);
    }

    #[test]
    fn fullwidth_colon_is_supported() {
        let report = parse("阿岚：早上好！\n");
        assert_eq!(report.lines[0].role, "阿岚");
        assert_eq!(report.lines[0].text, "早上好！");
    }

    #[test]
    fn indented_lines_continue_previous() {
        let report = parse("阿岚: 第一句。\n    第二句。\n\t第三句。\n");
        assert_eq!(report.counts.lines, 1);
        assert_eq!(report.lines[0].text, "第一句。第二句。第三句。");
        assert!(report.warnings.is_empty());
    }

    #[test]
    fn join_with_space_option() {
        let options = ParseOptions {
            join_with_space: true,
            ..ParseOptions::default()
        };
        let report = parse_script("Aki: Hello,\n    world.\n", &options, "en.txt").unwrap();
        assert_eq!(report.lines[0].text, "Hello, world.");
    }

    #[test]
    fn blank_line_ends_continuation() {
        let report = parse("阿岚: 第一句。\n\n    独立缩进段。\n");
        assert_eq!(report.counts.lines, 1);
        assert_eq!(report.warnings.len(), 1);
        assert_eq!(report.warnings[0].code, "W_ORPHAN_CONTINUATION");
    }

    #[test]
    fn narrator_fallback_can_be_disabled() {
        let options = ParseOptions {
            narrator_fallback: false,
            ..ParseOptions::default()
        };
        let report = parse_script("无角色段落。\n", &options, "t").unwrap();
        assert!(report.lines.is_empty());
        assert_eq!(report.warnings[0].code, "W_TEXT_IGNORED");
    }

    #[test]
    fn comments_and_deep_headings_are_ignored() {
        let report = parse("// 注释\n### 小节\n阿岚: 台词\n");
        assert_eq!(report.counts.lines, 1);
        assert!(report.chapters.is_empty());
        assert!(report.scenes.is_empty());
    }

    #[test]
    fn inline_tags_are_preserved() {
        let report = parse("阿岚: (低沉)[叹气] 又是这样的雨……\n");
        assert_eq!(report.lines[0].text, "(低沉)[叹气] 又是这样的雨……");
    }

    #[test]
    fn stable_ids_are_deterministic_and_dedup_suffixes() {
        let a = parse("阿岚: 台词\n");
        let b = parse("阿岚: 台词\n");
        assert_eq!(a.lines[0].id, b.lines[0].id);
        assert!(a.lines[0].id.starts_with('L'));
        assert_eq!(a.lines[0].id.len(), 9);

        let dup = parse("阿岚: 台词\n阿岚: 台词\n");
        assert!(dup.lines[1].id.ends_with("-2"));
        assert_ne!(dup.lines[0].id, dup.lines[1].id);

        // 场景/角色不同 → ID 不同
        let other = parse("## 别的场景\n阿岚: 台词\n");
        assert_ne!(a.lines[0].id, other.lines[0].id);
    }

    #[test]
    fn overlong_role_is_parse_error_with_location() {
        let role = "长".repeat(25);
        let source = format!("{role}: 台词\n");
        let err = parse_script(&source, &ParseOptions::default(), "chapter1.txt").unwrap_err();
        assert_eq!(err.code(), E_PARSE_SCRIPT);
        let details = err.body.details.as_ref().unwrap();
        assert_eq!(details["file"], serde_json::json!("chapter1.txt"));
        assert_eq!(details["line"], serde_json::json!(1));
        assert!(details["role"].as_str().unwrap().chars().count() > 24);
    }

    #[test]
    fn forbidden_characters_do_not_make_a_role_line() {
        let report = parse("a/b: 这不是角色行\n");
        assert_eq!(report.lines[0].role, NARRATOR);
        assert_eq!(report.lines[0].text, "a/b: 这不是角色行");
    }

    #[test]
    fn url_like_line_is_not_a_role_line() {
        let report = parse("参考 https://example.com/docs 说明。\n");
        assert_eq!(report.lines[0].role, NARRATOR);
        // 无缩进、非角色行 → 旁白，整行保留
        assert!(report.lines[0].text.starts_with("参考 https://"));
    }

    #[test]
    fn crlf_is_handled() {
        let report = parse("阿岚: 第一句。\r\n    第二句。\r\n");
        assert_eq!(report.lines[0].text, "第一句。第二句。");
        assert_eq!(report.counts.lines, 1);
    }

    #[test]
    fn missing_file_is_not_found() {
        let err = parse_file(
            Path::new("/nonexistent/script.txt"),
            &ParseOptions::default(),
        )
        .unwrap_err();
        assert_eq!(err.code(), E_NOT_FOUND);
    }
}
