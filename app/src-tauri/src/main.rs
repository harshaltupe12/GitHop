// No console window in release builds (the app lives in the tray).
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    std::process::exit(githop_app::run());
}
