# AGENTS.md — TTS-WorkSpace

面向在本仓库工作的 AI Agent 的约定。**先读 `docs/` 说明书，再改代码。**

## 快速命令

| 命令 | 用途 |
| --- | --- |
| `cargo build --workspace` | 构建 |
| `tools/test` | 全部测试（cargo test + CLI 契约/schema 校验；`ui/` 存在时含前端检查） |
| `tools/test --filter <子串>` | 仅运行匹配的测试 |
| `tools/test --min-tests <N>` | 用例数下限（防假绿；退出码 3 = 未达标） |
| `tools/lint` | `cargo fmt --check` + `clippy -D warnings`（+ 前端 lint） |
| `cargo run -p tts-cli -- <子命令>` | 本地运行 CLI |

`tools/test` 退出码：0 通过 · 1 测试失败 · 2 工具链缺失等内部错误 · 3 未发现测试/低于下限。

## 架构

- `crates/core/`：引擎（config / paths / provider / mimo / cache / audio / doctor / error）。**零 UI 依赖，headless 可测**；业务逻辑一律放这里。
- `crates/cli/`：`tts` 二进制。只做参数解析、调用 core、按契约输出；不放业务逻辑。
- `schemas/`：JSON Schema 即对外契约；CLI 输出结构变更必须同步 schema、`docs/cli.md` 与测试。
- `crates/cli/tests/`：CLI 契约测试（stdin/stdout/退出码/schema）。仓库根 `tests/fixtures/`：共享夹具（回放数据，必须脱敏）。
- `ui/`（M3）：React 18.3 + TypeScript + Vite + **animal-island-ui**。
  - 必须 `import 'animal-island-ui/style'`；不要引入 Tailwind（与岛屿设计语言冲突）。
  - 页面生成前安装风格 skill：`skills add guokaigdg/animal-island-ui`，并对照其 `docs/design-system/`。

## 硬性规则

1. **契约优先**：新增/修改命令必须同时更新四处：实现、测试、`docs/cli.md`、`schemas/`。
2. **测试**：功能必须带测试。Rust 单测写在模块内 `#[cfg(test)]`；CLI 行为写 `crates/cli/tests/`。不得删除或改写既有测试来“变绿”（`--min-tests` 兜底）。
3. **网络与隐私**：测试禁止访问真实 API；使用 `tests/fixtures/` 回放数据或本地 mock。仓库内禁止出现 API Key、真实人声样本与用户数据。
4. **错误码**：统一使用 `crates/core/src/error.rs` 常量；新增错误码必须同步 `docs/cli.md` 错误码表。
5. **退出码契约**：0 成功 / 1 运行时失败 / 2 用法与校验错误 / 3 部分失败 / 4 需要显式确认（`--yes`）。
6. **数据目录**：一切路径经 `Paths` 解析；测试用 `TTS_WORKSPACE_ROOT` 指向临时目录，禁止写入用户真实数据目录。
7. **依赖纪律**：core 不得引入 UI/平台依赖（Tauri、GUI 工具包等）；新增依赖需在 PR/提交说明中给出理由。
8. **模型契约**：模型 ID 与 base_url 一律读配置（`Config`），不得硬编码到业务逻辑；流式能力以 `Capabilities` 声明为准。

## 提交前检查

1. `cargo fmt --all`
2. `tools/lint && tools/test`
3. 若改动涉及 CLI 输出 / 配置字段 / 错误码：确认 `schemas/` 与 `docs/` 已同步
4. 提交信息使用中文，一句话说明“做了什么 + 为什么”
