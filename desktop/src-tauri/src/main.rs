// Prevents an extra console window on Windows in release; noop elsewhere.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    bsdkrun_desktop_lib::run()
}
