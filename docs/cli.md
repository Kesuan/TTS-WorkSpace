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
| `E_FFMPEG_MISSING` | 1 | 需要 ffmpeg 但未找到 | ✅ |
| `E_FFMPEG_FAILED` | 1 | ffmpeg 执行失败（details/消息含 stderr 摘要） | ✅ |
| `E_NOT_FOUND` | 1 | 目标不存在（项目 / 行 / take / 音色 / 脚本） | ✅ |
| `E_PARSE_SCRIPT` | 1 | 对白脚本解析失败（details 带 file/line/col） | ✅ |
| `E_LINE_UNBOUND` | 2 | 批量前置校验：存在未绑定音色的行 | ✅ |
| `E_PARTIAL` | 3 | 批量部分失败（details 带 summary 与 failed_lines） | ✅ |
| `E_CAPABILITY` | 2 | 模型不支持该能力（如对设计模型使用 `--stream`） | ✅ |
| `E_CONFIRM_REQUIRED` | 4 | 危险操作需要 `--yes`（覆盖非空导出目录等） | ✅ |
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

### `tts synth`

单条合成（**幂等**：相同参数命中缓存不重复请求）。

| 参数 | 说明 |
| --- | --- |
| `--text <文本>` | 待合成文本；使用 `--line` 时可省略（取行文本） |
| `--voice <引用>` | 音色引用（语法见下）；省略时按「行级覆盖 > 角色绑定」解析 |
| `--style <指令>` | 风格指令（自然语言，写入 user 消息） |
| `--model <ID>` | 模型 ID（默认取配置 `models.tts`） |
| `--stream` | 低延迟流式输出（仅预置音色模型；仍落盘完整 wav） |
| `--no-cache` | 跳过缓存，强制请求 |
| `--out <文件>` | 输出文件（独立模式；默认 `<工作区>/synth/<cache_key>.wav`） |
| `--project <目录>` | 项目目录（配合 `--line` 使用，默认当前目录） |
| `--line <ID>` | 行 ID：写入 take、更新行状态与选中项 |

**音色引用语法**：

| 写法 | 含义 |
| --- | --- |
| `<预置名或 ID>` | 预置音色（如 `冰糖`、`mimo_default`）或**音色库内名称/ID** |
| `preset:<名>` | 显式预置音色（MiMo 新增预置音色可用它直通） |
| `design:<音色描述>` | 内联文本设计音色 |
| `clone:<音频文件路径>` | 内联克隆（wav/mp3，base64 后 ≤10MB） |

- 名称解析顺序：预置音色 → 音色库（design/clone/preset）→ 报 `E_NOT_FOUND`（附 `tts voice list` 提示）。
- **模型按音色类型自动选择**：预置 → `models.tts`；设计/克隆 → `models.voiceclone` / `models.design`；显式 `--model` 优先。

```json
{
  "ok": true,
  "data": {
    "output": "/Users/me/.tts-workbench/synth/9c1e0b7f3a2d4e56.wav",
    "model": "mimo-v2.5-tts",
    "voice": "冰糖",
    "chars": 4,
    "duration_ms": 1200,
    "latency_ms": 880,
    "cached": false,
    "cache_key": "9c1e0b7f3a2d4e56",
    "sample_rate": 24000,
    "stream": false,
    "line_id": null,
    "take_id": null
  }
}
```

行为说明（Schema：[`synth`](../schemas/cli/synth.schema.json)）：

- **缓存**：指纹 = 模型 / 音色 / 文本 / 风格 / 格式 / 流式标志（空白差异不影响）；命中时 `cached: true` 且 `latency_ms: 0`。
- **审计**：成功与失败都会写入 `audit/audit.jsonl`（`cmd: "synth"`；失败记录带 `error_code`）。
- **行模式**：take 写入 `takes/<line_id>/<NNNN>_<hash8>.wav`；首条自动设为 `selected_take`，行状态更新为 `ok`。`--out` 与 `--line` 互斥。
- 行已被标记 `removed` → `E_USAGE`；行不存在 → `E_NOT_FOUND`。

### `tts role bind`

绑定角色 ↔ 音色与默认风格（项目级；批量生成时自动套用）。

```bash
tts role bind --role 阿岚 --voice 冰糖 --style "年轻的清亮女声，语速偏快" --project ./works/rainy-station
```

- 至少需要 `--voice` 或 `--style` 之一（否则 `E_USAGE`）。
- `--voice` 会在绑定前先校验可解析（预置 / 音色库 / 内联语法），拼错立即报 `E_NOT_FOUND`。
- Schema：[`role-bind`](../schemas/cli/role-bind.schema.json)

### `tts line set`

行级覆盖（音色 / 风格），写入 `lines.json` 的 `overrides`；优先级：行级覆盖 > 角色绑定。

```bash
tts line set --line L3f2a9c01 --style "(低语) 我没事，真的。" --project ./works/rainy-station
tts line set --line L3f2a9c01 --clear-style                        # 清除行级风格
```

