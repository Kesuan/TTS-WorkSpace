//! 项目（= 作品）存储：`project.json` / `lines.json` / `takes/` / `exports/`。
//!
//! `import_script` 负责“解析 → 稳定 ID → diff → 迁移/标记”的完整流程：
//! - 未变化的行：保留 takes / 选中项 / 行级覆盖；
//! - 同位置同角色的文本修改：计为 `changed`，旧行标记 `removed`（保留 takes 作历史），新行需重新生成；
//! - 消失的行：标记 `removed`（**不删除** takes），导出默认排除。

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

use crate::error::{Result, TtsError, E_IO, E_NOT_FOUND, E_USAGE};
use crate::script::{parse_file, ParseCounts, ParseOptions, ParseWarning, ParsedLine};

pub const PROJECT_SCHEMA_VERSION: u32 = 1;
pub const LINES_SCHEMA_VERSION: u32 = 1;

pub const STATUS_PENDING: &str = "pending";
pub const STATUS_OK: &str = "ok";
pub const STATUS_FAILED: &str = "failed";
pub const STATUS_REMOVED: &str = "removed";

fn now_rfc3339() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

/// 角色 ↔ 音色绑定（项目级）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct RoleBinding {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ProjectMeta {
    pub schema_version: u32,
    pub name: String,
    pub created_at: String,
    pub updated_at: String,
    pub scripts: Vec<String>,
    pub roles: BTreeMap<String, RoleBinding>,
}

impl Default for ProjectMeta {
    fn default() -> Self {
        let now = now_rfc3339();
        Self {
            schema_version: PROJECT_SCHEMA_VERSION,
            name: String::new(),
            created_at: now.clone(),
            updated_at: now,
            scripts: Vec::new(),
            roles: BTreeMap::new(),
        }
    }
}

/// 行级覆盖（由 `tts line set` 写入）。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Overrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub voice: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub style: Option<String>,
}

impl Overrides {
    pub fn is_empty(&self) -> bool {
        self.voice.is_none() && self.style.is_none()
    }
}

/// 一次生成产生的 take 引用（音频文件在 `takes/<line_id>/`）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TakeRef {
    pub take_id: String,
    pub file: String,
    #[serde(default)]
    pub params_hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_key: Option<String>,
    #[serde(default)]
    pub chars: usize,
    #[serde(default)]
    pub duration_ms: u64,
    #[serde(default)]
    pub created_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineEntry {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scene: Option<String>,
    #[serde(default)]
    pub index: usize,
    pub role: String,
    pub text: String,
    #[serde(default = "default_status")]
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_take: Option<String>,
    #[serde(default, skip_serializing_if = "Overrides::is_empty")]
    pub overrides: Overrides,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub takes: Vec<TakeRef>,
}

fn default_status() -> String {
    STATUS_PENDING.to_string()
}

