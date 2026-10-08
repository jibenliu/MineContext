// Windows 下不弹控制台窗口；其它平台无影响。
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    minecontext_shell::run()
}
