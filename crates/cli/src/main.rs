//! `tts` 命令行入口（Agent 主接口）。
//!
//! 契约（详见 `docs/cli.md`）：
//! - `--json`：stdout 输出统一信封 `{"ok":true,"data":…}` 或 `{"ok":false,"error":…}`；
//! - 非 `--json`：成功输出到 stdout（人类可读），失败输出到 stderr；
//! - 退出码：0 成功 / 1 运行时失败 / 2 用法与校验错误 / 3 部分失败 / 4 需要确认。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use tts_core::audit::{self, AuditRecord};
use tts_core::batch::{self, BatchEvent, BatchOptions};
use tts_core::cache::{directory_size, Cache};
use tts_core::config::{mask_secret, Config, ENV_API_KEY};
use tts_core::doctor::{self, CheckStatus};
use tts_core::error::{
    success_envelope, Result, TtsError, E_DOCTOR_UNHEALTHY, E_IO, E_NOT_FOUND, E_PARTIAL, E_USAGE,
};
use tts_core::export::{self, ExportFormat, ExportOptions, Layout, DEFAULT_NAME_TEMPLATE};
use tts_core::mimo::MimoClient;
use tts_core::paths::Paths;
use tts_core::project::{Project, STATUS_FAILED, STATUS_OK, STATUS_PENDING, STATUS_REMOVED};
use tts_core::provider::AudioFormat;
use tts_core::script::ParseOptions;
use tts_core::synth::{self, SynthOptions};
use tts_core::usage;
use tts_core::voices::parse_voice_ref;

#[derive(Parser, Debug)]
#[command(
    name = "tts",
    version,
    about = "TTS-WorkSpace — MiMo TTS 工作台 CLI（Agent 优先）",
    long_about = "基于 MiMo-V2.5-TTS 的通用语音合成工作台。\n\n所有命令支持 --json（stdout 统一信封）；退出码：0 成功 / 1 运行时失败 / 2 用法错误 / 3 部分失败 / 4 需要确认。详见 docs/cli.md"
)]
struct Cli {
    /// 以 JSON 输出（机器可读；stdout 统一信封）
    #[arg(long, global = true)]
    json: bool,

    /// 危险操作确认（非交互环境必需；覆盖导出目录时使用）
    #[arg(long, global = true)]
    yes: bool,

    /// 静默模式（成功时不输出人类可读内容）
    #[arg(long, global = true)]
    quiet: bool,

    /// 详细日志（stderr）
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// 环境自检（工作区 / 配置 / API Key / ffmpeg / 网络）
    Doctor(DoctorArgs),
    /// 读取与修改配置
    Config(ConfigArgs),
    /// 项目（= 作品）管理：初始化 / 导入对白 / 状态
    Project(ProjectArgs),
    /// 角色 ↔ 音色绑定
    Role(RoleArgs),
    /// 行级覆盖（音色 / 风格）
    Line(LineArgs),
    /// 批量生成（并发 / 重试 / 幂等续跑 / 事件流）
    Batch(BatchArgs),
    /// Take 管理（列表 / 选用）
    Takes(TakesArgs),
    /// 同句多音色对比（选角）
    Cast(CastArgs),
    /// 用量统计（来自审计日志）
    Usage(UsageArgs),
    /// 清理未被引用的缓存
    Gc(GcArgs),
    /// 导出：逐行音频 / 目录布局 / 整轨拼接 / 清单
    Export(ExportArgs),
    /// 合成单条语音（缓存去重；可联动项目行写入 take）
    Synth(SynthArgs),
    /// 打印版本信息
    Version,
}

#[derive(Args, Debug)]
struct DoctorArgs {
    /// 跳过网络连通性检查（离线 / CI 环境）
    #[arg(long)]
    skip_network: bool,
}

#[derive(Args, Debug)]
struct ConfigArgs {
    #[command(subcommand)]
    cmd: ConfigCmd,
}

#[derive(Subcommand, Debug)]
enum ConfigCmd {
    /// 列出全部配置项
    List {
        /// 显示完整 API Key（默认打码）
        #[arg(long)]
        reveal: bool,
    },
    /// 读取单个配置项
    Get {
        /// 配置项键（如 api.base_url）
        key: String,
        /// 显示完整 API Key（默认打码）
        #[arg(long)]
        reveal: bool,
    },
    /// 设置单个配置项（空值可用于清除 api.api_key）
    Set {
        /// 配置项键（如 api.api_key）
        key: String,
        /// 配置项值
        value: String,
    },
}

#[derive(Args, Debug)]
struct ProjectArgs {
    #[command(subcommand)]
    cmd: ProjectCmd,
}

