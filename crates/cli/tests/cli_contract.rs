//! CLI 契约测试：JSON 信封、退出码、schema 校验。
//!
//! 测试通过 `TTS_WORKSPACE_ROOT` 指向临时目录，完全隔离本机真实数据；
//! 不访问网络（`doctor --skip-network`）。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

mod support;

/// 仓库根目录（crates/cli → ../..）。
fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("仓库根目录不可用")
}

fn schema_path(relative: &str) -> PathBuf {
    repo_root().join("schemas").join(relative)
}

fn validate(relative: &str, instance: &Value) {
    let raw = std::fs::read_to_string(schema_path(relative))
        .unwrap_or_else(|e| panic!("schema 缺失 schemas/{relative}：{e}"));
    let schema: Value = serde_json::from_str(&raw).expect("schema 不是合法 JSON");
    let validator = jsonschema::validator_for(&schema).expect("schema 非法");
    if !validator.is_valid(instance) {
        let errors: Vec<String> = validator
            .iter_errors(instance)
            .map(|e| e.to_string())
            .collect();
        panic!("输出不符合 schemas/{relative}：\n{}", errors.join("\n"));
    }
}

struct Sandbox {
    dir: TempDir,
}

impl Sandbox {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("创建临时目录失败"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn config_file(&self) -> PathBuf {
        self.dir.path().join("config.json")
    }

    /// 构造在隔离工作区内运行的 `tts` 命令。
    fn cmd(&self) -> Command {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_tts"));
        cmd.env("TTS_WORKSPACE_ROOT", self.dir.path())
            .env_remove("TTS_CONFIG")
            .env_remove("MIMO_API_KEY")
            .env_remove("MIMO_BASE_URL")
            .env_remove("FFMPEG_PATH")
            .current_dir(self.dir.path());
        cmd
    }

    /// 运行并解析 stdout JSON。
    fn run_json(&self, args: &[&str]) -> (i32, Value) {
        let output = self.cmd().args(args).output().expect("无法启动 tts 进程");
        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!("stdout 不是 JSON：{e}\nargs={args:?}\nstdout={stdout}\nstderr={stderr}")
        });
        (code, json)
    }

    /// 指向本地 mock API 运行并解析 stdout JSON。
    fn run_json_with_api(&self, base_url: &str, api_key: &str, args: &[&str]) -> (i32, Value) {
        let mut cmd = self.cmd();
        cmd.env("MIMO_BASE_URL", base_url)
            .env("MIMO_API_KEY", api_key);
        let output = cmd.args(args).output().expect("无法启动 tts 进程");
        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let json: Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
            panic!("stdout 不是 JSON：{e}\nargs={args:?}\nstdout={stdout}\nstderr={stderr}")
        });
        (code, json)
    }

    /// 指向本地 mock API 运行，按行解析 stdout（JSONL：事件 + 最后一行为信封）。
    fn run_jsonl_with_api(
        &self,
        base_url: &str,
        api_key: &str,
        args: &[&str],
    ) -> (i32, Vec<Value>) {
        let mut cmd = self.cmd();
        cmd.env("MIMO_BASE_URL", base_url)
            .env("MIMO_API_KEY", api_key);
        let output = cmd.args(args).output().expect("无法启动 tts 进程");
        let code = output.status.code().unwrap_or(-1);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<Value> = stdout
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| {
                serde_json::from_str(line)
                    .unwrap_or_else(|e| panic!("输出行不是 JSON：{e}\n行：{line}\nstderr={stderr}"))
            })
            .collect();
        (code, lines)
    }
}

#[test]
fn doctor_json_is_valid_and_healthy() {
    let sandbox = Sandbox::new();
    let (code, json) = sandbox.run_json(&["--json", "doctor", "--skip-network"]);
    assert_eq!(code, 0, "doctor 应成功：{json}");
    assert_eq!(json["ok"], Value::Bool(true));
    validate("cli/doctor.schema.json", &json);
    assert_eq!(json["data"]["summary"]["fail"], Value::from(0));
    assert_eq!(json["data"]["summary"]["skipped"], Value::from(1));
}

#[test]
fn doctor_creates_workspace_layout() {
    let sandbox = Sandbox::new();
    let (code, _) = sandbox.run_json(&["--json", "doctor", "--skip-network"]);
    assert_eq!(code, 0);
    for relative in ["library/voices", "cache", "audit", "projects"] {
        assert!(
            sandbox.path().join(relative).is_dir(),
            "应创建目录 {relative}"
        );
    }
}

#[test]
fn version_json_is_valid() {
    let sandbox = Sandbox::new();
    let (code, json) = sandbox.run_json(&["--json", "version"]);
    assert_eq!(code, 0);
    validate("cli/version.schema.json", &json);
    assert_eq!(
        json["data"]["version"],
        Value::from(env!("CARGO_PKG_VERSION"))
    );
}

