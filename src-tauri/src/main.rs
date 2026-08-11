// 发布版本隐藏 Windows 上的控制台窗口（macOS 无影响）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    devbox_lib::run()
}
