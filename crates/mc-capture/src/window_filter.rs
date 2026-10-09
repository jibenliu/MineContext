//! 窗口是否值得出现在采集目标列表里。
//!
//! 与平台枚举解耦：纯规则，CI（含非 macOS）也能测。

/// 这个窗口值不值得采 / 不该出现在设置页勾选列表。
///
/// 系统里有大量没有标题、没有应用名的辅助窗口（菜单栏、浮层、零尺寸窗口），
/// 采集它们只会把存储和后续分析浪费掉。菜单栏还会在设置页冒充可选项。
pub fn is_capturable(app_name: Option<&str>, title: Option<&str>, width: u32, height: u32) -> bool {
    if width == 0 || height == 0 {
        return false;
    }
    if is_system_ui(app_name, title) {
        return false;
    }
    app_name.is_some() || title.is_some()
}

/// macOS 系统 UI：设置页不应列出，也不能当作普通应用去勾选。
fn is_system_ui(app_name: Option<&str>, title: Option<&str>) -> bool {
    let normalize = |s: &str| s.trim().to_ascii_lowercase();
    if let Some(app) = app_name.map(normalize) {
        if matches!(
            app.as_str(),
            "menubar"
                | "menu bar"
                | "systemuiserver"
                | "control center"
                | "controlcentre"
                | "notification center"
                | "notificationcentre"
                | "window server"
                | "dock"
                | "wallpaper"
                | "spotlight"
                | "loginwindow"
                | "textinputmenuagent"
                | "control strip"
                | "statusbarserver"
        ) {
            return true;
        }
    }
    if let Some(title) = title.map(normalize) {
        if matches!(title.as_str(), "menubar" | "menu bar" | "item-0") {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_sized_and_anonymous_windows_are_skipped() {
        assert!(!is_capturable(None, None, 800, 600), "没有身份信息的窗口");
        assert!(!is_capturable(Some("App"), Some("标题"), 0, 600), "零宽");
        assert!(!is_capturable(Some("App"), Some("标题"), 800, 0), "零高");
        assert!(is_capturable(Some("App"), None, 800, 600));
        assert!(is_capturable(None, Some("标题"), 800, 600));
    }

    #[test]
    fn system_menu_bar_windows_are_not_capturable() {
        assert!(!is_capturable(Some("Menu Bar"), Some("Item-0"), 1440, 25));
        assert!(!is_capturable(
            Some("Control Center"),
            Some("Menu Bar"),
            400,
            400
        ));
        assert!(!is_capturable(Some("SystemUIServer"), None, 100, 100));
        assert!(
            is_capturable(Some("Docker"), Some("Dashboard"), 800, 600),
            "Docker 不是 Dock，不能被系统 UI 规则误伤"
        );
    }
}
