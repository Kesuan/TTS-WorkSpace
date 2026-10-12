//! TTS-WorkSpace 桌面应用入口。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tts_desktop_lib::run()
}
