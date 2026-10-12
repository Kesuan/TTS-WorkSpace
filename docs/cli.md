# CLI 参考（`tts`）

> 面向 Agent 与脚本的稳定契约。**本文件与实现、`schemas/` 必须保持同步**；变更即视为契约变更。
> 状态标注：✅ 已实现（M0）· 🚧 计划中（M1 及以后）。

## 退出码契约

| 码 | 含义 | 场景示例 |
| --- | --- | --- |
| `0` | 成功（含“仅告警”） | 命令正常完成 |
| `1` | 运行时失败 | 网络错误、配置解析失败、`doctor` 存在 fail 项 |
| `2` | 用法 / 校验错误 | 未知参数、非法取值、缺少必填项 |
| `3` | 部分失败 | 批量生成中部分条目失败（批量本身完成） |
| `4` | 需要显式确认 | 危险操作未携带 `--yes` |

## JSON 信封

所有命令支持全局旗标 `--json`。**带 `--json` 时，无论成功失败都在 stdout 输出单个 JSON 对象**；不带时，成功输出人类可读文本到 stdout，失败输出到 stderr。

成功：

```json
{ "ok": true, "data": { } }
```

失败（schema：[`schemas/common/envelope-error.schema.json`](../schemas/common/envelope-error.schema.json)）：

```json
{
  "ok": false,
  "error": {
    "code": "E_USAGE",
    "message": "generation.concurrency 需要整数，收到：abc",
    "hint": "取值范围 1~16",
    "details": { }
  }
}
```

- `error.code`：`^E_[A-Z0-9_]+$`，见错误码表。
- `error.hint` / `error.details`：可选；`details` 恒为对象。

### 错误码表

| 错误码 | 退出码 | 含义 | 状态 |
| --- | --- | --- | --- |
| `E_USAGE` | 2 | 参数 / 取值非法，或未知配置项 | ✅ |
| `E_IO` | 1 | 通用 I/O 失败 | ✅ |
| `E_JSON` | 1 | 内部 JSON 序列化失败 | ✅ |
| `E_WORKSPACE` | 1 | 工作区目录不可用 | ✅ |
| `E_CONFIG_IO` | 1 | 配置文件读写失败 | ✅ |
| `E_CONFIG_PARSE` | 1 | 配置文件解析失败 | ✅ |
| `E_CONFIG_KEY` | 1 | 未配置 API Key | ✅ |
| `E_HTTP_NETWORK` | 1 | 网络层失败（超时 / 连接） | ✅ |
| `E_HTTP_STATUS` | 1 | API 返回非 2xx | ✅ |
| `E_HTTP_RATE_LIMIT` | 1 | 429 限流（重试后仍失败） | ✅ |
| `E_API_RESPONSE` | 1 | 响应结构不符合预期 | ✅ |
| `E_AUDIO_DECODE` | 1 | 音频解码失败（base64 / WAV / PCM） | ✅ |
| `E_AUDIO_ENCODE` | 1 | WAV 编码失败 | ✅ |
| `E_CACHE_IO` | 1 | 缓存读写失败 | ✅ |
| `E_FFMPEG_MISSING` | 1 | 需要 ffmpeg 但未找到 | 🚧 |
| `E_FFMPEG_FAILED` | 1 | ffmpeg 执行失败 | 🚧 |
| `E_NOT_FOUND` | 1 | 目标不存在（项目 / 行 / take / 音色 / 脚本） | ✅ |
| `E_PARSE_SCRIPT` | 1 | 对白脚本解析失败（details 带 file/line/col） | ✅ |
| `E_CAPABILITY` | 2 | 模型不支持该能力（如对设计模型使用 `--stream`） | ✅ |
| `E_CONFIRM_REQUIRED` | 4 | 危险操作需要 `--yes` | 🚧 |
| `E_DOCTOR_UNHEALTHY` | 1 | `doctor` 存在失败项（明细在 `details`） | ✅ |
| `E_NOT_IMPLEMENTED` | 2 | 功能未实现 | ✅（保留） |

## 全局旗标

| 旗标 | 说明 |
| --- | --- |
| `--json` | stdout 输出统一信封（机器可读） |
| `--quiet` | 人类可读模式下仅输出错误 |
| `--verbose` | stderr 输出调试日志（`RUST_LOG` 可覆盖） |
| `--yes` | 危险操作确认（🚧 M1 起由删除类命令使用） |
| `-h, --help` / `-V, --version` | 帮助 / 版本 |

