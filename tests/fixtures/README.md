# tests/fixtures — 共享测试夹具

本目录存放跨测试共享的**回放数据**（M1 起使用）：

- `responses/`：从真实 MiMo API 录制后**脱敏**的响应体（`chat.completions` 非流式 / 流式帧 / `models` 列表），用于离线回放与 golden 比对。
- `scripts/`：对白脚本样例（含中英混排、行内标签、旁白、异常样例），供解析器测试引用。

## 规则（见仓库根 AGENTS.md）

1. **必须脱敏**：不得包含 API Key、请求头、账号信息、真实人声样本。
2. **禁止真实网络**：测试一律读夹具或本地 mock，不访问 `api.xiaomimimo.com`。
3. **体积控制**：音频夹具用短音频（≤1s 的合成 PCM/WAV）；大文件不入库。
4. **与文档同步**：新增夹具时在本文件说明用途，并注明对应的 schema 或命令。

Rust 集成测试位于 `crates/cli/tests/`；本目录通过 `CARGO_MANIFEST_DIR` 相对路径引用。
