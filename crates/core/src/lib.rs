//! TTS-WorkSpace 内核。
//!
//! 零 UI 依赖：命令行、桌面壳（Tauri）与测试共用同一实现。
//! 契约详见 `docs/cli.md`（JSON 信封 / 退出码）与 `docs/data-layout.md`（数据目录）。

pub mod audio;
pub mod audit;
pub mod batch;
pub mod cache;
pub mod config;
pub mod doctor;
pub mod error;
pub mod mimo;
pub mod paths;
pub mod project;
pub mod provider;
pub mod script;
pub mod synth;
pub mod util;
pub mod voices;

#[cfg(test)]
pub mod testing;

/// 内核版本（与 workspace 版本一致）。
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