## 环境变量

| 变量 | 作用 | 优先级 |
| --- | --- | --- |
| `TTS_WORKSPACE_ROOT` | 工作区根目录（数据目录） | 高于配置文件中的 `workspace_root` |
| `TTS_CONFIG` | 配置文件路径 | 高于默认 `<root>/config.json` |
| `MIMO_API_KEY` | API Key | 高于 `config.json` 中的 `api.api_key` |
| `MIMO_BASE_URL` | API 地址（测试指向本地 mock 用） | 高于 `api.base_url` |
| `FFMPEG_PATH` | ffmpeg 可执行文件路径 | 高于 `ffmpeg.path` 配置 |
| `RUST_LOG` | 日志过滤（配合 `--verbose`） | — |

---

## 已实现命令（M0）

### `tts version`

输出名称、版本与解析后的工作区路径。

- Schema：[`schemas/cli/version.schema.json`](../schemas/cli/version.schema.json)
- 示例：

```json
{ "ok": true, "data": { "name": "tts", "version": "0.1.0", "workspace_root": "/Users/me/.tts-workbench" } }
```

### `tts doctor`

环境自检，检查项（固定顺序）：`workspace` · `config` · `api_key` · `ffmpeg` · `network`。

- 选项：`--skip-network` 跳过网络连通性检查（离线 / CI 推荐）。
- 状态语义：`ok` 正常 · `warn` 可继续（如未配置 Key）· `fail` 阻断 · `skipped` 主动跳过。
- 退出码：无 `fail` → `0`（含仅有 `warn`）；存在 `fail` → `1`，错误码 `E_DOCTOR_UNHEALTHY`，完整报告在 `error.details`。
- Schema：[`schemas/cli/doctor.schema.json`](../schemas/cli/doctor.schema.json)
- 成功示例：

```json
{
  "ok": true,
  "data": {
    "version": "0.1.0",
    "workspace_root": "/Users/me/.tts-workbench",
    "config_path": "/Users/me/.tts-workbench/config.json",
    "checks": [
      { "name": "workspace", "status": "ok", "detail": "已就绪：/Users/me/.tts-workbench" },
      { "name": "config", "status": "warn", "detail": "未找到配置（使用默认值）：…", "hint": "运行 `tts config set api.api_key <KEY>` 写入配置" },
      { "name": "api_key", "status": "warn", "detail": "未配置 API Key", "hint": "…" },
      { "name": "ffmpeg", "status": "ok", "detail": "已找到：/opt/homebrew/bin/ffmpeg" },
      { "name": "network", "status": "skipped", "detail": "已跳过（--skip-network）" }
    ],
    "summary": { "ok": 2, "warn": 2, "fail": 0, "skipped": 1 }
  }
}
```

### `tts config`

`list` / `get <key>` / `set <key> <value>`。

- `api.api_key` 在输出中默认打码（`前4****后4`，长度 ≤8 显示 `****`）；`--reveal` 显示完整值。
- `set` 对字段做类型与范围校验，非法取值返回 `E_USAGE`（退出码 2）。
- 写入使用 JSON 美化格式；Unix 下文件权限 0600。
- 配置项键（即 `config.json` 字段，含范围）：

| 键 | 类型 / 范围 | 默认值 |
| --- | --- | --- |
| `schema_version` | 只读 | `1` |
| `api.base_url` | 非空字符串 | `https://api.xiaomimimo.com/v1` |
| `api.api_key` | 字符串；空值清除 | 无 |
| `api.timeout_ms` | 1000 ~ 600000 | `120000` |
| `models.tts` | 非空字符串 | `mimo-v2.5-tts` |
| `models.design` | 非空字符串 | `mimo-v2.5-tts-voicedesign` |
| `models.clone` | 非空字符串 | `mimo-v2.5-tts-voiceclone` |
| `generation.concurrency` | 1 ~ 16 | `3` |
| `generation.max_chars_per_request` | 1 ~ 8000 | `500` |
| `generation.retry.max` | 0 ~ 10 | `3` |
| `generation.retry.backoff_ms` | 0 ~ 60000 | `800` |
| `ffmpeg.path` | 路径或 `auto` | `auto` |
| `workspace_root` | 路径；空值清除 | 无 |

