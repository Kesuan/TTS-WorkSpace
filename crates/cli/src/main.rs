//! `tts` 命令行入口（Agent 主接口）。
//!
//! 契约（详见 `docs/cli.md`）：
//! - `--json`：stdout 输出统一信封 `{"ok":true,"data":…}` 或 `{"ok":false,"error":…}`；
//! - 非 `--json`：成功输出到 stdout（人类可读），失败输出到 stderr；
//! - 退出码：0 成功 / 1 运行时失败 / 2 用法与校验错误 / 3 部分失败 / 4 需要确认。

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};
use serde_json::{json, Value};

use tts_core::config::{mask_secret, Config, ENV_API_KEY};
use tts_core::doctor::{self, CheckStatus};
use tts_core::error::{success_envelope, Result, TtsError, E_DOCTOR_UNHEALTHY, E_USAGE};
use tts_core::paths::Paths;
use tts_core::project::{Project, STATUS_FAILED, STATUS_OK, STATUS_PENDING, STATUS_REMOVED};
use tts_core::script::ParseOptions;

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

    /// 危险操作确认（非交互环境必需；v0.1 危险命令见 docs/cli.md）
    #[arg(long, global = true)]
    #[allow(dead_code)] // M1 删除类命令启用
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