#[derive(Subcommand, Debug)]
enum ProjectCmd {
    /// 初始化项目（= 作品；目录不存在则创建）
    Init {
        /// 项目目录
        path: PathBuf,
        /// 作品名（默认取目录名）
        #[arg(long)]
        name: Option<String>,
    },
    /// 解析对白脚本并写入 lines.json（输出解析报告与 diff）
    Import {
        /// 脚本文件路径
        script: PathBuf,
        /// 项目目录
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// 关闭“无角色前缀段落归入旁白”（这类段落将被忽略并告警）
        #[arg(long)]
        no_narrator_fallback: bool,
        /// 续行之间插入空格（英文文稿适用）
        #[arg(long)]
        join_with_space: bool,
    },
    /// 项目状态概览（条目状态分布 / 角色绑定 / 脚本清单）
    Status {
        /// 项目目录
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
}

#[derive(Args, Debug)]
struct RoleArgs {
    #[command(subcommand)]
    cmd: RoleCmd,
}

#[derive(Subcommand, Debug)]
enum RoleCmd {
    /// 绑定角色 ↔ 音色 / 默认风格
    Bind {
        /// 角色名（与脚本中的角色名一致）
        #[arg(long)]
        role: String,
        /// 音色引用（预置名 / design:<描述> / clone:<音频文件>）
        #[arg(long)]
        voice: Option<String>,
        /// 角色默认风格指令
        #[arg(long)]
        style: Option<String>,
        /// 项目目录
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
}

#[derive(Args, Debug)]
struct LineArgs {
    #[command(subcommand)]
    cmd: LineCmd,
}

#[derive(Subcommand, Debug)]
enum LineCmd {
    /// 设置行级覆盖（音色 / 风格）
    Set {
        /// 行 ID
        #[arg(long)]
        line: String,
        /// 行级音色覆盖
        #[arg(long)]
        voice: Option<String>,
        /// 行级风格覆盖
        #[arg(long)]
        style: Option<String>,
        /// 清除行级音色覆盖
        #[arg(long)]
        clear_voice: bool,
        /// 清除行级风格覆盖
        #[arg(long)]
        clear_style: bool,
        /// 项目目录
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
}

#[derive(Args, Debug)]
struct BatchArgs {
    #[command(subcommand)]
    cmd: Option<BatchCmd>,
    /// 项目目录
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// 仅生成指定行（可重复；显式指定即视为重新生成）
    #[arg(long = "line")]
    line: Vec<String>,
    /// 仅生成指定场景
    #[arg(long)]
    scene: Option<String>,
    /// 并发数（默认取配置 generation.concurrency）
    #[arg(long)]
    concurrency: Option<usize>,
    /// 模型 ID（默认取配置 models.tts）
    #[arg(long)]
    model: Option<String>,
    /// 忽略已完成状态，全部重新生成
    #[arg(long)]
    force: bool,
    /// 跳过缓存
    #[arg(long)]
    no_cache: bool,
    /// 事件流格式（仅支持 jsonl；需同时使用 --json）
    #[arg(long)]
    events: Option<String>,
}

#[derive(Subcommand, Debug)]
enum BatchCmd {
    /// 批量状态：状态分布与失败行清单
    Status {
        #[arg(long, default_value = ".")]
        project: PathBuf,
    },
    /// 将失败行重置为 pending（便于重跑）
    Retry {
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// 仅重置指定行（可重复）
        #[arg(long = "line")]
        line: Vec<String>,
    },
}

#[derive(Args, Debug)]
struct TakesArgs {
    #[command(subcommand)]
    cmd: TakesCmd,
}

#[derive(Subcommand, Debug)]
enum TakesCmd {
    /// 列出 take（可选按行过滤）
    List {
        /// 项目目录
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// 仅列出指定行的 take
        #[arg(long)]
        line: Option<String>,
    },
    /// 选用 take（设置 selected_take）
    Select {
        /// 项目目录
        #[arg(long, default_value = ".")]
        project: PathBuf,
        /// 行 ID
        #[arg(long)]
        line: String,
        /// take ID（如 0002）
        #[arg(long)]
        take: String,
    },
}

#[derive(Args, Debug)]
struct CastArgs {
    /// 项目目录
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// 行 ID
    #[arg(long)]
    line: String,
    /// 参与对比的音色（逗号分隔，如 冰糖,茉莉,苏打）
    #[arg(long)]
    voices: String,
    /// 风格指令（默认取行级覆盖 / 角色绑定）
    #[arg(long)]
    style: Option<String>,
    /// 跳过缓存
    #[arg(long)]
    no_cache: bool,
    /// 模型 ID（默认取配置 models.tts）
    #[arg(long)]
    model: Option<String>,
}

#[derive(Args, Debug)]
struct UsageArgs {
    /// 时间戳前缀过滤（如 2026-10、2026-10-12）
    #[arg(long)]
    since: Option<String>,
}

#[derive(Args, Debug)]
struct GcArgs {
    /// 额外扫描的项目目录（可重复）；默认扫描 <工作区>/projects/*
    #[arg(long = "project")]
    project: Vec<PathBuf>,
    /// 只预览将删除的内容，不实际删除
    #[arg(long)]
    dry_run: bool,
}

#[derive(Args, Debug)]
struct ExportArgs {
    /// 项目目录
    #[arg(long, default_value = ".")]
    project: PathBuf,
    /// 输出目录（默认 <项目>/exports/<时间戳>；目录非空时需 --yes）
    #[arg(long)]
    out: Option<PathBuf>,
    /// 输出格式：wav | mp3
    #[arg(long, default_value = "wav")]
    format: String,
    /// 目录布局：flat | role | scene
    #[arg(long, default_value = "flat")]
    layout: String,
    /// 文件名模板（占位符：{seq} {role} {line} {take} {scene} {chapter}）
    #[arg(long)]
    name: Option<String>,
    /// 生成整轨拼接文件 full.<ext>
    #[arg(long)]
    concat: bool,
    /// 响度归一化（ffmpeg loudnorm，-16 LUFS）
    #[arg(long)]
    normalize: bool,
    /// 裁剪首尾静音（ffmpeg silenceremove）
    #[arg(long)]
    trim_silence: bool,
    /// 仅导出指定行（可重复）
    #[arg(long = "line")]
    line: Vec<String>,
    /// 仅导出指定场景
    #[arg(long)]
    scene: Option<String>,
}

#[derive(Args, Debug)]
struct SynthArgs {
    /// 待合成文本（使用 --line 时可省略，取行文本）
    #[arg(long)]
    text: Option<String>,
    /// 音色：预置名/ID（如 冰糖），或 design:<描述>、clone:<音频文件>
    #[arg(long)]
    voice: Option<String>,
    /// 风格指令（自然语言，写入 user 消息）
    #[arg(long)]
    style: Option<String>,
    /// 模型 ID（默认取配置 models.tts）
    #[arg(long)]
    model: Option<String>,
    /// 低延迟流式输出（仅预置音色模型；仍落盘完整 wav）
    #[arg(long)]
    stream: bool,
    /// 跳过缓存，强制请求
    #[arg(long)]
    no_cache: bool,
    /// 输出文件（独立模式；默认 <工作区>/synth/<cache_key>.wav）
    #[arg(long)]
    out: Option<PathBuf>,
    /// 项目目录（配合 --line 写入 take）
    #[arg(long)]
    project: Option<PathBuf>,
    /// 行 ID（联动项目：写 take、更新状态与选中项）
    #[arg(long)]
    line: Option<String>,
}

struct CmdOutput {
    data: Value,
    human: String,
}

fn main() {
    let raw_args: Vec<String> = std::env::args().collect();
    let wants_json = raw_args.iter().any(|arg| arg == "--json");

    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(err) => {
            use clap::error::ErrorKind;
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                print!("{err}");
                std::process::exit(0);
            }
            let text = err.to_string();
            let message = text
                .lines()
                .find(|line| !line.trim().is_empty())
                .unwrap_or("参数错误")
                .trim()
                .to_string();
            let error = TtsError::usage(E_USAGE, message)
                .with_details(json!({ "usage_error": text.trim() }));
            emit_error(&error, wants_json);
            std::process::exit(2);
        }
    };