- Schema：[`config-list`](../schemas/cli/config-list.schema.json) · [`config-get`](../schemas/cli/config-get.schema.json) · [`config-set`](../schemas/cli/config-set.schema.json)

### `tts project`

项目（= 作品）管理：`init` / `import` / `status`。

- `tts project init <路径> [--name <作品名>]`：创建项目目录结构（`project.json` / `script/` / `takes/` / `exports/` / `lines.json`）。目录已存在项目时报 `E_USAGE`（退出码 2）。Schema：[`project-init`](../schemas/cli/project-init.schema.json)
- `tts project import <脚本> [--project <目录>] [--no-narrator-fallback] [--join-with-space]`：解析对白脚本（格式见 [docs/script-format.md](script-format.md)），写入 `lines.json`，输出解析报告与 diff。Schema：[`project-import`](../schemas/cli/project-import.schema.json)

```json
{
  "ok": true,
  "data": {
    "script": "script/chapter1.txt",
    "counts": { "lines": 42, "roles": 3, "chapters": 1, "scenes": 2, "narrator_lines": 5 },
    "roles": ["阿岚", "小满", "旁白"],
    "unbound_roles": ["小满"],
    "warnings": [{ "line": 7, "code": "W_ORPHAN_CONTINUATION", "message": "缩进行没有可续接的台词，已忽略" }],
    "diff": { "added": 42, "removed": 0, "changed": 0, "unchanged": 0 }
  }
}
```

`--project` 默认当前目录；脚本路径不存在 → `E_NOT_FOUND`；角色名超长等格式错误 → `E_PARSE_SCRIPT`（`details.file/line/col`）。

- `tts project status [--project <目录>]`：条目状态分布与角色绑定概览。Schema：[`project-status`](../schemas/cli/project-status.schema.json)

---

## 计划中命令（🚧 M1 / M2）

> 以下命令尚未实现，接口可能微调；实现后将补全参数表与 Schema。

| 命令 | 说明 |
| --- | --- |
| `tts synth --text … --voice … [--style …] [--stream] [--play]` | 单条合成（流式仅预置音色模型；`--play` 本机播放） |
| `tts voice list\|show\|rm` | 全局音色库（预置 / 设计 / 克隆） |
| `tts design --prompt … --text … [--optimize-text-preview]` | 文本设计音色并试听 |
| `tts design solidify --take <id> --name <名>` | 设计 → 克隆固化（保证跨句一致） |
| `tts clone --sample <文件> --name <名>` | 创建克隆音色（≤10MB、wav/mp3、base64 data URL） |
| `tts role bind --role <名> --voice <ref> [--style …]` | 角色 ↔ 音色绑定 |
| `tts line set --line <id> [--style …] [--voice <ref>]` | 行级覆盖（写入 lines.json） |
| `tts project rm` | 删除项目（需 `--yes`） |
| `tts batch --all\|--scene <名> [--concurrency N] [--events jsonl]` | 批量生成（幂等；断点续跑；缓存去重） |
| `tts batch status\|retry` | 批量状态 / 重试失败项 |
| `tts takes list\|select --line <id>` | Take 列表与选用 |
| `tts cast --line <id> --voices <a,b,c>` | 同句多音色对比（选角） |
| `tts export [--format mp3] [--layout role\|scene\|flat] [--concat] [--manifest]` | 导出（逐行文件 / 分目录 / 整轨 / 清单） |
| `tts usage [--since YYYY-MM]` | 用量统计（来自审计日志） |
| `tts gc` | 清理未引用缓存与孤儿 take |

### `--events jsonl` 事件流（🚧 M1）

事件逐行输出（JSONL），供 Agent 实时消费：

```jsonl
{"event":"started","line_id":"L3f2a9c01","total":42}
{"event":"succeeded","line_id":"L3f2a9c01","take_id":"0001","duration_ms":1820,"cached":false}
{"event":"cached","line_id":"L5b1c0d22","take_id":"0002"}
{"event":"failed","line_id":"L9e0f4a77","code":"E_HTTP_RATE_LIMIT","message":"…"}
{"event":"finished","succeeded":40,"failed":2}
```
