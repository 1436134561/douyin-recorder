// 发布版本隐藏控制台窗口（仅 Windows）
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    douyin_recorder_lib::run();
}