impl LineEntry {
    pub fn from_parsed(parsed: &ParsedLine) -> Self {
        Self {
            id: parsed.id.clone(),
            chapter: parsed.chapter.clone(),
            scene: parsed.scene.clone(),
            index: parsed.index,
            role: parsed.role.clone(),
            text: parsed.text.clone(),
            status: STATUS_PENDING.to_string(),
            selected_take: None,
            overrides: Overrides::default(),
            takes: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LinesFile {
    pub schema_version: u32,
    pub lines: Vec<LineEntry>,
}

impl Default for LinesFile {
    fn default() -> Self {
        Self {
            schema_version: LINES_SCHEMA_VERSION,
            lines: Vec::new(),
        }
    }
}

impl LinesFile {
    pub fn find(&self, id: &str) -> Option<&LineEntry> {
        self.lines.iter().find(|line| line.id == id)
    }

    pub fn find_mut(&mut self, id: &str) -> Option<&mut LineEntry> {
        self.lines.iter_mut().find(|line| line.id == id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ImportDiff {
    pub added: usize,
    pub removed: usize,
    pub changed: usize,
    pub unchanged: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportOutcome {
    pub script: String,
    pub counts: ParseCounts,
    pub roles: Vec<String>,
    pub unbound_roles: Vec<String>,
    pub warnings: Vec<ParseWarning>,
    pub diff: ImportDiff,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub root: PathBuf,
    pub meta: ProjectMeta,
}

impl Project {
    /// 初始化项目（已存在则报用法错误）。
    pub fn create(root: &Path, name: &str) -> Result<Self> {
        let meta_path = root.join("project.json");
        if meta_path.exists() {
            return Err(
                TtsError::usage(E_USAGE, format!("项目已存在：{}", meta_path.display()))
                    .with_hint("换一个目录，或直接使用现有项目"),
            );
        }
        for sub in ["script", "takes", "exports"] {
            let dir = root.join(sub);
            fs::create_dir_all(&dir).map_err(|e| {
                TtsError::runtime(E_IO, format!("无法创建目录 {}：{e}", dir.display()))
            })?;
        }
        let meta = ProjectMeta {
            name: resolve_name(root, name),
            ..ProjectMeta::default()
        };
        let project = Self {
            root: root.to_path_buf(),
            meta,
        };
        project.save_meta()?;
        project.save_lines(&LinesFile::default())?;
        Ok(project)
    }

    /// 打开已有项目（缺少 `project.json` 报 NOT_FOUND）。
    pub fn open(root: &Path) -> Result<Self> {
        let meta_path = root.join("project.json");
        if !meta_path.exists() {
            return Err(TtsError::runtime(
                E_NOT_FOUND,
                format!("未找到项目（缺少 project.json）：{}", root.display()),
            )
            .with_hint("用 `tts project init <路径>` 初始化项目"));
        }
        let raw = fs::read_to_string(&meta_path).map_err(|e| {
            TtsError::runtime(E_IO, format!("无法读取 {}：{e}", meta_path.display()))
        })?;
        let meta: ProjectMeta = serde_json::from_str(&raw).map_err(|e| {
            TtsError::runtime(
                E_IO,
                format!("project.json 解析失败（{}）：{e}", meta_path.display()),
            )
        })?;
        Ok(Self {
            root: root.to_path_buf(),
            meta,
        })
    }

    pub fn meta_path(&self) -> PathBuf {
        self.root.join("project.json")
    }

    pub fn lines_path(&self) -> PathBuf {
        self.root.join("lines.json")
    }

    pub fn script_dir(&self) -> PathBuf {
        self.root.join("script")
    }

    pub fn takes_dir(&self) -> PathBuf {
        self.root.join("takes")
    }

    pub fn exports_dir(&self) -> PathBuf {
        self.root.join("exports")
    }

    pub fn save_meta(&self) -> Result<()> {
        write_json(&self.meta_path(), &self.meta)
    }

    pub fn load_lines(&self) -> Result<LinesFile> {
        let path = self.lines_path();
        if !path.exists() {
            return Ok(LinesFile::default());
        }
        let raw = fs::read_to_string(&path)
            .map_err(|e| TtsError::runtime(E_IO, format!("无法读取 {}：{e}", path.display())))?;
        serde_json::from_str(&raw).map_err(|e| {
            TtsError::runtime(
                E_IO,
                format!("lines.json 解析失败（{}）：{e}", path.display()),
            )
        })
    }

    pub fn save_lines(&self, lines: &LinesFile) -> Result<()> {
        write_json(&self.lines_path(), lines)
    }

    /// 绑定角色 ↔ 音色 / 默认风格（`None` 表示保持原值）。
    pub fn bind_role(
        &mut self,
        role: &str,
        voice: Option<String>,
        style: Option<String>,
    ) -> Result<()> {
        {
            let entry = self.meta.roles.entry(role.to_string()).or_default();
            if voice.is_some() {
                entry.voice = voice;
            }
            if style.is_some() {
                entry.style = style;
            }
        }
        self.meta.updated_at = now_rfc3339();
        self.save_meta()
    }

    /// 生效音色：行级覆盖 > 角色绑定。
    pub fn resolve_voice(&self, line: &LineEntry) -> Option<String> {
        line.overrides.voice.clone().or_else(|| {
            self.meta
                .roles
                .get(&line.role)
                .and_then(|r| r.voice.clone())
        })
    }

    /// 生效风格：行级覆盖 > 角色绑定。
    pub fn resolve_style(&self, line: &LineEntry) -> Option<String> {
        line.overrides.style.clone().or_else(|| {
            self.meta
                .roles
                .get(&line.role)
                .and_then(|r| r.style.clone())
        })
    }

    /// 解析脚本 → 稳定 ID → diff → 写回 `lines.json` 与 `project.json`。
    pub fn import_script(
        &mut self,
        script_path: &Path,
        options: &ParseOptions,
    ) -> Result<ImportOutcome> {
        let report = parse_file(script_path, options)?;
        let mut lines_file = self.load_lines()?;
        let old_lines = std::mem::take(&mut lines_file.lines);

        let mut old_by_id: HashMap<String, LineEntry> = HashMap::new();
        for entry in old_lines {
            old_by_id.insert(entry.id.clone(), entry);
        }

        let mut new_entries: Vec<LineEntry> = Vec::with_capacity(report.lines.len());
        let mut added_indices: Vec<usize> = Vec::new();
        let mut unchanged = 0usize;

        for parsed in &report.lines {
            if let Some(old) = old_by_id.remove(&parsed.id) {
                unchanged += 1;
                new_entries.push(LineEntry {
                    id: parsed.id.clone(),
                    chapter: parsed.chapter.clone(),
                    scene: parsed.scene.clone(),
                    index: parsed.index,
                    role: parsed.role.clone(),
                    text: parsed.text.clone(),
                    status: if old.status == STATUS_REMOVED {
                        STATUS_PENDING.to_string()
                    } else {
                        old.status
                    },
                    selected_take: old.selected_take,
                    overrides: old.overrides,
                    takes: old.takes,
                });
            } else {
                added_indices.push(new_entries.len());
                new_entries.push(LineEntry::from_parsed(parsed));
            }
        }

        // 同位置（章节/场景/角色/序号）的文本修改配对：迁移行级覆盖，旧行转 removed 幽灵条目
        let mut leftovers: Vec<LineEntry> = old_by_id.into_values().collect();
        let mut ghosts: Vec<LineEntry> = Vec::new();
        let mut changed = 0usize;
        for &new_idx in &added_indices {
            let key = position_key(&new_entries[new_idx]);
            let matched = leftovers.iter().position(|old| position_key(old) == key);
            if let Some(i) = matched {
                let mut old = leftovers.remove(i);
                new_entries[new_idx].overrides = old.overrides.clone();
                old.status = STATUS_REMOVED.to_string();
                ghosts.push(old);
                changed += 1;
            }
        }
        let added = added_indices.len() - changed;

        // 未配对旧行 → removed（保留 takes 历史，不删除）
        let mut removed = 0usize;
        let mut removed_entries: Vec<LineEntry> = Vec::new();
        for mut old in leftovers {
            if old.status != STATUS_REMOVED {
                removed += 1;
            }
            old.status = STATUS_REMOVED.to_string();
            removed_entries.push(old);
        }
        removed_entries.extend(ghosts);
        removed_entries.sort_by(|a, b| a.index.cmp(&b.index).then_with(|| a.id.cmp(&b.id)));

        lines_file.lines = new_entries;
        lines_file.lines.extend(removed_entries);
        self.save_lines(&lines_file)?;

        let relative = relative_display(&self.root, script_path);
        if !self.meta.scripts.contains(&relative) {
            self.meta.scripts.push(relative.clone());
        }
        self.meta.updated_at = now_rfc3339();
        self.save_meta()?;

        let unbound_roles = report
            .roles
            .iter()
            .filter(|role| !self.meta.roles.contains_key(*role))
            .cloned()
            .collect();

        Ok(ImportOutcome {
            script: relative,
            counts: report.counts,
            roles: report.roles,
            unbound_roles,
            warnings: report.warnings,
            diff: ImportDiff {
                added,
                removed,
                changed,
                unchanged,
            },
        })
    }
}

fn resolve_name(root: &Path, name: &str) -> String {
    let trimmed = name.trim();
    if !trimmed.is_empty() {
        return trimmed.to_string();
    }
    root.file_name()
        .and_then(|s| s.to_str())
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "untitled".to_string())
}

fn position_key(entry: &LineEntry) -> String {
    format!(
        "{}\u{1f}{}\u{1f}{}\u{1f}{}",
        entry.chapter.as_deref().unwrap_or(""),
        entry.scene.as_deref().unwrap_or(""),
        entry.role,
        entry.index
    )
}

fn relative_display(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(rel) => rel.display().to_string(),
        Err(_) => path.display().to_string(),
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| {
            TtsError::runtime(E_IO, format!("无法创建目录 {}：{e}", parent.display()))
        })?;
    }
    let mut text = serde_json::to_string_pretty(value)?;
    text.push('\n');
    fs::write(path, text)
        .map_err(|e| TtsError::runtime(E_IO, format!("无法写入 {}：{e}", path.display())))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        _dir: tempfile::TempDir,
        project: Project,
        script: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        let project = Project::create(&root, "测试作品").unwrap();
        let script = root.join("script/chapter1.txt");
        Fixture {
            _dir: dir,
            project,
            script,
        }
    }

    fn write_script(path: &Path, content: &str) {
        fs::write(path, content).unwrap();
    }

    #[test]
    fn create_and_open_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("my-work");
        let project = Project::create(&root, "").unwrap();
        assert_eq!(project.meta.name, "my-work");
        assert_eq!(project.meta.schema_version, PROJECT_SCHEMA_VERSION);
        assert!(project.script_dir().is_dir());
        assert!(project.takes_dir().is_dir());
        assert!(project.exports_dir().is_dir());
        assert!(project.lines_path().is_file());

        let reopened = Project::open(&root).unwrap();
        assert_eq!(reopened.meta, project.meta);
    }

    #[test]
    fn create_existing_is_usage_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("proj");
        Project::create(&root, "a").unwrap();
        let err = Project::create(&root, "b").unwrap_err();
        assert_eq!(err.code(), E_USAGE);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn open_missing_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let err = Project::open(&dir.path().join("nope")).unwrap_err();
        assert_eq!(err.code(), E_NOT_FOUND);
    }

    #[test]
    fn import_creates_lines_and_marks_unbound_roles() {
        let mut fx = fixture();
        write_script(
            &fx.script,
            "# 第一章\n## 雨夜\n阿岚: 又是这样的雨……\n小满: 哥，你还在等那班车吗？\n",
        );
        let outcome = fx
            .project
            .import_script(&fx.script, &ParseOptions::default())
            .unwrap();
        assert_eq!(outcome.counts.lines, 2);
        assert_eq!(outcome.diff.added, 2);
        assert_eq!(outcome.diff.unchanged, 0);
        assert_eq!(outcome.unbound_roles, vec!["阿岚", "小满"]);
        assert_eq!(outcome.script, "script/chapter1.txt");

        let lines = fx.project.load_lines().unwrap();
        assert_eq!(lines.lines.len(), 2);
        assert_eq!(lines.lines[0].status, STATUS_PENDING);
        assert_eq!(lines.lines[0].role, "阿岚");

        let meta = Project::open(&fx.project.root).unwrap().meta;
        assert_eq!(meta.scripts, vec!["script/chapter1.txt"]);
    }

    #[test]
    fn reimport_unchanged_is_stable() {
        let mut fx = fixture();
        write_script(&fx.script, "阿岚: 台词\n");
        fx.project
            .import_script(&fx.script, &ParseOptions::default())
            .unwrap();
        let outcome = fx
            .project
            .import_script(&fx.script, &ParseOptions::default())
            .unwrap();
        assert_eq!(outcome.diff.added, 0);
        assert_eq!(outcome.diff.removed, 0);
        assert_eq!(outcome.diff.changed, 0);
        assert_eq!(outcome.diff.unchanged, 1);
        assert_eq!(fx.project.load_lines().unwrap().lines.len(), 1);
    }

    #[test]
    fn text_edit_is_changed_and_preserves_history() {
        let mut fx = fixture();
        write_script(&fx.script, "阿岚: 旧台词\n");
        fx.project
            .import_script(&fx.script, &ParseOptions::default())
            .unwrap();

        // 手工注入 take 与覆盖，模拟已生成
        let mut lines = fx.project.load_lines().unwrap();
        {
            let line = lines.find_mut(&lines.lines[0].id.clone()).unwrap();
            line.takes.push(TakeRef {
                take_id: "0001".into(),
                file: "takes/x/0001_abcd1234.wav".into(),
                params_hash: "abcd1234".into(),
                cache_key: None,
                chars: 3,
                duration_ms: 1000,
                created_at: now_rfc3339(),
            });
            line.selected_take = Some("0001".into());
            line.overrides.style = Some("(低沉)".into());
            line.status = STATUS_OK.to_string();
        }
        fx.project.save_lines(&lines).unwrap();

        write_script(&fx.script, "阿岚: 新台词\n");
        let outcome = fx
            .project
            .import_script(&fx.script, &ParseOptions::default())
            .unwrap();
        assert_eq!(outcome.diff.changed, 1);
        assert_eq!(outcome.diff.added, 0);
        assert_eq!(outcome.diff.removed, 0);

        let lines = fx.project.load_lines().unwrap();
        assert_eq!(lines.lines.len(), 2, "新行 + 旧行幽灵条目");
        let fresh = lines.lines.iter().find(|l| l.text == "新台词").unwrap();
        assert_eq!(fresh.status, STATUS_PENDING);
        assert!(fresh.takes.is_empty(), "文本已变化，旧 take 不迁移");
        assert_eq!(fresh.overrides.style.as_deref(), Some("(低沉)"), "覆盖迁移");

        let ghost = lines.lines.iter().find(|l| l.text == "旧台词").unwrap();
        assert_eq!(ghost.status, STATUS_REMOVED);
        assert_eq!(ghost.takes.len(), 1, "旧 take 保留为历史");
        assert_eq!(ghost.selected_take.as_deref(), Some("0001"));
    }

    #[test]
    fn removed_line_is_marked_but_kept() {
        let mut fx = fixture();
        write_script(&fx.script, "阿岚: 第一句\n小满: 第二句\n");
        fx.project
            .import_script(&fx.script, &ParseOptions::default())
            .unwrap();

        write_script(&fx.script, "阿岚: 第一句\n");
        let outcome = fx
            .project
            .import_script(&fx.script, &ParseOptions::default())
            .unwrap();
        assert_eq!(outcome.diff.removed, 1);
        assert_eq!(outcome.diff.unchanged, 1);

        let lines = fx.project.load_lines().unwrap();
        assert_eq!(lines.lines.len(), 2);
        let removed = lines
            .lines
            .iter()
            .find(|l| l.role == "小满")
            .expect("被删除的行应保留为 removed");
        assert_eq!(removed.status, STATUS_REMOVED);
    }

    #[test]
    fn role_binding_and_resolution() {
        let mut fx = fixture();
        write_script(&fx.script, "阿岚: 台词\n");
        fx.project
            .import_script(&fx.script, &ParseOptions::default())
            .unwrap();
        fx.project
            .bind_role("阿岚", Some("冰糖".into()), Some("清亮".into()))
            .unwrap();

        let mut lines = fx.project.load_lines().unwrap();
        let line = &lines.lines[0];
        assert_eq!(fx.project.resolve_voice(line).as_deref(), Some("冰糖"));
        assert_eq!(fx.project.resolve_style(line).as_deref(), Some("清亮"));

        // 行级覆盖优先
        lines.lines[0].overrides.voice = Some("茉莉".into());
        assert_eq!(
            fx.project.resolve_voice(&lines.lines[0]).as_deref(),
            Some("茉莉")
        );

        // 绑定持久化
        let reopened = Project::open(&fx.project.root).unwrap();
        assert_eq!(
            reopened.meta.roles.get("阿岚").unwrap().voice.as_deref(),
            Some("冰糖")
        );
    }
}
