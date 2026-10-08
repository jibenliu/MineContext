//! `capture.sources` 决定跑哪些采集源 —— 写错了要说，不能猜。
//!
//! 配置里的 `sources` 必须被真正遵守，不能永远只跑屏幕源。
//! 现在的规则（**fail-closed**）：配置里点名的源要么真的跑起来，
//! 要么明确报告「这个源还不存在」；**绝不**因为用户只勾了未实现的源
//! 就悄悄退回「截全部屏幕」—— 那是替用户扩大采集范围。

#![cfg(target_os = "macos")]

use mc_capture::platform::sources_for;
use mc_capture::source::SourceKind;

#[test]
fn default_config_enables_screen_and_window() {
    let selection = sources_for(&["screen".to_string(), "window".to_string()])
        .expect("默认配置必须能构造出采集源");

    let ids = selection.source_ids();
    assert!(
        ids.iter().any(|id| id.contains("screen")),
        "缺少屏幕源：{ids:?}"
    );
    assert!(
        ids.iter().any(|id| id.contains("window")),
        "缺少窗口源：{ids:?}"
    );
    assert!(selection.ignored.is_empty(), "默认配置不该有被忽略的源");
}

#[test]
fn screen_only_config_does_not_enable_window_capture() {
    let selection = sources_for(&["screen".to_string()]).expect("只配屏幕也要能构造");
    assert_eq!(selection.source_ids().len(), 1);
    assert_eq!(selection.source.kind(), SourceKind::Screen);
}

#[test]
fn unimplemented_source_is_reported_not_silently_swapped() {
    // 用户只勾了剪贴板（尚未实现）：
    // 正确的行为是**报告**，而不是偷偷跑屏幕采集 —— 那等于扩大采集范围。
    let error = sources_for(&["clipboard".to_string()]).expect_err("不该静默退回屏幕采集");
    let detail = error.detail().to_string();
    assert!(
        detail.contains("clipboard"),
        "错误里要点名是哪个源，实际：{detail}"
    );
}

#[test]
fn unknown_source_id_is_listed_as_ignored() {
    let selection = sources_for(&["screen".to_string(), "typo-source".to_string()])
        .expect("有可用源时不应失败");

    assert_eq!(selection.ignored, vec!["typo-source".to_string()]);
    assert_eq!(selection.source_ids().len(), 1, "拼错的源不能变成第二个源");
}
