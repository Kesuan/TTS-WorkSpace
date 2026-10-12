# 桌面 GUI（M3）

Tauri 2 + React 18.3 + TypeScript + [animal-island-ui](https://github.com/guokaigdg/animal-island-ui) 的本地桌面应用：
**人工预览与调优台**，与 CLI 共用同一内核（`tts-core`）与同一数据目录（`~/.tts-workbench`，可用 `TTS_WORKSPACE_ROOT` 覆盖）。

## 启动

```bash
tools/gui              # 安装依赖 + 构建前端 + 启动应用（dev）
tools/gui --build      # 仅构建前端并编译（产物：target/debug/tts-desktop）
cargo run -p tts-desktop   # 已构建过前端时的快捷启动
```

前置：Rust 工具链、Node.js 18+、ffmpeg 9（导出 mp3 / 拼接 / 归一化时需要）。

## 页面

| 页面 | 能力 |
| --- | --- |
| **项目** | 项目列表 / 新建；导入对白脚本（输出 diff 与未绑定角色）；角色 ↔ 音色绑定；逐行“重新生成”；行级覆盖（音色/风格）；take 试听与选用；批量生成（并发、强制、实时事件进度与日志）；导出面板（wav/mp3、flat/role/scene、拼接、归一化、静音裁剪，一键在 Finder 显示） |
| **音色库** | 音色设计（voicedesign，含智能润色、可直接入库）；克隆（本地样本）；设计 → 固化；列表与删除（预置音色不可删） |
| **用量** | 请求/成功率/缓存命中/字符数/音频时长统计（来自审计日志）；缓存清理（dry-run 预览 + 执行） |
| **设置** | API Key（写入 0600 配置文件，支持清除）；Base URL；三个模型 ID；并发/重试/退避；ffmpeg 路径；`doctor` 环境自检（Key/ffmpeg/网络） |

## 架构

```
ui/                       # 前端（Vite + React 18.3 + animal-island-ui）
└─ src/
   ├─ api.ts              # Tauri 命令封装 + 统一错误模型 + convertFileSrc
   ├─ types.ts            # 与 Rust 侧对齐的类型
   └─ pages/{Projects,Voices,Usage,Settings}Page.tsx
crates/desktop/           # Tauri 2 壳
├─ src/lib.rs             # 命令层（全部逻辑调用 tts-core）
└─ tauri.conf.json        # devUrl 5173；assetProtocol 允许读取本地音频
```

- **错误模型**：IPC 失败返回 `{code, message, hint?, details?}`（与 CLI 的 `error` 字段同构）；前端 `TtsCommandError` 统一展示；`E_PARTIAL` 表示批量部分失败（仍已产出结果）。
- **音频播放**：take 的绝对路径经 `convertFileSrc` 转换为 `asset://` URL 交给 `<audio>`。
- **批量事件**：`batch_run` 通过 Tauri `Channel<BatchEvent>` 推送（与 CLI `--events jsonl` 同构）。

## 开发约定（见 AGENTS.md）

- 组件与样式：只用 animal-island-ui 的组件与设计令牌；**不引入 Tailwind**；必须 `import 'animal-island-ui/style'`。
- 新增 IPC 命令：在 `crates/desktop/src/lib.rs` 实现（逻辑放 core），并在 `ui/src/api.ts` / `types.ts` 补封装与类型。
- 类型检查：`cd ui && npm run typecheck`（`tools/lint` 会自动执行；`ui/` 存在时 `tools/test` 也会执行）。

## 已知限制（v0.1）

- 无文件选择对话框：路径（脚本/样本/导出目录）需手动输入或粘贴。
- 不打包安装包（源码运行）；打包见后续里程碑。
- 无 ASR 回读校验、无 MCP（后续里程碑）。

## 排障

- **音频播放器显示 Error**：`assetProtocol.scope` 需 `requireLiteralLeadingDot: false`。
  默认工作区是 `~/.tts-workbench`（点目录），而 Tauri 在 Unix 下默认要求点组件必须由
  模式中的字面 `.` 匹配，`**` 无法匹配点目录 → 改为
  `{"allow": ["**"], "requireLiteralLeadingDot": false}`（见 `crates/desktop/tauri.conf.json`）。
- **窗口白屏**：dev 模式需要 Vite（`tools/gui` 会先构建再启动）。只想快速体验可
  `tools/gui --build` 后运行 `target/debug/tts-desktop`。
- **改了 Rust 命令层没生效**：重启应用（`Ctrl+C` 后重跑 `tools/gui`）；前端改动由 Vite 热更新。