#[test]
fn config_set_get_roundtrip_with_masking() {
    let sandbox = Sandbox::new();

    let (code, json) = sandbox.run_json(&[
        "--json",
        "config",
        "set",
        "api.base_url",
        "http://127.0.0.1:18080/v1",
    ]);
    assert_eq!(code, 0);
    validate("cli/config-set.schema.json", &json);

    let (code, json) = sandbox.run_json(&["--json", "config", "get", "api.base_url"]);
    assert_eq!(code, 0);
    validate("cli/config-get.schema.json", &json);
    assert_eq!(
        json["data"]["value"],
        Value::from("http://127.0.0.1:18080/v1")
    );
    assert_eq!(json["data"]["masked"], Value::Bool(false));

    // Key：写入后默认打码，--reveal 显示完整值
    let (code, _) = sandbox.run_json(&[
        "--json",
        "config",
        "set",
        "api.api_key",
        "sk-test-1234567890",
    ]);
    assert_eq!(code, 0);

    let (code, json) = sandbox.run_json(&["--json", "config", "get", "api.api_key"]);
    assert_eq!(code, 0);
    assert_eq!(json["data"]["value"], Value::from("sk-t****7890"));
    assert_eq!(json["data"]["masked"], Value::Bool(true));

    let (code, json) = sandbox.run_json(&["--json", "config", "get", "api.api_key", "--reveal"]);
    assert_eq!(code, 0);
    assert_eq!(json["data"]["value"], Value::from("sk-test-1234567890"));
    assert_eq!(json["data"]["masked"], Value::Bool(false));

    let (code, json) = sandbox.run_json(&["--json", "config", "list"]);
    assert_eq!(code, 0);
    validate("cli/config-list.schema.json", &json);
    assert_eq!(json["data"]["exists"], Value::Bool(true));
    assert_eq!(json["data"]["api_key_source"], Value::from("config"));
    assert_eq!(
        json["data"]["config"]["api"]["api_key"],
        Value::from("sk-t****7890")
    );
}

#[test]
fn invalid_value_is_usage_error() {
    let sandbox = Sandbox::new();
    let (code, json) =
        sandbox.run_json(&["--json", "config", "set", "generation.concurrency", "abc"]);
    assert_eq!(code, 2);
    validate("common/envelope-error.schema.json", &json);
    assert_eq!(json["error"]["code"], Value::from("E_USAGE"));
}

#[test]
fn unknown_command_is_usage_error() {
    let sandbox = Sandbox::new();
    let (code, json) = sandbox.run_json(&["--json", "frobnicate"]);
    assert_eq!(code, 2);
    validate("common/envelope-error.schema.json", &json);
    assert_eq!(json["error"]["code"], Value::from("E_USAGE"));
}

#[test]
fn broken_config_is_parse_error_runtime_exit() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.config_file(), "{ broken").unwrap();
    let (code, json) = sandbox.run_json(&["--json", "config", "list"]);
    assert_eq!(code, 1);
    validate("common/envelope-error.schema.json", &json);
    assert_eq!(json["error"]["code"], Value::from("E_CONFIG_PARSE"));
}

#[test]
fn doctor_with_broken_config_is_unhealthy() {
    let sandbox = Sandbox::new();
    std::fs::write(sandbox.config_file(), "{ broken").unwrap();
    let (code, json) = sandbox.run_json(&["--json", "doctor", "--skip-network"]);
    assert_eq!(code, 1);
    validate("common/envelope-error.schema.json", &json);
    assert_eq!(json["error"]["code"], Value::from("E_DOCTOR_UNHEALTHY"));
    let failed: Vec<&str> = json["error"]["details"]["checks"]
        .as_array()
        .expect("details.checks 应为数组")
        .iter()
        .filter(|check| check["status"] == "fail")
        .filter_map(|check| check["name"].as_str())
        .collect();
    assert_eq!(failed, vec!["config"]);
}

