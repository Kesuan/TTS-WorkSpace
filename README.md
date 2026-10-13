# TTS-WorkSpace

基于 [MiMo-V2.5-TTS](https://mimo.mi.com/docs/zh-CN/quick-start/usage-guide/audio/speech-synthesis-v2.5) 的**通用语音合成工作台**：Agent 优先的 CLI + 桌面 GUI（预览与调优）。

- **定位**：批量与工程化的 TTS 生产工具（项目 / 音色库 / Take 管理 / 导出 / 用量），与具体游戏或应用解耦。
- **第一消费者是 LLM Agent**：CLI 提供稳定契约 —— 统一 JSON 信封、退出码、JSON Schema、说明书文档。
- **GUI**：Tauri 2 + React + [animal-island-ui](https://github.com/guokaigdg/animal-island-ui)，面向人工试听、对比与批量监控；与 CLI 共用内核与数据目录。

> 当前状态：**M0 / M1 / M2 / M3 完成** —— CLI 全流程 + 桌面 GUI 均可用。

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
cargo build --workspace                 # 构建（debug）
cargo run -p tts-cli -- doctor          # 环境自检（工作区/配置/Key/ffmpeg/网络）

# 安装 CLI（release 二进制 → ~/.cargo/bin/tts）
cargo install --path crates/cli
# 或只构建：cargo build --release -p tts-cli   → target/release/tts
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

# 角色绑定与批量生成（M1）
tts role bind --role 阿岚 --voice 冰糖 --style "清亮女声" --project ./works/rainy-station
tts batch --project ./works/rainy-station --json --events jsonl    # 幂等；失败自动重试
tts batch status --project ./works/rainy-station --json
tts batch retry  --project ./works/rainy-station                   # 重置失败行

# Take / 选角 / 用量 / 清理（M1）
tts takes list --project ./works/rainy-station --json
tts takes select --project ./works/rainy-station --line L3f2a9c01 --take 0002
tts cast --project ./works/rainy-station --line L3f2a9c01 --voices 冰糖,茉莉
tts usage --since 2026-10 --json
tts gc --dry-run --json

# 导出（M1）
tts export --project ./works/rainy-station --format mp3 --layout role --concat --json

# 音色库：设计 → 固化 → 克隆（M2）
tts design --prompt "年轻清亮女声" --text "试听文本" --name "雨夜女主"
tts design solidify --from "雨夜女主" --name "雨夜女主-固化"
tts clone --sample ./voices/yuki.mp3 --name Yuki
tts voice list --json

# 单条合成（幂等缓存 + 审计；--line 联动项目 take；模型按音色类型自动选择）
tts synth --text "早上好！" --voice 冰糖 --json
tts synth --project ./works/rainy-station --line L3f2a9c01 --voice "雨夜女主-固化"
```

M1 + M2 已全部可用；下一步 M3：Tauri + React + animal-island-ui 桌面 GUI。

## 桌面 GUI

```bash
tools/gui            # 开发模式：Vite 热更新 + 运行应用
tools/gui --run      # 独立运行：内嵌前端（不需要 Vite）
tools/gui --build    # 打包：TTS WorkSpace.app（release、内嵌前端）
```

打包产物：`target/release/bundle/macos/TTS WorkSpace.app`（拖入 `/Applications` 即用；**未签名**，
拷到其他 Mac 需右键打开或自行签名/公证）。

四个页面：**项目**（导入剧本 / 角色绑定 / 逐行重生成 / take 试听与选用 / 批量进度 / 导出）、**音色库**（设计 / 克隆 / 固化 / 删除）、**用量**（统计 + 缓存清理）、**设置**（Key / 模型 / 并发重试 / ffmpeg / doctor）。
详见 [docs/gui.md](docs/gui.md)。

## 仓库结构

```
crates/
├─ core/          # 引擎：配置 / Provider / MiMo 客户端 / 缓存 / 音频 / 项目 / 批量 / 导出 / 音色库
├─ cli/           # 二进制 `tts`（Agent 主接口）+ 契约测试
└─ desktop/       # Tauri 2 桌面壳（命令层复用 core）
schemas/
├─ cli/           # 各命令 JSON 输出 Schema（契约，测试逐项校验）
└─ common/        # 错误信封 Schema
docs/             # 说明书：agent-guide / cli / script-format / data-layout / gui
tests/fixtures/   # 共享测试夹具（回放数据，脱敏）
tools/            # test / lint / gui 统一入口
ui/               # 桌面前端（Vite + React 18.3 + animal-island-ui）
```

## 数据目录

默认 `~/.tts-workbench/`（可用 `TTS_WORKSPACE_ROOT` 覆盖）：全局音色库、内容寻址缓存、审计日志、项目根。
规格见 [docs/data-layout.md](docs/data-layout.md)。

## 开发

```bash
tools/test     # 全部测试（cargo test + schema 校验 + 前端 typecheck/lint）
tools/lint     # fmt + clippy(-D warnings) + 前端 lint
tools/gui      # 启动桌面应用（dev）
```

约定见 [AGENTS.md](AGENTS.md)。

## 路线图

| 阶段 | 内容 | 状态 |
| --- | --- | --- |
| M0 | 骨架 + 契约（doctor / config / schema / 文档 / 工具链） | ✅ 完成 |
| M1 | CLI 端到端：合成 / 项目 / 对白解析 / 批量队列 / Take / 导出 / 用量 | ✅ 完成 |
| M2 | 音色库：音色设计、克隆、设计→克隆固化、库内音色全流程可用 | ✅ 完成 |
| M3 | GUI：Tauri 2 + React + animal-island-ui（试听 / 对比 / 批量监控） | ✅ 完成 |
| M4 | 候选：ASR 校验、MCP Server、Windows/Linux 打包、签名与公证、多 Provider | 候选 |

## 免责声明

- 本项目为独立开源工具，与小米 / MiMo 团队无关联；使用需遵守 MiMo 开放平台的服务条款。
- 模型 ID 与下线政策可能变化（V2.5 文本模型已有先例），模型 ID、base_url 均可通过配置切换。
- 请勿将 API Key 提交进仓库：配置文件权限 0600，运行产物不入库。

## License

[MIT](LICENSE)