- 无任何修改参数 → `E_USAGE`；行不存在 → `E_NOT_FOUND`；removed 行 → `E_USAGE`。
- Schema：[`line-set`](../schemas/cli/line-set.schema.json)

### `tts batch`

批量生成：并发限流、退避重试、幂等续跑、事件流。

| 参数 | 说明 |
| --- | --- |
| `--project <目录>` | 项目目录（默认当前目录） |
| `--line <ID>`（可重复） | 仅生成指定行；**显式指定即视为重新生成**（已完成行也会重跑） |
| `--scene <名>` | 仅生成指定场景 |
| `--concurrency <N>` | 并发数（默认取配置 `generation.concurrency`） |
| `--model <ID>` | 模型 ID（默认配置 `models.tts`） |
| `--force` | 忽略已完成状态，全部重新生成 |
| `--no-cache` | 跳过缓存 |
| `--events jsonl` | 事件流输出（需同时 `--json`；见下） |

行为契约：

- **默认目标**：状态非 `ok` 的非 removed 行；`ok` 行计入 `skipped`（不产生请求）。
- **fail-fast**：任一目标行未绑定音色 → `E_LINE_UNBOUND`（退出码 2），**不生成任何内容**。
- **重试**：`E_HTTP_NETWORK` / `E_HTTP_RATE_LIMIT` / 5xx 按指数退避重试（次数与基数见配置 `generation.retry`）。
- **幂等**：已完成行跳过；相同参数命中缓存；失败行标记 `failed`，重跑时自动重试。
- **部分失败**：全部完成后退出码 3（`E_PARTIAL`），`error.details` 为 `{project, summary, failed_lines}`。
- 成功输出 Schema：[`batch`](../schemas/cli/batch.schema.json)

`--events jsonl`：**逐行 JSON 输出事件，最后一行是统一信封**（因此需 `--json`）。事件类型：

```jsonl
{"event":"started","total":42,"concurrency":3,"skipped":2}
{"event":"line_skipped","line_id":"L1111","role":"阿岚","reason":"already_ok"}
{"event":"line_started","line_id":"L2222","role":"阿岚"}
{"event":"line_succeeded","line_id":"L2222","role":"阿岚","take_id":"0001","duration_ms":1820,"latency_ms":880,"cached":false}
{"event":"line_failed","line_id":"L3333","role":"小满","code":"E_HTTP_STATUS","message":"…","attempts":4}
{"event":"finished","succeeded":40,"failed":1,"skipped":1,"cached":12,"total_duration_ms":52310}
{"ok":true,"data":{"project":"…","summary":{…},"failed_lines":[]}}
```

### `tts batch status` / `tts batch retry`

```bash
tts batch status --project ./works/rainy-station --json   # 状态分布 + 失败行清单 + 未绑定角色
tts batch retry  --project ./works/rainy-station --json   # 失败行重置为 pending（--line 可限定）
```

- `status` Schema：[`batch-status`](../schemas/cli/batch-status.schema.json)
- `retry` Schema：[`batch-retry`](../schemas/cli/batch-retry.schema.json)；无失败条目时返回 `reset: 0`（成功）。

### `tts takes`

Take 管理：`list` / `select`。

```bash
tts takes list --project ./works/rainy-station --line L3f2a9c01 --json   # 可按行过滤
tts takes select --project ./works/rainy-station --line L3f2a9c01 --take 0002
```

- `list` 输出每条 take 的 `selected`（是否当前选用）与 `exists`（文件是否存在）。Schema：[`takes-list`](../schemas/cli/takes-list.schema.json)
- `select` 设置 `selected_take`（导出使用选中 take）。不存在的行/take → `E_NOT_FOUND`。Schema：[`takes-select`](../schemas/cli/takes-select.schema.json)

### `tts cast`

同句多音色对比（选角）：对同一行文本、使用多个音色各生成一条 take。

```bash
tts cast --project ./works/rainy-station --line L3f2a9c01 --voices 冰糖,茉莉,苏打 --json
```

- 风格取 `--style`，缺省时用「行级覆盖 > 角色绑定」。
- 任一音色失败即中止（已生成的 take 保留）；成功结果 Schema：[`cast`](../schemas/cli/cast.schema.json)

### `tts usage`

用量统计（聚合 `audit/audit.jsonl`）。

```bash
tts usage --since 2026-10 --json
```

- `--since` 为时间戳前缀过滤（`2026-10` / `2026-10-12`）。
- 输出：请求数 / 成功 / 失败 / 缓存命中、字符数、音频总时长、按命令与模型分布、缓存命中率、费用备注。Schema：[`usage`](../schemas/cli/usage.schema.json)

### `tts gc`

清理**未被任何项目引用**的缓存目录（`cache/<key16>/`）。

```bash
tts gc --dry-run --json     # 预览
tts gc --json               # 执行
tts gc --project ./works/rainy-station --json   # 额外扫描指定项目（默认扫描 <工作区>/projects/*）
```

- 引用来源：项目 `lines.json` 中各 take 的 `cache_key`。
- Schema：[`gc`](../schemas/cli/gc.schema.json)

### `tts export`

导出已生成音频（逐行文件 / 目录布局 / 整轨拼接 / 清单）。