#[test]
fn project_init_import_status_json_schemas() {
    let sandbox = Sandbox::new();
    let project_dir = sandbox.path().join("proj");
    let project_arg = project_dir.to_str().unwrap();

    let (code, json) = sandbox.run_json(&[
        "--json",
        "project",
        "init",
        project_arg,
        "--name",
        "雨夜车站",
    ]);
    assert_eq!(code, 0, "init 应成功：{json}");
    validate("cli/project-init.schema.json", &json);
    assert_eq!(json["data"]["name"], Value::from("雨夜车站"));

    let script = project_dir.join("script").join("chapter1.txt");
    std::fs::write(
        &script,
        "# 第一章\n## 雨夜\n阿岚: 又是这样的雨……\n小满: 哥，你还在等那班车吗？\n",
    )
    .unwrap();

    let (code, json) = sandbox.run_json(&[
        "--json",
        "project",
        "import",
        script.to_str().unwrap(),
        "--project",
        project_arg,
    ]);
    assert_eq!(code, 0, "import 应成功：{json}");
    validate("cli/project-import.schema.json", &json);
    assert_eq!(json["data"]["counts"]["lines"], Value::from(2));
    assert_eq!(json["data"]["diff"]["added"], Value::from(2));
    assert_eq!(json["data"]["unbound_roles"].as_array().unwrap().len(), 2);

    // 重复导入 → 未变化
    let (code, json) = sandbox.run_json(&[
        "--json",
        "project",
        "import",
        script.to_str().unwrap(),
        "--project",
        project_arg,
    ]);
    assert_eq!(code, 0);
    assert_eq!(json["data"]["diff"]["unchanged"], Value::from(2));

    let (code, json) = sandbox.run_json(&["--json", "project", "status", "--project", project_arg]);
    assert_eq!(code, 0);
    validate("cli/project-status.schema.json", &json);
    assert_eq!(json["data"]["counts"]["pending"], Value::from(2));
    assert_eq!(json["data"]["roles"].as_array().unwrap().len(), 2);
    assert_eq!(
        json["data"]["scripts"][0],
        Value::from("script/chapter1.txt")
    );
}

#[test]
fn project_init_existing_is_usage_error() {
    let sandbox = Sandbox::new();
    let project_dir = sandbox.path().join("proj");
    let project_arg = project_dir.to_str().unwrap();
    let (code, _) = sandbox.run_json(&["--json", "project", "init", project_arg]);
    assert_eq!(code, 0);
    let (code, json) = sandbox.run_json(&["--json", "project", "init", project_arg]);
    assert_eq!(code, 2);
    assert_eq!(json["error"]["code"], Value::from("E_USAGE"));
}

#[test]
fn project_import_missing_script_is_not_found() {
    let sandbox = Sandbox::new();
    let project_dir = sandbox.path().join("proj");
    let project_arg = project_dir.to_str().unwrap();
    let (code, _) = sandbox.run_json(&["--json", "project", "init", project_arg]);
    assert_eq!(code, 0);

    let missing = sandbox.path().join("nope.txt");
    let (code, json) = sandbox.run_json(&[
        "--json",
        "project",
        "import",
        missing.to_str().unwrap(),
        "--project",
        project_arg,
    ]);
    assert_eq!(code, 1);
    validate("common/envelope-error.schema.json", &json);
    assert_eq!(json["error"]["code"], Value::from("E_NOT_FOUND"));
}

#[test]
fn human_mode_writes_success_to_stdout_error_to_stderr() {
    let sandbox = Sandbox::new();

    let output = sandbox
        .cmd()
        .args(["doctor", "--skip-network"])
        .output()
        .expect("无法启动 tts");
    assert_eq!(output.status.code(), Some(0));
    assert!(!output.stdout.is_empty(), "人类可读输出应写到 stdout");
    assert!(
        serde_json::from_slice::<Value>(&output.stdout).is_err(),
        "非 --json 模式不应输出 JSON"
    );

    let output = sandbox
        .cmd()
        .args(["config", "set", "generation.concurrency", "abc"])
        .output()
        .expect("无法启动 tts");
    assert_eq!(output.status.code(), Some(2));
    assert!(output.stdout.is_empty(), "错误不应写 stdout");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("E_USAGE"), "stderr 应包含错误码：{stderr}");
}

#[test]
fn synth_generates_file_caches_and_audits() {
    let sandbox = Sandbox::new();
    let server = support::MockServer::start(vec![support::MockResponse::json(
        support::chat_response_with_wav(&support::tiny_wav()),
    )]);

    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &["--json", "synth", "--text", "早上好！", "--voice", "冰糖"],
    );
    assert_eq!(code, 0, "synth 应成功：{json}");
    validate("cli/synth.schema.json", &json);
    assert_eq!(json["data"]["cached"], Value::Bool(false));
    assert_eq!(json["data"]["duration_ms"], Value::from(100));
    assert_eq!(json["data"]["line_id"], Value::Null);
    assert_eq!(json["data"]["take_id"], Value::Null);
    assert_eq!(server.request_count(), 1);

    let output_path = json["data"]["output"].as_str().unwrap();
    assert!(
        std::path::Path::new(output_path).is_file(),
        "输出文件应存在：{output_path}"
    );

    let audit = std::fs::read_to_string(sandbox.path().join("audit/audit.jsonl")).unwrap();
    assert!(audit.contains("\"cmd\":\"synth\""));
    assert!(audit.contains("\"status\":\"ok\""));

    // 第二次：缓存命中，不重复请求
    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &["--json", "synth", "--text", "早上好！", "--voice", "冰糖"],
    );
    assert_eq!(code, 0);
    assert_eq!(json["data"]["cached"], Value::Bool(true));
    assert_eq!(server.request_count(), 1, "缓存命中不应产生新请求");

    // --no-cache：强制请求
    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "synth",
            "--text",
            "早上好！",
            "--voice",
            "冰糖",
            "--no-cache",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(json["data"]["cached"], Value::Bool(false));
    assert_eq!(server.request_count(), 2);
}

