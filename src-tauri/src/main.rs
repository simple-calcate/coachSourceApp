// 防止 Windows 上多弹一个控制台窗口，不要删
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    coach_source_app_lib::run()
}
