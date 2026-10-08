//! 平台支持范围：macOS 最低 13（Ventura），并覆盖 14（Sonoma）。
//!
//! 支持范围必须是**可执行的判定**，而不是只写在文档里：
//! 用户装到 macOS 12 上时，应当从 `/api/diagnostics` 或 `mc-cli doctor`
//! 看到「系统版本不受支持」，而不是一个链接期就已经错配的二进制悄悄出问题。

use mc_common::platform::{self, MacOsVersion, MIN_SUPPORTED_MACOS};

#[test]
fn minimum_supported_version_is_macos_13() {
    assert_eq!(MIN_SUPPORTED_MACOS.major, 13);
    assert_eq!(MIN_SUPPORTED_MACOS.minor, 0);
    assert_eq!(MIN_SUPPORTED_MACOS.to_string(), "13.0");
}

#[test]
fn parses_sw_vers_output() {
    // `sw_vers -productVersion` 的典型输出
    assert_eq!(
        platform::parse_sw_vers("14.5").expect("14.5"),
        MacOsVersion::new(14, 5)
    );
    assert_eq!(
        platform::parse_sw_vers("13.0\n").expect("带换行也要能解析"),
        MacOsVersion::new(13, 0)
    );
    assert_eq!(
        platform::parse_sw_vers("13").expect("只有主版本"),
        MacOsVersion::new(13, 0)
    );
    // 10.x 时代的三段版本号
    assert_eq!(
        platform::parse_sw_vers("10.15.7").expect("10.15.7"),
        MacOsVersion::new(10, 15)
    );
}

#[test]
fn rejects_garbage_version_strings() {
    for bad in ["", "  ", "Ventura", "13.x", "-1.0"] {
        assert!(platform::parse_sw_vers(bad).is_err(), "{bad:?} 必须被拒绝");
    }
}

#[test]
fn supported_range_starts_at_13() {
    // 不受支持：12 及更早
    assert!(!platform::is_supported(MacOsVersion::new(12, 7)));
    assert!(!platform::is_supported(MacOsVersion::new(10, 15)));

    // 受支持：13、14（用户明确要求这两代都要支持）
    assert!(platform::is_supported(MacOsVersion::new(13, 0)));
    assert!(platform::is_supported(MacOsVersion::new(13, 6)));
    assert!(platform::is_supported(MacOsVersion::new(14, 0)));
    assert!(platform::is_supported(MacOsVersion::new(14, 5)));

    // 未来版本按「向上兼容」处理：不因为版本更高而拒绝启动
    assert!(platform::is_supported(MacOsVersion::new(15, 0)));
}

/// 给诊断页与 `doctor` 用的一句话结论。
#[test]
fn support_report_is_human_readable() {
    let old = platform::support_report(MacOsVersion::new(12, 7));
    assert!(!old.supported);
    assert!(
        old.message.contains("12.7") && old.message.contains("13"),
        "要同时指出当前版本与最低要求：{}",
        old.message
    );

    let ok = platform::support_report(MacOsVersion::new(14, 5));
    assert!(ok.supported);
    assert!(ok.message.contains("14.5"));
}

/// 版本号包含预发布/补丁时应按数值比较，而不是字符串比较
#[test]
fn version_ordering_is_numeric_not_lexicographic() {
    assert!(MacOsVersion::new(13, 0) < MacOsVersion::new(13, 1));
    assert!(MacOsVersion::new(9, 0) < MacOsVersion::new(13, 0));
    assert!(MacOsVersion::new(14, 0) > MacOsVersion::new(13, 99));
}