    init_logging(cli.verbose);

    let runtime = match tokio::runtime::Runtime::new() {
        Ok(runtime) => runtime,
        Err(e) => {
            let error = TtsError::runtime(tts_core::error::E_IO, format!("运行时初始化失败：{e}"));
            emit_error(&error, cli.json);
            std::process::exit(2);
        }
    };

    match runtime.block_on(execute(&cli)) {
        Ok(output) => {
            emit_success(&output, &cli);
            std::process::exit(0);
        }
        Err(error) => {
            emit_error(&error, cli.json);
            std::process::exit(error.exit_code());
        }
    }
}

async fn execute(cli: &Cli) -> Result<CmdOutput> {
    match &cli.command {
        Command::Doctor(args) => cmd_doctor(args).await,
        Command::Config(args) => cmd_config(args),
        Command::Project(args) => cmd_project(args),
        Command::Role(args) => cmd_role(args),
        Command::Line(args) => cmd_line(args),
        Command::Batch(args) => cmd_batch(cli, args).await,
        Command::Takes(args) => cmd_takes(args),
        Command::Cast(args) => cmd_cast(args).await,
        Command::Usage(args) => cmd_usage(args),
        Command::Gc(args) => cmd_gc(args),
        Command::Export(args) => cmd_export(cli, args),
        Command::Synth(args) => cmd_synth(args).await,
        Command::Version => cmd_version(),
    }
}

fn cmd_version() -> Result<CmdOutput> {
    let paths = Paths::resolve()?;
    let data = json!({
        "name": "tts",
        "version": tts_core::VERSION,
        "workspace_root": paths.root.display().to_string(),
    });
    let human = format!(
        "tts {}（工作区：{}）",
        tts_core::VERSION,
        paths.root.display()
    );
    Ok(CmdOutput { data, human })
}

async fn cmd_doctor(args: &DoctorArgs) -> Result<CmdOutput> {
    let paths = Paths::resolve()?;
    let report = doctor::run(&paths, args.skip_network).await;
    let data = serde_json::to_value(&report)?;
    if report.has_failure() {
        let failed = report.failed_names().join(", ");
        return Err(TtsError::runtime(
            E_DOCTOR_UNHEALTHY,
            format!("环境自检未通过（失败项：{failed}）"),
        )
        .with_details(data)
        .with_hint("修复失败项后重试；详见 docs/cli.md"));
    }
    Ok(CmdOutput {
        data,
        human: render_doctor(&report),
    })
}

fn render_doctor(report: &doctor::DoctorReport) -> String {
    let mut lines = vec![
        format!("tts {}", report.version),
        format!("工作区：{}", report.workspace_root),
    ];
    for check in &report.checks {
        let mark = match check.status {
            CheckStatus::Ok => "ok",
            CheckStatus::Warn => "warn",
            CheckStatus::Fail => "fail",
            CheckStatus::Skipped => "skip",
        };
        lines.push(format!("[{mark}] {}：{}", check.name, check.detail));
        if let Some(hint) = &check.hint {
            lines.push(format!("       ↳ {hint}"));
        }
    }
    let summary = &report.summary;
    lines.push(format!(
        "汇总：ok={} warn={} fail={} skip={}",
        summary.ok, summary.warn, summary.fail, summary.skipped
    ));
    lines.join("\n")
}

fn cmd_config(args: &ConfigArgs) -> Result<CmdOutput> {
    let paths = Paths::resolve()?;
    match &args.cmd {
        ConfigCmd::List { reveal } => {
            let exists = paths.config_file.exists();
            let config = Config::load(&paths.config_file)?;
            let data = json!({
                "config_path": paths.config_file.display().to_string(),
                "exists": exists,
                "api_key_source": api_key_source(&config),
                "config": config.to_json_value(*reveal),
            });
            let human = serde_json::to_string_pretty(&data).unwrap_or_default();
            Ok(CmdOutput { data, human })
        }
        ConfigCmd::Get { key, reveal } => {
            let config = Config::load(&paths.config_file)?;
            let raw = config.get_field(key)?;
            let secret = is_secret_key(key);
            let (value, masked) = if secret && !reveal {
                match raw.as_str() {
                    Some(s) => (json!(mask_secret(s)), true),
                    None => (Value::Null, true),
                }
            } else {
                (raw, false)
            };
            let human = format!(
                "{key} = {}",
                value
                    .as_str()
                    .map(str::to_string)
                    .unwrap_or_else(|| value.to_string())
            );
            let data = json!({ "key": key, "value": value, "masked": masked });
            Ok(CmdOutput { data, human })
        }
        ConfigCmd::Set { key, value } => {
            let mut config = Config::load(&paths.config_file)?;
            config.set_field(key, value)?;
            config.save(&paths.config_file)?;
            paths.ensure_layout()?;
            let secret = is_secret_key(key);
            let shown = if secret {
                if value.trim().is_empty() {
                    Value::Null
                } else {
                    json!(mask_secret(value))
                }
            } else {
                json!(value)
            };
            let data = json!({
                "key": key,
                "value": shown,
                "config_path": paths.config_file.display().to_string(),
            });
            let human = format!("已写入 {}：{key} = {}", paths.config_file.display(), shown);
            Ok(CmdOutput { data, human })
        }
    }
}

fn cmd_project(args: &ProjectArgs) -> Result<CmdOutput> {
    match &args.cmd {
        ProjectCmd::Init { path, name } => {
            let project = Project::create(path, name.as_deref().unwrap_or(""))?;
            let data = json!({
                "root": project.root.display().to_string(),
                "name": project.meta.name,
                "project_file": project.meta_path().display().to_string(),
                "lines_file": project.lines_path().display().to_string(),
            });
            let human = format!(
                "已初始化项目「{}」：{}",
                project.meta.name,
                project.root.display()
            );
            Ok(CmdOutput { data, human })
        }
        ProjectCmd::Import {
            script,
            project: project_dir,
            no_narrator_fallback,
            join_with_space,
        } => {
            let mut project = Project::open(project_dir)?;
            let options = ParseOptions {
                narrator_fallback: !no_narrator_fallback,
                join_with_space: *join_with_space,
            };
            let outcome = project.import_script(script, &options)?;
            let data = serde_json::to_value(&outcome)?;
            let diff = outcome.diff;
            let unbound = if outcome.unbound_roles.is_empty() {
                "（无）".to_string()
            } else {
                outcome.unbound_roles.join("、")
            };
            let human = format!(
                "已导入 {}：条目 {}（新增 {} / 修改 {} / 删除 {} / 未变 {}）\n角色：{}\n未绑定音色：{}",
                outcome.script,
                outcome.counts.lines,
                diff.added,
                diff.changed,
                diff.removed,
                diff.unchanged,
                outcome.roles.join("、"),
                unbound,
            );
            Ok(CmdOutput { data, human })
        }
        ProjectCmd::Status {
            project: project_dir,
        } => {
            let project = Project::open(project_dir)?;
            let lines = project.load_lines()?;
            let count = |status: &str| {
                lines
                    .lines
                    .iter()
                    .filter(|line| line.status == status)
                    .count()
            };
            let counts = json!({
                "total": lines.lines.len(),
                "pending": count(STATUS_PENDING),
                "ok": count(STATUS_OK),
                "failed": count(STATUS_FAILED),
                "removed": count(STATUS_REMOVED),
            });
            let mut roles: Vec<Value> = Vec::new();
            for line in &lines.lines {
                if line.status == STATUS_REMOVED {
                    continue;
                }
                let name = Value::String(line.role.clone());
                if roles.iter().any(|role| role["name"] == name) {
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
            let data = json!({
                "root": project.root.display().to_string(),
                "name": project.meta.name,
                "scripts": project.meta.scripts,
                "lines_path": project.lines_path().display().to_string(),
                "counts": counts,
                "roles": roles,
            });
            let human = format!(
                "项目「{}」：条目 {}（pending {} / ok {} / failed {} / removed {}），角色 {} 个",
                project.meta.name,
                data["counts"]["total"],
                data["counts"]["pending"],
                data["counts"]["ok"],
                data["counts"]["failed"],
                data["counts"]["removed"],
                roles.len(),
            );
            Ok(CmdOutput { data, human })
        }
    }
}

async fn cmd_synth(args: &SynthArgs) -> Result<CmdOutput> {
    let paths = Paths::resolve()?;
    let config = Config::load(&paths.config_file)?;
    paths.ensure_layout()?;

    if args.out.is_some() && args.line.is_some() {
        return Err(TtsError::usage(E_USAGE, "--out 与 --line 不能同时使用"));
    }

    let project = match (&args.project, &args.line) {
        (Some(dir), Some(_)) => Some(Project::open(dir)?),
        (None, Some(_)) => Some(Project::open(Path::new("."))?),
        (_, None) => None,
    };

    let mut text = args.text.clone();
    let mut voice_spec = args.voice.clone();
    let mut style = args.style.clone();

    if let (Some(project), Some(line_id)) = (project.as_ref(), args.line.as_ref()) {
        let lines = project.load_lines()?;
        let line = lines.find(line_id).ok_or_else(|| {
            TtsError::runtime(E_NOT_FOUND, format!("未找到行：{line_id}"))
                .with_hint("用 `tts project status` 查看条目")
        })?;
        if line.status == STATUS_REMOVED {
            return Err(
                TtsError::usage(E_USAGE, format!("行已被标记 removed：{line_id}"))
                    .with_hint("重新导入脚本，或选择其他行"),
            );
        }
        if text.is_none() {
            text = Some(line.text.clone());
        }
        if voice_spec.is_none() {
            voice_spec = project.resolve_voice(line);
        }
        if style.is_none() {
            style = project.resolve_style(line);
        }
    }

    let text = text
        .ok_or_else(|| TtsError::usage(E_USAGE, "缺少 --text（或使用 --line 从项目读取行文本）"))?;
    let voice_spec = voice_spec.ok_or_else(|| {
        TtsError::usage(E_USAGE, "未指定音色")
            .with_hint("用 --voice <音色>，或先为角色绑定音色（tts role bind）")
    })?;
    let voice = parse_voice_ref(&voice_spec)?;
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| config.models.tts.clone());

    let options = SynthOptions {
        model: model.clone(),
        text: text.clone(),
        voice,
        instructions: style.clone(),
        format: AudioFormat::Wav,
        stream: args.stream,
        use_cache: !args.no_cache,
    };

    let client = MimoClient::from_config(&config)?;
    let cache = Cache::new(paths.cache_dir());
    let outcome = match synth::synthesize(&client, &cache, &options).await {
        Ok(outcome) => outcome,
        Err(error) => {
            let mut record = AuditRecord::ok("synth", &model).into_failed(error.code());
            record.chars = text.chars().count();
            record.voice = Some(voice_spec.clone());
            record.project = project.as_ref().map(|p| p.root.display().to_string());
            record.line_id = args.line.clone();
            let _ = audit::append(&paths.audit_file(), &record);
            return Err(error);
        }
    };

    let (output_path, take_id) = match args.line.as_ref() {
        Some(line_id) => {
            let project = project.as_ref().expect("--line 时项目必然存在");
            let mut lines = project.load_lines()?;
            let (path, take_id) = project.append_take(
                &mut lines,
                line_id,
                &outcome.audio,
                &outcome.cache_key,
                outcome.chars,
                outcome.duration_ms,
            )?;
            project.save_lines(&lines)?;
            (path, Some(take_id))
        }
        None => {
            let path = match &args.out {
                Some(path) => path.clone(),
                None => paths
                    .root
                    .join("synth")
                    .join(format!("{}.wav", outcome.cache_key)),
            };
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).map_err(|e| {
                    TtsError::runtime(E_IO, format!("无法创建目录 {}：{e}", parent.display()))
                })?;
            }
            fs::write(&path, &outcome.audio).map_err(|e| {
                TtsError::runtime(E_IO, format!("写入音频失败 {}：{e}", path.display()))
            })?;
            (path, None)
        }
    };