#[test]
fn synth_stream_consumes_sse() {
    let sandbox = Sandbox::new();
    let pcm = vec![0u8; 4_800]; // 100ms @24kHz
    let half = pcm.len() / 2;
    let server = support::MockServer::start(vec![support::MockResponse::sse(
        support::chat_sse_with_pcm(&[pcm[..half].to_vec(), pcm[half..].to_vec()]),
    )]);

    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "synth",
            "--text",
            "流式测试",
            "--voice",
            "冰糖",
            "--stream",
        ],
    );
    assert_eq!(code, 0, "流式 synth 应成功：{json}");
    validate("cli/synth.schema.json", &json);
    assert_eq!(json["data"]["stream"], Value::Bool(true));
    assert_eq!(json["data"]["duration_ms"], Value::from(100));
    assert_eq!(server.request_count(), 1);
}

#[test]
fn synth_line_mode_writes_take_and_updates_lines() {
    let sandbox = Sandbox::new();
    let server = support::MockServer::start(vec![support::MockResponse::json(
        support::chat_response_with_wav(&support::tiny_wav()),
    )]);
    let project_dir = sandbox.path().join("proj");
    let project_arg = project_dir.to_str().unwrap();

    let (code, _) = sandbox.run_json(&["--json", "project", "init", project_arg, "--name", "测试"]);
    assert_eq!(code, 0);
    let script = project_dir.join("script/chapter1.txt");
    std::fs::write(&script, "阿岚: 又是这样的雨……\n").unwrap();
    let (code, _) = sandbox.run_json(&[
        "--json",
        "project",
        "import",
        script.to_str().unwrap(),
        "--project",
        project_arg,
    ]);
    assert_eq!(code, 0);

    let lines: Value =
        serde_json::from_str(&std::fs::read_to_string(project_dir.join("lines.json")).unwrap())
            .unwrap();
    let line_id = lines["lines"][0]["id"].as_str().unwrap().to_string();

    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "synth",
            "--project",
            project_arg,
            "--line",
            &line_id,
            "--voice",
            "冰糖",
            "--style",
            "(低沉)",
        ],
    );
    assert_eq!(code, 0, "行合成应成功：{json}");
    validate("cli/synth.schema.json", &json);
    assert_eq!(json["data"]["line_id"], Value::from(line_id.clone()));
    assert_eq!(json["data"]["take_id"], Value::from("0001"));

    let lines: Value =
        serde_json::from_str(&std::fs::read_to_string(project_dir.join("lines.json")).unwrap())
            .unwrap();
    assert_eq!(lines["lines"][0]["status"], Value::from("ok"));
    assert_eq!(lines["lines"][0]["selected_take"], Value::from("0001"));
    assert_eq!(lines["lines"][0]["takes"].as_array().unwrap().len(), 1);
    let take_file = lines["lines"][0]["takes"][0]["file"].as_str().unwrap();
    assert!(
        project_dir.join(take_file).is_file(),
        "take 文件应存在：{take_file}"
    );

    // 再次生成：相同参数 → 缓存命中，新 take 序号递增
    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "synth",
            "--project",
            project_arg,
            "--line",
            &line_id,
            "--voice",
            "冰糖",
            "--style",
            "(低沉)",
        ],
    );
    assert_eq!(code, 0);
    assert_eq!(json["data"]["take_id"], Value::from("0002"));
    assert_eq!(json["data"]["cached"], Value::Bool(true));
    assert_eq!(server.request_count(), 1);
}

#[test]
fn synth_stream_rejected_for_design_model_and_audited() {
    let sandbox = Sandbox::new();
    let (code, json) = sandbox.run_json_with_api(
        "http://127.0.0.1:1/v1",
        "test-key",
        &[
            "--json",
            "synth",
            "--text",
            "测试",
            "--voice",
            "design:温柔女声",
            "--model",
            "mimo-v2.5-tts-voicedesign",
            "--stream",
        ],
    );
    assert_eq!(code, 2);
    validate("common/envelope-error.schema.json", &json);
    assert_eq!(json["error"]["code"], Value::from("E_CAPABILITY"));

    let audit = std::fs::read_to_string(sandbox.path().join("audit/audit.jsonl")).unwrap();
    assert!(audit.contains("E_CAPABILITY"), "失败也应写入审计：{audit}");
    assert!(audit.contains("\"status\":\"failed\""));
}

