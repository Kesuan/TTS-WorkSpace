# 数据目录与文件规格

> 默认工作区根：`~/.tts-workbench`，可用 `TTS_WORKSPACE_ROOT` 覆盖；配置文件路径可用 `TTS_CONFIG` 覆盖。
> 状态标注：✅ 已实现（M0）· 🚧 计划中（M1 / M2）。

## 全局目录

```
~/.tts-workbench/                     # 工作区根（TTS_WORKSPACE_ROOT）
├─ config.json                        # ✅ 配置（Unix 权限 0600）
├─ library/                           # 🚧 全局音色库
│  └─ voices/<voice_id>/              #    voice.json + sample.wav|mp3
├─ cache/                             # ✅ 内容寻址缓存
│  └─ <key16>/                        #    blob.wav + meta.json
├─ synth/                             # ✅ 独立模式合成输出（tts synth）
│  └─ <cache_key>.wav
├─ audit/                             # ✅ 审计日志
│  └─ audit.jsonl                     #    append-only JSONL
└─ projects/                          # 项目根（项目也可放在任意路径）
```

## config.json ✅

```json
{
  "schema_version": 1,
  "api": {
    "base_url": "https://api.xiaomimimo.com/v1",
    "api_key": "sk-…",
    "timeout_ms": 120000
  },
  "models": {
    "tts": "mimo-v2.5-tts",
    "design": "mimo-v2.5-tts-voicedesign",
    "clone": "mimo-v2.5-tts-voiceclone"
  },
  "generation": {
    "concurrency": 3,
    "max_chars_per_request": 500,
    "retry": { "max": 3, "backoff_ms": 800 }
  },
  "ffmpeg": { "path": "auto" }
}
```

- 字段范围与键名见 [docs/cli.md](cli.md#tts-config)；`workspace_root` 为可选字段（用于把数据目录重定向到别处）。
- 未知字段被保留（前向兼容）；缺省字段回落到默认值。
- API Key 优先级：环境变量 `MIMO_API_KEY` > `api.api_key`。

## 缓存 ✅

- 目录：`cache/<key16>/`，其中 `<key16>` = `sha256(模型␟音色␟文本␟风格␟格式␟附加项)[0:16]`（`␟` 为 `0x1f` 分隔符）。
- `blob.wav`：音频内容；`meta.json`：请求指纹与生成参数。
- 命中即复用（相同参数不重复请求、不重复等待）；清理用 `tts gc`（🚧 M1，删除未被引用的目录）。

## 音色库 🚧（M2）

`library/voices/<voice_id>/voice.json`：

```json
{
  "schema_version": 1,
  "id": "yuki-d3f2a1",
  "name": "Yuki",
  "type": "preset | design | clone",
  "model": "mimo-v2.5-tts-voiceclone",
  "preset_name": "冰糖",
  "design_prompt": "温柔但有距离感的年轻女声…",
  "source_take_id": "…",
  "sample_file": "sample.mp3",
  "sample_sha256": "…",
  "tags": ["主角"],
  "notes": "M2 固化自 design take",
  "created_at": "2026-10-12T09:00:00Z"
}
```

- `type=design`：`design_prompt` 是音色来源；**建议先 `design solidify` 固化为克隆样本再批量使用**（设计模型每次生成可能不一致）。
- `type=clone`：`sample_file` 为 ≤10MB 的 wav/mp3；请求时编码为 `data:audio/...;base64,…`。

## 审计日志 ✅

`audit/audit.jsonl`，每行一条，append-only（`tts synth` 成功与失败都会写入；后续 batch / export 同样）：

```json
{"ts":"2026-10-12T09:00:00Z","cmd":"synth","project":"/…/rainy-station","line_id":"L3f2a9c01","model":"mimo-v2.5-tts","voice":"冰糖","chars":37,"duration_ms":1820,"latency_ms":880,"cached":false,"status":"ok","cost_note":"tts:free"}
```

`tts usage`（🚧）由该文件聚合（请求数 / 字符数 / 缓存命中率 / 音频时长），为将来计费做准备。

## 项目（= 作品）🚧（M1）

```
<项目目录>/
├─ project.json                 # 名称 / 脚本 / 角色↔音色绑定 / 默认参数 / 导出配置
├─ script/*.txt                 # 原始对白脚本（格式见 docs/script-format.md）
├─ lines.json                   # 解析快照：行 ID / 角色 / 文本 / 标签 / 状态 / takes
├─ takes/<line_id>/             # 0001_<hash8>.wav + take.json
└─ exports/<时间戳>/            # 导出产物 + manifest.json
```

### lines.json（逐行条目）

```json
{
  "schema_version": 1,
  "lines": [
    {
      "id": "L3f2a9c01",
      "chapter": "第一章",
      "scene": "雨夜",
      "index": 12,
      "role": "阿岚",
      "text": "(低沉)[叹气] 又是这样的雨……",
      "status": "pending | ok | failed | removed",
      "selected_take": "0002",
      "overrides": { "voice": null, "style": null },
      "takes": [
        {
          "take_id": "0001",
          "file": "takes/L3f2a9c01/0001_d3f2a1b8.wav",
          "params_hash": "d3f2a1b8",
          "cache_key": "9c1e0b7f3a2d4e56",
          "chars": 18,
          "duration_ms": 2340,
          "created_at": "2026-10-12T09:00:00Z"
        }
      ]
    }
  ]
}
```

说明：

- `id` 为**稳定行 ID**（内容派生，见 [docs/script-format.md](script-format.md)）；脚本改动产生的 `removed` 行保留其 takes 不删除，导出默认排除。
- `overrides` 用于行级覆盖角色默认音色 / 风格（由 `tts line set` 写入）。
- 导出清单（`exports/<ts>/manifest.json`）：逐行记录 `line_id / role / file / duration_ms / take_id / text`，供下游工程消费。

## 约定

- 所有 JSON 文件带 `schema_version`；升级时递增并给出迁移说明。
- 时间戳一律 UTC（RFC 3339）。
- 目录/文件名仅使用 ASCII 与安全字符；角色名等中文信息存在 JSON 内，不参与路径。