    let mut record = AuditRecord::ok("synth", &outcome.model);
    record.project = project.as_ref().map(|p| p.root.display().to_string());
    record.line_id = args.line.clone();
    record.voice = Some(voice_spec.clone());
    record.chars = outcome.chars;
    record.duration_ms = outcome.duration_ms;
    record.latency_ms = outcome.latency_ms;
    record.cached = outcome.cached;
    record.cost_note = Some("tts:free".to_string());
    audit::append(&paths.audit_file(), &record)?;

    let data = json!({
        "output": output_path.display().to_string(),
        "model": outcome.model,
        "voice": voice_spec,
        "chars": outcome.chars,
        "duration_ms": outcome.duration_ms,
        "latency_ms": outcome.latency_ms,
        "cached": outcome.cached,
        "cache_key": outcome.cache_key,
        "sample_rate": outcome.sample_rate,
        "stream": args.stream,
        "line_id": args.line,
        "take_id": take_id,
    });
    let cached_note = if outcome.cached {
        "，缓存命中"
    } else {
        ""
    };
    let human = format!(
        "已生成 {}（{} 字符 / {:.1}s{}）",
        output_path.display(),
        outcome.chars,
        outcome.duration_ms as f64 / 1000.0,
        cached_note
    );
    Ok(CmdOutput { data, human })
}