/// 建项目 + 导入脚本 + 绑定角色，返回 (项目目录, 项目参数, 行 ID 列表)。
fn setup_project(
    sandbox: &Sandbox,
    script: &str,
    bindings: &[(&str, &str)],
) -> (PathBuf, String, Vec<String>) {
    let project_dir = sandbox.path().join("proj");
    let project_arg = project_dir.to_str().unwrap().to_string();
    let (code, json) = sandbox.run_json(&[
        "--json",
        "project",
        "init",
        &project_arg,
        "--name",
        "批量测试",
    ]);
    assert_eq!(code, 0, "init 失败：{json}");
    let script_path = project_dir.join("script/chapter1.txt");
    std::fs::write(&script_path, script).unwrap();
    let (code, json) = sandbox.run_json(&[
        "--json",
        "project",
        "import",
        script_path.to_str().unwrap(),
        "--project",
        &project_arg,
    ]);
    assert_eq!(code, 0, "import 失败：{json}");
    for (role, voice) in bindings {
        let (code, json) = sandbox.run_json(&[
            "--json",
            "role",
            "bind",
            "--role",
            role,
            "--voice",
            voice,
            "--project",
            &project_arg,
        ]);
        assert_eq!(code, 0, "role bind 失败：{json}");
    }
    let lines: Value =
        serde_json::from_str(&std::fs::read_to_string(project_dir.join("lines.json")).unwrap())
            .unwrap();
    let ids = lines["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| line["id"].as_str().unwrap().to_string())
        .collect();
    (project_dir, project_arg, ids)
}

#[test]
fn role_bind_and_line_set_contracts() {
    let sandbox = Sandbox::new();
    let (_, project_arg, ids) = setup_project(&sandbox, "阿岚: 又是这样的雨……\n", &[]);

    let (code, json) = sandbox.run_json(&[
        "--json",
        "role",
        "bind",
        "--role",
        "阿岚",
        "--voice",
        "冰糖",
        "--style",
        "清亮",
        "--project",
        &project_arg,
    ]);
    assert_eq!(code, 0, "{json}");
    validate("cli/role-bind.schema.json", &json);
    assert_eq!(json["data"]["voice"], Value::from("冰糖"));
    assert_eq!(json["data"]["roles_total"], Value::from(1));

    let (code, json) = sandbox.run_json(&[
        "--json",
        "line",
        "set",
        "--line",
        &ids[0],
        "--style",
        "(低沉)",
        "--project",
        &project_arg,
    ]);
    assert_eq!(code, 0, "{json}");
    validate("cli/line-set.schema.json", &json);
    assert_eq!(json["data"]["overrides"]["style"], Value::from("(低沉)"));

    // 无修改参数 → E_USAGE
    let (code, json) = sandbox.run_json(&[
        "--json",
        "line",
        "set",
        "--line",
        &ids[0],
        "--project",
        &project_arg,
    ]);
    assert_eq!(code, 2);
    assert_eq!(json["error"]["code"], Value::from("E_USAGE"));

    // 未知行 → E_NOT_FOUND
    let (code, json) = sandbox.run_json(&[
        "--json",
        "line",
        "set",
        "--line",
        "L00000000",
        "--style",
        "x",
        "--project",
        &project_arg,
    ]);
    assert_eq!(code, 1);
    assert_eq!(json["error"]["code"], Value::from("E_NOT_FOUND"));
}

#[test]
fn batch_fails_fast_when_role_unbound() {
    let sandbox = Sandbox::new();
    let (_, project_arg, _) = setup_project(&sandbox, "阿岚: 台词\n", &[]);

    let (code, json) = sandbox.run_json(&["--json", "batch", "--project", &project_arg]);
    assert_eq!(code, 2);
    validate("common/envelope-error.schema.json", &json);
    assert_eq!(json["error"]["code"], Value::from("E_LINE_UNBOUND"));
    assert_eq!(
        json["error"]["details"]["line_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn batch_generates_with_jsonl_events() {
    let sandbox = Sandbox::new();
    let server = support::MockServer::start(vec![support::MockResponse::json(
        support::chat_response_with_wav(&support::tiny_wav()),
    )]);
    let (project_dir, project_arg, _) = setup_project(
        &sandbox,
        "阿岚: 第一句\n小满: 第二句\n",
        &[("阿岚", "冰糖"), ("小满", "苏打")],
    );

    let (code, lines) = sandbox.run_jsonl_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "batch",
            "--project",
            &project_arg,
            "--concurrency",
            "1",
            "--events",
            "jsonl",
        ],
    );
    assert_eq!(code, 0, "批量应成功：{lines:?}");

    let events: Vec<&str> = lines
        .iter()
        .filter_map(|line| line["event"].as_str())
        .collect();
    assert_eq!(events.first(), Some(&"started"));
    assert_eq!(events.last(), Some(&"finished"));
    assert_eq!(
        events
            .iter()
            .filter(|event| **event == "line_succeeded")
            .count(),
        2
    );

    // 最后一行是统一信封
    let envelope = lines.last().unwrap();
    validate("cli/batch.schema.json", envelope);
    assert_eq!(envelope["data"]["summary"]["succeeded"], Value::from(2));

    // take 文件落盘
    let stored: Value =
        serde_json::from_str(&std::fs::read_to_string(project_dir.join("lines.json")).unwrap())
            .unwrap();
    for line in stored["lines"].as_array().unwrap() {
        assert_eq!(line["status"], Value::from("ok"));
        let take_file = line["takes"][0]["file"].as_str().unwrap();
        assert!(project_dir.join(take_file).is_file(), "缺少 {take_file}");
    }
}

#[test]
fn batch_partial_failure_then_retry_succeeds() {
    let sandbox = Sandbox::new();
    let server = support::MockServer::start(vec![
        support::MockResponse::json(support::chat_response_with_wav(&support::tiny_wav())),
        support::MockResponse::error(500, r#"{"error":{"message":"boom"}}"#),
        support::MockResponse::json(support::chat_response_with_wav(&support::tiny_wav())),
    ]);
    // 关闭重试，避免测试等待退避
    for (key, value) in [
        ("generation.retry.max", "0"),
        ("generation.retry.backoff_ms", "0"),
    ] {
        let (code, json) = sandbox.run_json(&["--json", "config", "set", key, value]);
        assert_eq!(code, 0, "{json}");
    }

    let (project_dir, project_arg, _) = setup_project(
        &sandbox,
        "阿岚: 第一句\n小满: 第二句\n",
        &[("阿岚", "冰糖"), ("小满", "苏打")],
    );

    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "batch",
            "--project",
            &project_arg,
            "--concurrency",
            "1",
        ],
    );
    assert_eq!(code, 3, "部分失败应为退出码 3：{json}");
    validate("common/envelope-error.schema.json", &json);
    assert_eq!(json["error"]["code"], Value::from("E_PARTIAL"));
    assert_eq!(
        json["error"]["details"]["summary"]["failed"],
        Value::from(1)
    );
    assert_eq!(
        json["error"]["details"]["failed_lines"][0]["code"],
        Value::from("E_HTTP_STATUS")
    );

    // 失败行状态为 failed
    let (code, json) = sandbox.run_json(&["--json", "batch", "status", "--project", &project_arg]);
    assert_eq!(code, 0);
    validate("cli/batch-status.schema.json", &json);
    assert_eq!(json["data"]["counts"]["failed"], Value::from(1));
    assert_eq!(json["data"]["counts"]["ok"], Value::from(1));

    // 重置失败行
    let (code, json) = sandbox.run_json(&["--json", "batch", "retry", "--project", &project_arg]);
    assert_eq!(code, 0);
    validate("cli/batch-retry.schema.json", &json);
    assert_eq!(json["data"]["reset"], Value::from(1));

    // 重跑：跳过已 ok，失败行成功
    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "batch",
            "--project",
            &project_arg,
            "--concurrency",
            "1",
        ],
    );
    assert_eq!(code, 0, "重跑应成功：{json}");
    validate("cli/batch.schema.json", &json);
    assert_eq!(json["data"]["summary"]["succeeded"], Value::from(1));
    assert_eq!(json["data"]["summary"]["skipped"], Value::from(1));

    // 全部 ok
    let (code, json) = sandbox.run_json(&["--json", "batch", "status", "--project", &project_arg]);
    assert_eq!(code, 0);
    assert_eq!(json["data"]["counts"]["total"], Value::from(2));
    assert_eq!(json["data"]["counts"]["ok"], Value::from(2));

    let stored: Value =
        serde_json::from_str(&std::fs::read_to_string(project_dir.join("lines.json")).unwrap())
            .unwrap();
    assert_eq!(stored["lines"][1]["status"], Value::from("ok"));
    assert_eq!(stored["lines"][1]["takes"].as_array().unwrap().len(), 1);
}

