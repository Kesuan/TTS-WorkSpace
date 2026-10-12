//! CLI 契约测试：JSON 信封、退出码、schema 校验。
//!
//! 测试通过 `TTS_WORKSPACE_ROOT` 指向临时目录，完全隔离本机真实数据；
//! 不访问网络（`doctor --skip-network`）。

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;
use tempfile::TempDir;

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