fn cmd_role(args: &RoleArgs) -> Result<CmdOutput> {
    match &args.cmd {
        RoleCmd::Bind {
            role,
            voice,
            style,
            project: project_dir,
        } => {
            if voice.is_none() && style.is_none() {
                return Err(TtsError::usage(E_USAGE, "至少需要 --voice 或 --style 之一"));
            }
            let role_name = role.trim().to_string();
            if role_name.is_empty() {
                return Err(TtsError::usage(E_USAGE, "角色名不能为空"));
            }
            let mut project = Project::open(project_dir)?;
            project.bind_role(&role_name, voice.clone(), style.clone())?;
            let binding = project
                .meta
                .roles
                .get(&role_name)
                .cloned()
                .unwrap_or_default();
            let data = json!({
                "project": project.root.display().to_string(),
                "role": role_name,
                "voice": binding.voice,
                "style": binding.style,
                "roles_total": project.meta.roles.len(),
            });
            let human = format!(
                "已绑定角色「{}」：音色 {} / 风格 {}",
                role_name,
                binding.voice.unwrap_or_else(|| "（未设置）".to_string()),
                binding.style.unwrap_or_else(|| "（未设置）".to_string()),
            );
            Ok(CmdOutput { data, human })
        }
    }
}

fn cmd_line(args: &LineArgs) -> Result<CmdOutput> {
    match &args.cmd {
        LineCmd::Set {
            line,
            voice,
            style,
            clear_voice,
            clear_style,
            project: project_dir,
        } => {
            if voice.is_none() && style.is_none() && !clear_voice && !clear_style {
                return Err(TtsError::usage(E_USAGE, "未指定任何修改")
                    .with_hint("可用 --voice / --style / --clear-voice / --clear-style"));
            }
            let project = Project::open(project_dir)?;
            let mut lines = project.load_lines()?;
            {
                let entry = lines.find_mut(line).ok_or_else(|| {
                    TtsError::runtime(E_NOT_FOUND, format!("未找到行：{line}"))
                        .with_hint("用 `tts project status` 查看有效行 ID")
                })?;
                if entry.status == STATUS_REMOVED {
                    return Err(TtsError::usage(
                        E_USAGE,
                        format!("行已被标记 removed：{line}"),
                    ));
                }
                if *clear_voice {
                    entry.overrides.voice = None;
                }
                if *clear_style {
                    entry.overrides.style = None;
                }
                if let Some(value) = voice {
                    entry.overrides.voice = Some(value.clone());
                }
                if let Some(value) = style {
                    entry.overrides.style = Some(value.clone());
                }
            }
            let entry = lines
                .find(line)
                .cloned()
                .ok_or_else(|| TtsError::runtime(E_NOT_FOUND, format!("未找到行：{line}")))?;
            project.save_lines(&lines)?;
            let data = json!({
                "project": project.root.display().to_string(),
                "line_id": line,
                "overrides": { "voice": entry.overrides.voice, "style": entry.overrides.style },
                "status": entry.status,
            });
            let human = format!(
                "已更新行 {line}：音色 {} / 风格 {}",
                entry
                    .overrides
                    .voice
                    .unwrap_or_else(|| "（未覆盖）".to_string()),
                entry
                    .overrides
                    .style
                    .unwrap_or_else(|| "（未覆盖）".to_string()),
            );
            Ok(CmdOutput { data, human })
        }
    }
}

async fn cmd_batch(cli: &Cli, args: &BatchArgs) -> Result<CmdOutput> {
    if let Some(command) = &args.cmd {
        return match command {
            BatchCmd::Status { project } => batch_status(project),
            BatchCmd::Retry { project, line } => batch_retry(project, line),
        };
    }

    let events_jsonl = match args.events.as_deref() {
        None => false,
        Some("jsonl") => true,
        Some(other) => {
            return Err(
                TtsError::usage(E_USAGE, format!("不支持的事件格式：{other}"))
                    .with_hint("仅支持 --events jsonl"),
            )
        }
    };
    if events_jsonl && !cli.json {
        return Err(
            TtsError::usage(E_USAGE, "--events jsonl 需要同时使用 --json")
                .with_hint("事件流与最终信封都走 stdout（逐行 JSON，最后一行是统一信封）"),
        );
    }

    let paths = Paths::resolve()?;
    let config = Config::load(&paths.config_file)?;
    paths.ensure_layout()?;

    let options = BatchOptions {
        concurrency: args
            .concurrency
            .unwrap_or(config.generation.concurrency as usize),
        retry_max: config.generation.retry.max,
        backoff_ms: config.generation.retry.backoff_ms,
        model: args
            .model
            .clone()
            .unwrap_or_else(|| config.models.tts.clone()),
        use_cache: !args.no_cache,
        force: args.force,
        scene: args.scene.clone(),
        line_ids: args.line.clone(),
    };
    if options.concurrency == 0 {
        return Err(TtsError::usage(E_USAGE, "--concurrency 至少为 1"));
    }

    let project = Project::open(&args.project)?;
    let client = Arc::new(MimoClient::from_config(&config)?);
    let cache = Cache::new(paths.cache_dir());
    let quiet = cli.quiet;
    let sink = move |event: BatchEvent| {
        if events_jsonl {
            println!("{}", serde_json::to_string(&event).unwrap_or_default());
        } else if !quiet {
            eprintln!("{}", render_batch_event(&event));
        }
    };

    let outcome = batch::run(
        client,
        cache,
        &project.root,
        &paths.audit_file(),
        &options,
        &sink,
    )
    .await?;

    let data = json!({
        "project": project.root.display().to_string(),
        "summary": outcome.summary,
        "failed_lines": outcome.failed_lines,
    });

    if outcome.summary.failed > 0 {
        return Err(TtsError::partial(
            E_PARTIAL,
            format!(
                "批量完成：{} 成功 / {} 失败 / {} 跳过",
                outcome.summary.succeeded, outcome.summary.failed, outcome.summary.skipped
            ),
        )
        .with_hint("用 `tts batch status` 查看失败行；修复后 `tts batch retry` 重置再重跑")
        .with_details(data));
    }

    let summary = &outcome.summary;
    let human = format!(
        "批量完成：成功 {} / 失败 {} / 跳过 {}（缓存 {}），耗时 {:.1}s",
        summary.succeeded,
        summary.failed,
        summary.skipped,
        summary.cached,
        summary.total_duration_ms as f64 / 1000.0
    );
    Ok(CmdOutput { data, human })
}