#[test]
fn takes_cast_select_contracts() {
    let sandbox = Sandbox::new();
    let server = support::MockServer::start(vec![
        support::MockResponse::json(support::chat_response_with_wav(&support::tiny_wav())),
        support::MockResponse::json(support::chat_response_with_wav(&support::tiny_wav())),
        support::MockResponse::json(support::chat_response_with_wav(&support::tiny_wav())),
    ]);
    let (_, project_arg, ids) =
        setup_project(&sandbox, "阿岚: 又是这样的雨……\n", &[("阿岚", "冰糖")]);
    let line_id = ids[0].clone();

    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "synth",
            "--project",
            &project_arg,
            "--line",
            &line_id,
            "--voice",
            "冰糖",
        ],
    );
    assert_eq!(code, 0, "synth 失败：{json}");

    let (code, json) = sandbox.run_json(&[
        "--json",
        "takes",
        "list",
        "--project",
        &project_arg,
        "--line",
        &line_id,
    ]);
    assert_eq!(code, 0, "{json}");
    validate("cli/takes-list.schema.json", &json);
    assert_eq!(json["data"]["total"], Value::from(1));
    assert_eq!(json["data"]["takes"][0]["selected"], Value::Bool(true));
    assert_eq!(json["data"]["takes"][0]["exists"], Value::Bool(true));

    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &[
            "--json",
            "cast",
            "--project",
            &project_arg,
            "--line",
            &line_id,
            "--voices",
            "冰糖,茉莉",
        ],
    );
    assert_eq!(code, 0, "cast 失败：{json}");
    validate("cli/cast.schema.json", &json);
    assert_eq!(json["data"]["results"].as_array().unwrap().len(), 2);
    assert_eq!(json["data"]["results"][0]["take_id"], Value::from("0002"));
    assert_eq!(json["data"]["results"][1]["take_id"], Value::from("0003"));

    let (code, json) = sandbox.run_json(&[
        "--json",
        "takes",
        "list",
        "--project",
        &project_arg,
        "--line",
        &line_id,
    ]);
    assert_eq!(code, 0);
    assert_eq!(json["data"]["total"], Value::from(3));

    let (code, json) = sandbox.run_json(&[
        "--json",
        "takes",
        "select",
        "--project",
        &project_arg,
        "--line",
        &line_id,
        "--take",
        "0003",
    ]);
    assert_eq!(code, 0, "{json}");
    validate("cli/takes-select.schema.json", &json);
    assert_eq!(json["data"]["previous_selected"], Value::from("0001"));

    let (code, json) = sandbox.run_json(&[
        "--json",
        "takes",
        "select",
        "--project",
        &project_arg,
        "--line",
        &line_id,
        "--take",
        "9999",
    ]);
    assert_eq!(code, 1);
    assert_eq!(json["error"]["code"], Value::from("E_NOT_FOUND"));

    let (code, json) = sandbox.run_json(&[
        "--json",
        "takes",
        "list",
        "--project",
        &project_arg,
        "--line",
        "L00000000",
    ]);
    assert_eq!(code, 1);
    assert_eq!(json["error"]["code"], Value::from("E_NOT_FOUND"));
}

