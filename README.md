# TTS-WorkSpace

基于 [MiMo-V2.5-TTS](https://mimo.mi.com/docs/zh-CN/quick-start/usage-guide/audio/speech-synthesis-v2.5) 的**通用语音合成工作台**：Agent 优先的 CLI + 桌面 GUI（预览与调优）。

- **定位**：批量与工程化的 TTS 生产工具（项目 / 音色库 / Take 管理 / 导出 / 用量），与具体游戏或应用解耦。
- **第一消费者是 LLM Agent**：CLI 提供稳定契约 —— 统一 JSON 信封、退出码、JSON Schema、说明书文档。
- **GUI**（M3）：Tauri 2 + React + [animal-island-ui](https://github.com/guokaigdg/animal-island-ui)，面向人工试听、对比与批量监控。

> 当前状态：**M0（骨架与契约）完成** · M1（CLI 端到端）开发中。

## 安装

需要 Rust（stable）与 ffmpeg（mp3 转码 / 拼接 / 响度处理需要，可后装）：

```bash
# Rust（rustup minimal）
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
rustup component add clippy rustfmt

# macOS 安装 ffmpeg
brew install ffmpeg
```

## 构建与上手

```bash
cargo build --workspace                 # 构建
cargo run -p tts-cli -- doctor          # 环境自检（工作区/配置/Key/ffmpeg/网络）
cargo install --path crates/cli         # 安装 `tts` 到 PATH（可选）
```

首次配置 API Key（也可使用环境变量 `MIMO_API_KEY`）：

```bash
tts config set api.api_key sk-xxxxxxxx
tts doctor --json
```

## CLI 速览

已实现：

```bash
# 环境与配置（M0）
tts doctor --json --skip-network
tts config set api.api_key sk-xxxx

# 项目与对白脚本（M1）
tts project init ./works/rainy-station --name "雨夜车站"
tts project import ./works/rainy-station/script/chapter1.txt --project ./works/rainy-station --json
tts project status --project ./works/rainy-station --json

# 单条合成（幂等缓存 + 审计；--line 联动项目 take）
tts synth --text "早上好！" --voice 冰糖 --json
tts synth --project ./works/rainy-station --line L3f2a9c01 --voice 冰糖 --style "(低沉)"
```

计划中（M1 后续，见 [docs/cli.md](docs/cli.md)）：
`role` · `line` · `batch` · `takes` · `cast` · `export` · `usage` · `gc`；M2：`voice` · `design` · `clone`。

## 仓库结构

```
crates/
├─ core/          # 引擎：配置 / Provider / MiMo 客户端 / 缓存 / 音频 / doctor（零 UI 依赖）
└─ cli/           # 二进制 `tts`（Agent 主接口）+ 契约测试
schemas/
├─ cli/           # 各命令 JSON 输出 Schema（契约，测试逐项校验）
└─ common/        # 错误信封 Schema
docs/             # 说明书：agent-guide / cli / script-format / data-layout
tests/fixtures/   # 共享测试夹具（回放数据，脱敏）
tools/            # test / lint 统一入口
ui/               # 桌面前端（M3）
```

## 数据目录

默认 `~/.tts-workbench/`（可用 `TTS_WORKSPACE_ROOT` 覆盖）：全局音色库、内容寻址缓存、审计日志、项目根。
规格见 [docs/data-layout.md](docs/data-layout.md)。

## 开发

```bash
tools/test     # 全部测试（cargo test + schema 校验；ui/ 存在时含前端检查）
tools/lint     # fmt + clippy(-D warnings) + 前端 lint
```

约定见 [AGENTS.md](AGENTS.md)。

## 路线图

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 骨架 + 契约（doctor / config / schema / 文档 / 工具链） | ✅ 完成 |
| M1 | CLI 端到端：合成 / 项目 / 对白解析 / 批量队列 / Take / 导出 / 用量 | 开发中 |
| M2 | 音色库：音色设计、克隆、设计→克隆固化、Cast 对比 | 计划 |
| M3 | GUI：Tauri 2 + React + animal-island-ui（试听 / 对比 / 批量监控） | 计划 |
| M4 | 候选：ASR 校验、MCP Server、打包分发、多 Provider | 候选 |

## 免责声明

- 本项目为独立开源工具，与小米 / MiMo 团队无关联；使用需遵守 MiMo 开放平台的服务条款。
- 模型 ID 与下线政策可能变化（V2.5 文本模型已有先例），模型 ID、base_url 均可通过配置切换。
- 请勿将 API Key 提交进仓库：配置文件权限 0600，运行产物不入库。

## License

[MIT](LICENSE)