fn batch_status(project_dir: &Path) -> Result<CmdOutput> {
    let project = Project::open(project_dir)?;
    let lines = project.load_lines()?;
    let count = |status: &str| {
        lines
            .lines
            .iter()
            .filter(|line| line.status == status)
            .count()
    };
    let counts = json!({
        "total": lines.lines.len(),
        "pending": count(STATUS_PENDING),
        "ok": count(STATUS_OK),
        "failed": count(STATUS_FAILED),
        "removed": count(STATUS_REMOVED),
    });
    let failed_lines: Vec<Value> = lines
        .lines
        .iter()
        .filter(|line| line.status == STATUS_FAILED)
        .map(|line| {
            json!({
                "line_id": line.id,
                "role": line.role,
                "index": line.index,
                "scene": line.scene,
                "text": line.text,
            })
        })
        .collect();

    let mut unbound: Vec<String> = Vec::new();
    for line in &lines.lines {
        if line.status == STATUS_REMOVED {
            continue;
        }
        if project.resolve_voice(line).is_none() && !unbound.iter().any(|role| role == &line.role) {
            unbound.push(line.role.clone());
        }
    }

    let data = json!({
        "project": project.root.display().to_string(),
        "name": project.meta.name,
        "counts": counts,
        "failed_lines": failed_lines,
        "unbound_roles": unbound,
    });
    let human = format!(
        "项目「{}」：pending {} / ok {} / failed {} / removed {}（失败 {} 行，未绑定角色 {} 个）",
        project.meta.name,
        data["counts"]["pending"],
        data["counts"]["ok"],
        data["counts"]["failed"],
        data["counts"]["removed"],
        failed_lines.len(),
        unbound.len(),
    );
    Ok(CmdOutput { data, human })
}

fn batch_retry(project_dir: &Path, only: &[String]) -> Result<CmdOutput> {
    let project = Project::open(project_dir)?;
    let mut lines = project.load_lines()?;

    if !only.is_empty() {
        let missing: Vec<String> = only
            .iter()
            .filter(|id| lines.find(id).is_none())
            .cloned()
            .collect();
        if !missing.is_empty() {
            return Err(TtsError::runtime(
                E_NOT_FOUND,
                format!("以下行不存在：{}", missing.join(", ")),
            ));
        }
    }

    let mut reset: Vec<String> = Vec::new();
    for line in lines.lines.iter_mut() {
        if line.status != STATUS_FAILED {
            continue;
        }
        if !only.is_empty() && !only.iter().any(|id| id == &line.id) {
            continue;
        }
        line.status = STATUS_PENDING.to_string();
        reset.push(line.id.clone());
    }
    project.save_lines(&lines)?;

    let data = json!({
        "project": project.root.display().to_string(),
        "reset": reset.len(),
        "line_ids": reset,
    });
    let human = if reset.is_empty() {
        "没有需要重置的失败条目".to_string()
    } else {
        format!("已重置 {} 条失败条目为 pending", reset.len())
    };
    Ok(CmdOutput { data, human })
}

fn render_batch_event(event: &BatchEvent) -> String {
    match event {
        BatchEvent::Started {
            total,
            concurrency,
            skipped,
        } => format!("开始批量：{total} 条（并发 {concurrency}，跳过 {skipped}）"),
        BatchEvent::LineStarted { line_id, role } => format!("· {line_id} [{role}] 生成中…"),
        BatchEvent::LineSkipped {
            line_id, reason, ..
        } => format!("· {line_id} 跳过（{reason}）"),
        BatchEvent::LineSucceeded {
            line_id,
            take_id,
            duration_ms,
            cached,
            ..
        } => format!(
            "✓ {line_id} → take {take_id}（{:.1}s{}）",
            *duration_ms as f64 / 1000.0,
            if *cached { "，缓存" } else { "" }
        ),
        BatchEvent::LineFailed {
            line_id,
            code,
            message,
            attempts,
            ..
        } => format!("✗ {line_id} 失败（{code}，尝试 {attempts} 次）：{message}"),
        BatchEvent::Finished {
            succeeded,
            failed,
            skipped,
            cached,
            total_duration_ms,
        } => format!(
            "完成：成功 {succeeded} / 失败 {failed} / 跳过 {skipped}（缓存 {cached}），耗时 {:.1}s",
            *total_duration_ms as f64 / 1000.0
        ),
    }
}

