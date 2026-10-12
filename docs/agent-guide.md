# Agent 使用指南

> 本工作台的第一消费者是 LLM Agent。本文档教 Agent 如何可靠地驱动 `tts`。
> 状态标注：✅ 已实现（M0）· 🚧 计划中（M1 / M2）。

## 0. 会话开场（每次都做）

```bash
tts version --json          # 确认版本与工作区
tts doctor --json           # 环境自检；离线环境用 --skip-network
```

判定规则（契约，见 [docs/cli.md](cli.md)）：

- 退出码 `0` 且 `data.summary.fail == 0` → 可以继续；`warn` 项按提示处理（例如缺 Key）。
- 退出码 `1` 且 `error.code == "E_DOCTOR_UNHEALTHY"` → 读 `error.details.checks`，修复所有 `fail` 项后重试。
- 退出码 `2` → 命令用法写错了，读 `error.message` 修正参数，**不要**重试同样的命令。

## 1. 输出约定（必须遵守）

- **永远加 `--json`**：stdout 恒为单个 JSON 对象（信封），便于解析。
- 成功：`{"ok":true,"data":{…}}`；失败：`{"ok":false,"error":{"code","message","hint","details"}}`。
- 用 `error.code` 分支处理（码表见 docs/cli.md），不要匹配 `message` 文本。
- 退出码：`0` 成功 · `1` 运行时失败（可重试：网络/限流）· `2` 用法错误（不可重试，需改参数）· `3` 部分失败 · `4` 需要 `--yes`。

典型重试策略（🚧 M1 起）：

| 错误码 | 建议 |
| --- | --- |
| `E_HTTP_NETWORK` | 指数退避重试（1s/2s/4s，≤3 次） |
| `E_HTTP_RATE_LIMIT` | 等待 30s 后重试；降低 `--concurrency` |
| `E_HTTP_STATUS`（5xx） | 可重试；批量模式已内置退避重试 |
| `E_PARTIAL`（退出码 3） | 部分失败：读 `details.failed_lines`，修复后重跑（幂等） |
| `E_CONFIG_KEY` | 先配置 Key 或导出 `MIMO_API_KEY`，不要重试 |
| `E_LINE_UNBOUND` | 先 `tts role bind` / `tts line set` 绑定音色，再跑 batch |
| `E_USAGE` | 修改参数，不要重试 |
| `E_CAPABILITY` | 换模型或去掉该参数（如对流式限制的模型去掉 `--stream`） |

## 2. 配置（✅ M0）

```bash
tts config set api.api_key sk-xxx        # 写配置（0600）
tts config get api.base_url --json       # 读单项；api.api_key 默认打码
tts config list --json                   # 全量（含 api_key_source: env|config|missing）
```

- Key 也可由环境变量 `MIMO_API_KEY` 提供（优先级更高，适合 CI）。
- 测试/隔离：把 `TTS_WORKSPACE_ROOT` 指向临时目录即可得到完全独立的工作区。

## 3. 工作流配方

### 3.0 单条合成与试听（✅ 已实现）

```bash
# 独立合成（写入 <工作区>/synth/<cache_key>.wav）
tts synth --text "早上好！" --voice 冰糖 --style "轻快一点" --json

# 指定输出文件 / 设计音色（design: 在 M2 配合 voicedesign 模型）
tts synth --text "试听文本" --voice design:"温柔的年轻女声" --out ./demo.wav

# 项目行合成：take 落盘 + 行状态更新 + 选中项自动设置
tts synth --project ./works/rainy-station --line L3f2a9c01 --voice 冰糖 --style "(低沉)" --json
```

要点：相同参数重跑命中缓存（`cached: true`，不重复请求、不再等待）；失败同样写审计（`error_code`）；`--stream` 仅预置音色模型可用。

### 3.1 从剧本到批量配音（主流程；✅ 全部已实现）