#[test]
fn usage_and_gc_contracts() {
    let sandbox = Sandbox::new();
    let server = support::MockServer::start(vec![support::MockResponse::json(
        support::chat_response_with_wav(&support::tiny_wav()),
    )]);

    let (code, json) = sandbox.run_json_with_api(
        &server.base_url,
        "test-key",
        &["--json", "synth", "--text", "早上好", "--voice", "冰糖"],
    );
    assert_eq!(code, 0, "{json}");
    let cache_key = json["data"]["cache_key"].as_str().unwrap().to_string();

    let (code, json) = sandbox.run_json(&["--json", "usage"]);
    assert_eq!(code, 0, "{json}");
    validate("cli/usage.schema.json", &json);
    assert_eq!(json["data"]["summary"]["requests"], Value::from(1));
    assert_eq!(json["data"]["summary"]["by_cmd"]["synth"], Value::from(1));

    let (code, json) = sandbox.run_json(&["--json", "gc", "--dry-run"]);
    assert_eq!(code, 0, "{json}");
    validate("cli/gc.schema.json", &json);
    assert_eq!(json["data"]["dry_run"], Value::Bool(true));
    assert_eq!(json["data"]["removed"], Value::from(1));
    assert!(
        sandbox.path().join("cache").join(&cache_key).is_dir(),
        "预览模式不应删除"
    );

    let (code, json) = sandbox.run_json(&["--json", "gc"]);
    assert_eq!(code, 0, "{json}");
    assert_eq!(json["data"]["removed"], Value::from(1));
    assert!(!sandbox.path().join("cache").join(&cache_key).is_dir());

    let (code, json) = sandbox.run_json(&["--json", "gc"]);
    assert_eq!(code, 0);
    assert_eq!(json["data"]["removed"], Value::from(0));
}

/// 本机是否具备 ffmpeg（缺失时跳过相关导出测试）。
fn ffmpeg_available(sandbox: &Sandbox) -> bool {
    let (code, json) = sandbox.run_json(&["--json", "doctor", "--skip-network"]);
    assert_eq!(code, 0, "{json}");
    json["data"]["checks"]
        .as_array()
        .unwrap()
        .iter()
        .any(|check| check["name"] == "ffmpeg" && check["status"] == "ok")
}