fn cmd_takes(args: &TakesArgs) -> Result<CmdOutput> {
    match &args.cmd {
        TakesCmd::List {
            project: project_dir,
            line,
        } => {
            let project = Project::open(project_dir)?;
            let lines = project.load_lines()?;
            if let Some(filter) = line {
                if lines.find(filter).is_none() {
                    return Err(TtsError::runtime(
                        E_NOT_FOUND,
                        format!("未找到行：{filter}"),
                    ));
                }
            }

            let mut entries: Vec<Value> = Vec::new();
            for entry in &lines.lines {
                if let Some(filter) = line {
                    if entry.id != *filter {
                        continue;
                    }
                }
                for take in &entry.takes {
                    let path = project.root.join(&take.file);
                    entries.push(json!({
                        "line_id": entry.id,
                        "role": entry.role,
                        "line_status": entry.status,
                        "take_id": take.take_id,
                        "file": take.file,
                        "duration_ms": take.duration_ms,
                        "chars": take.chars,
                        "created_at": take.created_at,
                        "selected": entry.selected_take.as_deref() == Some(take.take_id.as_str()),
                        "exists": path.is_file(),
                    }));
                }
            }

            let data = json!({
                "project": project.root.display().to_string(),
                "line_id": line,
                "total": entries.len(),
                "takes": entries,
            });
            let human = if entries.is_empty() {
                "暂无 take".to_string()
            } else {
                let mut text = format!("共 {} 个 take：", entries.len());
                for entry in &entries {
                    let mark = if entry["selected"].as_bool().unwrap_or(false) {
                        "★"
                    } else {
                        " "
                    };
                    text.push_str(&format!(
                        "\n  {mark} {} {} {}",
                        entry["line_id"].as_str().unwrap_or(""),
                        entry["take_id"].as_str().unwrap_or(""),
                        entry["file"].as_str().unwrap_or(""),
                    ));
                }
                text
            };
            Ok(CmdOutput { data, human })
        }
        TakesCmd::Select {
            project: project_dir,
            line,
            take,
        } => {
            let project = Project::open(project_dir)?;
            let mut lines = project.load_lines()?;
            let previous;
            {
                let entry = lines
                    .find_mut(line)
                    .ok_or_else(|| TtsError::runtime(E_NOT_FOUND, format!("未找到行：{line}")))?;
                if !entry.takes.iter().any(|item| item.take_id == *take) {
                    let available: Vec<String> = entry
                        .takes
                        .iter()
                        .map(|item| item.take_id.clone())
                        .collect();
                    let hint = if available.is_empty() {
                        "该行尚未生成任何 take".to_string()
                    } else {
                        format!("可用 take：{}", available.join(", "))
                    };
                    return Err(TtsError::runtime(
                        E_NOT_FOUND,
                        format!("行 {line} 没有 take {take}"),
                    )
                    .with_hint(hint));
                }
                previous = entry.selected_take.clone();
                entry.selected_take = Some(take.clone());
            }
            project.save_lines(&lines)?;
            let data = json!({
                "project": project.root.display().to_string(),
                "line_id": line,
                "take_id": take,
                "previous_selected": previous,
            });
            let human = format!("已选用 take {take}（行 {line}）");
            Ok(CmdOutput { data, human })
        }
    }
}

async fn cmd_cast(args: &CastArgs) -> Result<CmdOutput> {
    let paths = Paths::resolve()?;
    let config = Config::load(&paths.config_file)?;
    paths.ensure_layout()?;

    let project = Project::open(&args.project)?;
    let mut lines = project.load_lines()?;
    let (text, resolved_style) = {
        let entry = lines
            .find(&args.line)
            .ok_or_else(|| TtsError::runtime(E_NOT_FOUND, format!("未找到行：{}", args.line)))?;
        if entry.status == STATUS_REMOVED {
            return Err(TtsError::usage(
                E_USAGE,
                format!("行已被标记 removed：{}", args.line),
            ));
        }
        (
            entry.text.clone(),
            args.style.clone().or_else(|| project.resolve_style(entry)),
        )
    };

    let voices: Vec<String> = args
        .voices
        .split(',')
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
        .collect();
    if voices.is_empty() {
        return Err(TtsError::usage(
            E_USAGE,
            "--voices 不能为空（逗号分隔音色列表）",
        ));
    }

    let client = MimoClient::from_config(&config)?;
    let cache = Cache::new(paths.cache_dir());
    let model = args
        .model
        .clone()
        .unwrap_or_else(|| config.models.tts.clone());

    let mut results: Vec<Value> = Vec::new();
    for voice_spec in &voices {
        let voice = parse_voice_ref(voice_spec)?;
        let options = SynthOptions {
            model: model.clone(),
            text: text.clone(),
            voice,
            instructions: resolved_style.clone(),
            format: AudioFormat::Wav,
            stream: false,
            use_cache: !args.no_cache,
        };
        let outcome = synth::synthesize(&client, &cache, &options).await?;
        let (_, take_id) = project.append_take(
            &mut lines,
            &args.line,
            &outcome.audio,
            &outcome.cache_key,
            outcome.chars,
            outcome.duration_ms,
        )?;
        project.save_lines(&lines)?;
        let file = lines
            .find(&args.line)
            .and_then(|entry| entry.takes.last())
            .map(|take| take.file.clone())
            .unwrap_or_default();

        let mut record = AuditRecord::ok("cast", &model);
        record.project = Some(project.root.display().to_string());
        record.line_id = Some(args.line.clone());
        record.voice = Some(voice_spec.clone());
        record.chars = outcome.chars;
        record.duration_ms = outcome.duration_ms;
        record.latency_ms = outcome.latency_ms;
        record.cached = outcome.cached;
        record.cost_note = Some("tts:free".to_string());
        audit::append(&paths.audit_file(), &record)?;

        results.push(json!({
            "voice": voice_spec,
            "take_id": take_id,
            "file": file,
            "duration_ms": outcome.duration_ms,
            "latency_ms": outcome.latency_ms,
            "cached": outcome.cached,
        }));
    }

    let data = json!({
        "project": project.root.display().to_string(),
        "line_id": args.line,
        "text": text,
        "results": results,
    });
    let human = format!(
        "已为行 {} 生成 {} 个音色的 take（用 `tts takes list` 查看）",
        args.line,
        voices.len()
    );
    Ok(CmdOutput { data, human })
}