```bash
# 1) 初始化 + 导入（✅ 已实现）
tts project init ./works/rainy-station --name "雨夜车站"
tts project import ./works/rainy-station/script/chapter1.txt --project ./works/rainy-station --json
#    → 解析报告含 unbound_roles：批量前必须全部绑定

# 2) 绑定角色音色（✅）
tts role bind --role 阿岚 --voice 冰糖 --style "年轻的清亮女声，语速偏快" --project ./works/rainy-station
tts line set --line L3f2a9c01 --style "(低语) 我没事，真的。" --project ./works/rainy-station   # 行级覆盖（可选）

# 3) 批量生成（✅ 幂等：重跑自动跳过已完成；失败行自动重试）
tts batch --project ./works/rainy-station --json --events jsonl     # 事件流 + 最终信封（最后一行）

# 4) 失败处理（✅）
tts batch status --project ./works/rainy-station --json             # 查看失败行
tts batch retry  --project ./works/rainy-station                    # 重置失败行后重跑 batch

# 5) Take 与用量（✅）
tts takes list --project ./works/rainy-station --line L3f2a9c01 --json   # 查看 selected / exists
tts takes select --project ./works/rainy-station --line L3f2a9c01 --take 0002
tts cast --project ./works/rainy-station --line L3f2a9c01 --voices 冰糖,茉莉,苏打 --json  # 选角对比
tts usage --since 2026-10 --json
tts gc --dry-run --json

# 6) 导出（✅）
tts export --project ./works/rainy-station --format mp3 --layout role --concat --json
#    → <项目>/exports/<时间戳>/ 下逐行音频 + manifest.json（+ full.mp3）
```

要点：

- 批量 **fail-fast**：存在未绑定音色的行 → `E_LINE_UNBOUND`（退出码 2），不会产生任何请求；先绑定再跑。
- 部分失败 → 退出码 **3**（`E_PARTIAL`），`error.details.failed_lines` 给出每行错误码；修复后 `batch retry` + 重跑即可（或直接重跑 batch，失败行会自动重试、ok 行自动跳过）。
- `--events jsonl` 需同时 `--json`：事件逐行输出，**最后一行是统一信封**。

### 3.2 音色设计 → 固化（保证跨句一致性，✅）

```bash
tts design --prompt "二十岁出头的年轻男声，音色清亮，略带鼻音，语速中等" --text "试听文本" --name "小满草稿" --json
# 满意后固化（避免设计模型每次生成不一致）
tts design solidify --from "小满草稿" --name "小满" --json
tts role bind --role 小满 --voice "小满" --project ./works/rainy-station
```

### 3.3 克隆既有声音（✅）

```bash
tts clone --sample ./voices/yuki.mp3 --name "Yuki" --json    # 校验：≤10MB，wav/mp3
tts voice list --kind clone --json
```

### 3.4 选角对比（Cast，✅）

```bash
tts cast --project ./works/rainy-station --line L3f2a9c01 --voices 冰糖,茉莉,苏打 --json
```

### 3.5 音色模型约定

- 名称解析：预置音色 → 音色库 → 报错（拼错请用 `tts voice list` 核对）。
- 模型自动选择：预置 → `models.tts`；库内/内联 clone → `models.voiceclone`；design → `models.design`。显式 `--model` 优先。
- 批量按**每行音色**分别选模型（可在同一批混用预置 + 克隆角色）。

## 4. 护栏（Agent 注意事项）

- **危险操作**（删除音色/样本、覆盖导出、`project rm`）需要 `--yes`，否则返回 `E_CONFIRM_REQUIRED`（退出码 4）。先向用户确认，再带 `--yes` 执行（🚧 M1）。
- **审计**：所有生成 / 导出 / 删除写入 `audit/audit.jsonl`（append-only）。用 `tts usage` 查用量（🚧 M1）。
- **成本**：TTS 当前限时免费；ASR（后续版本）按音频时长计费（¥0.5/小时）。TTS 若开始计费，请先 `usage` 预估再批量。
- **隐私**：克隆样本可能包含个人声音，属于敏感数据；不要上传到仓库或公开位置。

## 5. 常见问题

**Q：`doctor` 里 `api_key` 是 warn，能跑合成吗？**
不能。合成/克隆等命令会返回 `E_CONFIG_KEY`（退出码 1）。先配置 Key。

**Q：`--stream` 报 `E_CAPABILITY`？**
流式仅 `mimo-v2.5-tts`（预置音色）支持；设计 / 克隆模型请去掉 `--stream`。

**Q：批量跑到一半中断了怎么办？**
直接重跑同一条 `batch` 命令；已完成条目会被跳过（幂等），失败项重试。用 `tts batch status` 查看进度（🚧 M1）。

**Q：模型名变了怎么办？**
模型 ID 全部走配置：`tts config set models.tts <新 ID>`；先 `tts config get models.tts --json` 看当前值。