fn batch_all(sandbox: &Sandbox, base_url: &str, project_arg: &str) -> Value {
    let (code, json) = sandbox.run_json_with_api(
        base_url,
        "test-key",
        &[
            "--json",
            "batch",
            "--project",
            project_arg,
            "--concurrency",
            "1",
        ],
    );
    assert_eq!(code, 0, "批量生成失败：{json}");
    json
}

#[test]
fn export_wav_flat_and_overwrite_guard() {
    let sandbox = Sandbox::new();
    let server = support::MockServer::start(vec![support::MockResponse::json(
        support::chat_response_with_wav(&support::tiny_wav()),
    )]);
    let (project_dir, project_arg, _) = setup_project(
        &sandbox,
        "阿岚: 第一句\n小满: 第二句\n",
        &[("阿岚", "冰糖"), ("小满", "苏打")],
    );
    batch_all(&sandbox, &server.base_url, &project_arg);

    let (code, json) = sandbox.run_json(&["--json", "export", "--project", &project_arg]);
    assert_eq!(code, 0, "导出失败：{json}");
    validate("cli/export.schema.json", &json);
    assert_eq!(json["data"]["format"], Value::from("wav"));
    assert_eq!(json["data"]["count"], Value::from(2));
    assert_eq!(json["data"]["ffmpeg_used"], Value::Bool(false));
    assert_eq!(json["data"]["concat"], Value::Null);

    let out_dir = PathBuf::from(json["data"]["out_dir"].as_str().unwrap());
    assert!(out_dir.starts_with(&project_dir));
    let manifest = PathBuf::from(json["data"]["manifest"].as_str().unwrap());
    assert!(manifest.is_file(), "manifest 应存在");
    for item in json["data"]["items"].as_array().unwrap() {
        let file = out_dir.join(item["file"].as_str().unwrap());
        assert!(file.is_file(), "导出文件缺失：{}", file.display());
    }

    // 覆盖保护：非空输出目录 + 未确认 → 退出码 4
    let out_arg = out_dir.to_str().unwrap();
    let (code, json) = sandbox.run_json(&[
        "--json",
        "export",
        "--project",
        &project_arg,
        "--out",
        out_arg,
    ]);
    assert_eq!(code, 4, "{json}");
    assert_eq!(json["error"]["code"], Value::from("E_CONFIRM_REQUIRED"));

    // --yes 覆盖成功
    let (code, json) = sandbox.run_json(&[
        "--json",
        "export",
        "--project",
        &project_arg,
        "--out",
        out_arg,
        "--yes",
    ]);
    assert_eq!(code, 0, "{json}");
    assert_eq!(json["data"]["count"], Value::from(2));
}

#[test]
fn export_mp3_role_layout_with_concat() {
    let sandbox = Sandbox::new();
    if !ffmpeg_available(&sandbox) {
        eprintln!("跳过：本机未安装 ffmpeg");
        return;
    }
    let server = support::MockServer::start(vec![support::MockResponse::json(
        support::chat_response_with_wav(&support::tone_wav(1_000)),
    )]);
    let (_, project_arg, _) = setup_project(
        &sandbox,
        "阿岚: 第一句\n小满: 第二句\n",
        &[("阿岚", "冰糖"), ("小满", "苏打")],
    );
    batch_all(&sandbox, &server.base_url, &project_arg);

    let (code, json) = sandbox.run_json(&[
        "--json",
        "export",
        "--project",
        &project_arg,
        "--format",
        "mp3",
        "--layout",
        "role",
        "--concat",
        "--normalize",
        "--trim-silence",
    ]);
    assert_eq!(code, 0, "mp3 导出失败：{json}");
    validate("cli/export.schema.json", &json);
    assert_eq!(json["data"]["format"], Value::from("mp3"));
    assert_eq!(json["data"]["layout"], Value::from("role"));
    assert_eq!(json["data"]["ffmpeg_used"], Value::Bool(true));

    let out_dir = PathBuf::from(json["data"]["out_dir"].as_str().unwrap());
    for item in json["data"]["items"].as_array().unwrap() {
        let file = item["file"].as_str().unwrap();
        assert!(file.ends_with(".mp3"), "应为 mp3：{file}");
        assert!(file.contains('/'), "role 布局应分子目录：{file}");
        let path = out_dir.join(file);
        assert!(path.is_file());
        assert!(
            std::fs::metadata(&path).unwrap().len() > 0,
            "mp3 不应为空文件"
        );
    }

    let concat = json["data"]["concat"].as_str().expect("应有拼接文件");
    let concat_path = out_dir.join(concat);
    assert!(concat_path.is_file(), "拼接文件缺失");
    assert!(concat.ends_with("full.mp3"));

    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(out_dir.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["items"].as_array().unwrap().len(), 2);
    assert_eq!(manifest["concat"], Value::from("full.mp3"));
    assert_eq!(manifest["format"], Value::from("mp3"));
}