fn cmd_usage(args: &UsageArgs) -> Result<CmdOutput> {
    let paths = Paths::resolve()?;
    let records = audit::read_all(&paths.audit_file())?;
    let summary = usage::summarize(&records, args.since.as_deref());
    let data = json!({
        "audit_file": paths.audit_file().display().to_string(),
        "since": args.since,
        "summary": summary,
    });
    let human = format!(
        "请求 {}（ok {} / failed {} / 缓存 {}），字符 {}，音频 {:.1}s，缓存命中率 {:.1}%",
        summary.requests,
        summary.ok,
        summary.failed,
        summary.cached,
        summary.chars,
        summary.audio_duration_ms as f64 / 1000.0,
        summary.cache_hit_rate * 100.0,
    );
    Ok(CmdOutput { data, human })
}

fn cmd_gc(args: &GcArgs) -> Result<CmdOutput> {
    let paths = Paths::resolve()?;
    paths.ensure_layout()?;

    let mut project_dirs: Vec<PathBuf> = args.project.clone();
    let default_root = paths.projects_dir();
    if default_root.is_dir() {
        let mut children: Vec<PathBuf> = fs::read_dir(&default_root)
            .map_err(|e| {
                TtsError::runtime(
                    E_IO,
                    format!("无法读取项目目录 {}：{e}", default_root.display()),
                )
            })?
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.join("lines.json").is_file())
            .collect();
        children.sort();
        project_dirs.extend(children);
    }

    let mut referenced: HashSet<String> = HashSet::new();
    let mut scanned = 0usize;
    for dir in &project_dirs {
        let project = Project::open(dir)?;
        let lines = project.load_lines()?;
        referenced.extend(usage::referenced_cache_keys(&lines));
        scanned += 1;
    }

    let cache = Cache::new(paths.cache_dir());
    let (removed, freed_bytes, kept, dry_run) = if args.dry_run {
        let mut removed = 0u64;
        let mut freed = 0u64;
        let mut kept = 0u64;
        for key in cache.keys()? {
            if referenced.contains(&key) {
                kept += 1;
            } else {
                removed += 1;
                freed += directory_size(&cache.dir_for(&key));
            }
        }
        (removed, freed, kept, true)
    } else {
        let report = cache.gc(&referenced)?;
        (report.removed, report.freed_bytes, report.kept, false)
    };

    let data = json!({
        "cache_dir": cache.root().display().to_string(),
        "scanned_projects": scanned,
        "referenced_keys": referenced.len(),
        "removed": removed,
        "freed_bytes": freed_bytes,
        "kept": kept,
        "dry_run": dry_run,
    });
    let human = if dry_run {
        format!(
            "[预览] 将删除 {removed} 个未引用缓存（约 {:.1} MB），保留 {kept} 个",
            freed_bytes as f64 / 1024.0 / 1024.0
        )
    } else {
        format!(
            "已删除 {removed} 个未引用缓存（释放 {:.1} MB），保留 {kept} 个",
            freed_bytes as f64 / 1024.0 / 1024.0
        )
    };
    Ok(CmdOutput { data, human })
}

fn cmd_export(cli: &Cli, args: &ExportArgs) -> Result<CmdOutput> {
    let paths = Paths::resolve()?;
    let config = Config::load(&paths.config_file)?;

    let project = Project::open(&args.project)?;
    let format = ExportFormat::parse(&args.format)?;
    let layout = Layout::parse(&args.layout)?;
    let ffmpeg = doctor::resolve_ffmpeg(&config);

    let out_dir = match &args.out {
        Some(dir) => dir.clone(),
        None => unique_export_dir(&project.exports_dir()),
    };

    let options = ExportOptions {
        out_dir: out_dir.clone(),
        format,
        layout,
        name_template: args
            .name
            .clone()
            .unwrap_or_else(|| DEFAULT_NAME_TEMPLATE.to_string()),
        concat: args.concat,
        normalize: args.normalize,
        trim_silence: args.trim_silence,
        overwrite: cli.yes,
        line_ids: args.line.clone(),
        scene: args.scene.clone(),
        ffmpeg,
    };

    let outcome = export::run(&project, &options)?;

    let mut record = AuditRecord::ok("export", "export");
    record.project = Some(project.root.display().to_string());
    record.chars = outcome.items.iter().map(|item| item.chars).sum();
    record.duration_ms = outcome.items.iter().map(|item| item.duration_ms).sum();
    record.cost_note = Some("export:local".to_string());
    audit::append(&paths.audit_file(), &record)?;

    let data = json!({
        "project": project.root.display().to_string(),
        "out_dir": outcome.out_dir.display().to_string(),
        "format": outcome.format,
        "layout": outcome.layout,
        "manifest": outcome.manifest.display().to_string(),
        "concat": outcome.concat.as_ref().map(|path| path.display().to_string()),
        "ffmpeg_used": outcome.ffmpeg_used,
        "count": outcome.items.len(),
        "items": outcome.items,
    });
    let human = format!(
        "已导出 {} 行 → {}（{} / {}）",
        data["count"],
        outcome.out_dir.display(),
        outcome.format,
        outcome.layout
    );
    Ok(CmdOutput { data, human })
}

fn export_timestamp() -> String {
    let raw = tts_core::util::now_rfc3339();
    let digits: String = raw.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 14 {
        format!("{}-{}", &digits[..8], &digits[8..14])
    } else {
        digits
    }
}

fn unique_export_dir(base: &Path) -> PathBuf {
    let stamp = export_timestamp();
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

fn is_secret_key(key: &str) -> bool {
    key == "api.api_key"
}

fn api_key_source(config: &Config) -> &'static str {
    let from_env = std::env::var(ENV_API_KEY)
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false);
    if from_env {
        "env"
    } else if config
        .api
        .api_key
        .as_deref()
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
    {
        "config"
    } else {
        "missing"
    }
}

fn emit_success(output: &CmdOutput, cli: &Cli) {
    if cli.json {
        let envelope = success_envelope(&output.data);
        println!("{}", serde_json::to_string(&envelope).unwrap_or_default());
    } else if !cli.quiet {
        println!("{}", output.human);
    }
}

fn emit_error(error: &TtsError, wants_json: bool) {
    if wants_json {
        println!(
            "{}",
            serde_json::to_string(&error.envelope()).unwrap_or_default()
        );
    } else {
        eprintln!("{error}");
    }
}

fn init_logging(verbose: bool) {
    if !verbose {
        return;
    }
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("debug"));
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(filter)
        .try_init();
}
