# 桌面 GUI（M3）

Tauri 2 + React 18.3 + TypeScript + [animal-island-ui](https://github.com/guokaigdg/animal-island-ui) 的本地桌面应用：
**人工预览与调优台**，与 CLI 共用同一内核（`tts-core`）与同一数据目录（`~/.tts-workbench`，可用 `TTS_WORKSPACE_ROOT` 覆盖）。

## 启动与打包

| 命令 | 说明 |
| --- | --- |
| `tools/gui` | **开发模式**：自动启动 Vite（热更新）+ 运行应用；改前端即时生效 |
| `tools/gui --run` | **独立运行**：内嵌前端（`custom-protocol`）构建后运行，不依赖 Vite |
| `tools/gui --build` | **打包**：构建前端 + `tauri build` → `TTS WorkSpace.app`（release、内嵌前端） |

打包产物：`target/release/bundle/macos/TTS WorkSpace.app`（可直接拖入 `/Applications`）。

- 应用**未签名**：本机双击可用；拷到其他 Mac 会被 Gatekeeper 拦截（右键「打开」放行，或自行签名/公证）。
- 图标：`crates/desktop/icons/icon.png`（1024 源）→ `icon.icns`（由 `iconutil` 生成，脚本见提交记录）。
- dev / 独立运行的区别来自 `crates/desktop/Cargo.toml` 的 `custom-protocol` feature：
  未启用 = 读 `devUrl`（需 Vite）；启用 = 内嵌 `ui/dist`。

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
- 图标：统一使用 **naive-icons**（animal-island-ui 的运行依赖，已在 `ui/package.json` 声明）；不用 emoji / Unicode 符号充当图标。
- 自定义元素样式：只用 `--animal-*` 设计令牌（颜色 / 圆角 / 间距 / 动效），不写死十六进制色值。
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
- **改了 Rust 命令层没生效**：重启应用；前端改动由 Vite 热更新（`tools/gui`）。
- **打包后双击没反应**：确认用的是 `target/release/bundle/macos/*.app`（`tools/gui --build` 产出），
  而不是 `target/debug/tts-desktop`（debug 构建默认走 devUrl，需要 Vite 才不白屏）。