| 参数 | 说明 |
| --- | --- |
| `--project <目录>` | 项目目录（默认当前目录） |
| `--out <目录>` | 输出目录（默认 `<项目>/exports/<时间戳>`；非空时需 `--yes`） |
| `--format wav\|mp3` | 输出格式（默认 wav；wav 无处理时直接复制，零转码） |
| `--layout flat\|role\|scene` | 目录布局（默认 flat；role/scene 按角色/场景分子目录） |
| `--name <模板>` | 文件名模板，占位符：`{seq}` `{role}` `{line}` `{take}` `{scene}` `{chapter}`（默认 `{seq}_{role}_{line}`） |
| `--concat` | 生成整轨拼接文件 `full.<ext>`（按行序） |
| `--normalize` | 响度归一化（ffmpeg loudnorm，-16 LUFS） |
| `--trim-silence` | 裁剪首尾静音（ffmpeg silenceremove，-45dB） |
| `--line <ID>` / `--scene <名>` | 仅导出指定行 / 场景 |

行为契约（Schema：[`export`](../schemas/cli/export.schema.json)）：

- **导出对象**：非 removed 且有 `selected_take` 的行（用 `tts takes select` 决定）；pending 行默认跳过，显式 `--line` 指定 pending 行 → `E_USAGE`。
- **take 文件缺失** → `E_NOT_FOUND`（提示重新生成）。
- **ffmpeg 依赖**：`--format mp3` / `--concat` / `--normalize` / `--trim-silence` 需要 ffmpeg；缺失 → `E_FFMPEG_MISSING`（wav 直复制路径不需要）。
- **覆盖保护**：`--out` 指向非空目录且未加 `--yes` → `E_CONFIRM_REQUIRED`（退出码 4）。
- **清单**：始终写入 `manifest.json`（`items` 含 `line_id / role / scene / chapter / index / take_id / file / duration_ms / chars`，供下游工程消费）。
- 导出同样写入审计（`cmd: "export"`）。

输出示例：

```json
{
  "ok": true,
  "data": {
    "project": "/…/rainy-station",
    "out_dir": "/…/rainy-station/exports/20261012-182530",
    "format": "mp3",
    "layout": "role",
    "manifest": "/…/manifest.json",
    "concat": "/…/full.mp3",
    "ffmpeg_used": true,
    "count": 42,
    "items": [
      { "line_id": "L3f2a9c01", "role": "阿岚", "scene": "雨夜", "chapter": "第一章", "index": 1, "take_id": "0002", "file": "阿岚/0001_阿岚_L3f2a9c01.mp3", "duration_ms": 1820, "chars": 12 }
    ]
  }
}
```

### `tts voice`

音色库：`list` / `show` / `rm`。

```bash
tts voice list --json                 # 预置 + 库内音色
tts voice list --kind clone --json    # 仅克隆音色
tts voice show 雨夜女主 --json
tts voice rm 雨夜女主 --yes           # 需 --yes（预置音色不可删除）
```

- `list` 输出每个音色的 `kind`（preset/design/clone）、`builtin`、`has_sample`、`sample_file`、`design_prompt`、`source_voice` 等。
- Schema：[`voice-list`](../schemas/cli/voice-list.schema.json) · [`voice-show`](../schemas/cli/voice-show.schema.json) · [`voice-rm`](../schemas/cli/voice-rm.schema.json)

### `tts design`

文本设计音色（voicedesign 模型）；`--name` 时把生成音频留档并入库（`kind=design`）。

```bash
tts design --prompt "二十岁出头的年轻女声，清亮偏薄" --text "试听文本" --name "雨夜女主" --json
tts design --prompt "…" --text "…" --optimize-text-preview --out ./draft.wav
```

- Schema：[`design`](../schemas/cli/design.schema.json)
- 注意：voicedesign 每次生成可能有差异；**建议随后固化**（见下）。

### `tts design solidify`

把设计音色固化为克隆音色（跨句一致性）：

```bash
tts design solidify --from 雨夜女主 --name 雨夜女主-固化 --json
tts design solidify --from ./draft.wav --name 手工固化        # 也可直接给音频文件
```

- 生成 `kind=clone` 的音色并记录 `source_voice`；此后用该音色即固定音色。
- Schema：[`solidify`](../schemas/cli/solidify.schema.json)

### `tts clone`

用音频样本创建克隆音色（wav/mp3、base64 后 ≤10MB）：

```bash
tts clone --sample ./voices/yuki.mp3 --name Yuki --json
```

- Schema：[`clone`](../schemas/cli/clone.schema.json)

---

## 后续计划（🚧）

> 以下命令尚未实现，接口可能微调；实现后将补全参数表与 Schema。

| 项 | 说明 |
| --- | --- |
| `tts project rm` | 删除项目（需 `--yes`） |
| `--play` | 合成后本机播放（当前请直接播放输出文件） |
| M3 GUI | Tauri + React + animal-island-ui（试听 / 对比 / 批量监控） |
| M4 候选 | ASR 回读校验、MCP Server、打包分发 |
