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
| `E_CONFIG_KEY` | 先配置 Key 或导出 `MIMO_API_KEY`，不要重试 |
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

## 3. 工作流配方（🚧 M1 / M2）

> 下列命令尚未实现，将在 M1（合成/项目/批量/导出）与 M2（音色库）交付；接口见 docs/cli.md 计划表。

### 3.1 从剧本到批量配音（主流程）

```bash
tts project init ./works/rainy-station
tts project import --project ./works/rainy-station script/chapter1.txt --json   # 解析报告：角色/告警/unbound_roles
tts role bind --project ./works/rainy-station --role 阿岚 --voice 冰糖 --style "年轻的清亮女声，语速偏快"
tts batch --project ./works/rainy-station --all --events jsonl                  # 幂等：重跑自动跳过已完成
tts takes list --project ./works/rainy-station --line L3f2a9c01 --json
tts takes select --project ./works/rainy-station --line L3f2a9c01 --take 0002
tts export --project ./works/rainy-station --format mp3 --layout role --concat --manifest --json
```

要点：

- `batch` 幂等：失败重跑只处理未完成项；相同参数命中缓存不重复请求。
- `--events jsonl` 逐行输出事件（`started/progress/succeeded/cached/failed/finished`），适合长任务流式消费。
- 批量存在失败项时退出码为 `3`（部分失败）——**这是可恢复状态**，修正后重跑即可。

### 3.2 音色设计 → 固化（保证跨句一致性）

```bash
tts design --prompt "二十岁出头的年轻男声，音色清亮，略带鼻音，语速中等" --text "试听文本" --json
# 挑选满意的 take 固化为克隆音色（避免设计模型每次生成不一致）
tts design solidify --take <take_id> --name "小满" --json
tts role bind --role 小满 --voice <voice_id>
```

### 3.3 克隆既有声音

```bash
tts clone --sample ./voices/yuki.mp3 --name "Yuki" --json    # 校验：≤10MB，wav/mp3
```

### 3.4 选角对比（Cast）

```bash
tts cast --project ./works/rainy-station --line L3f2a9c01 --voices 冰糖,茉莉,苏打 --json
```

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
