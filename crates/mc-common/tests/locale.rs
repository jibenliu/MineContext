use mc_common::locale::Locale;

#[test]
fn locale_detection_preserves_config_fallbacks() {
    for value in ["en", "en-US", "EN", "eN-gB", "en_US"] {
        assert_eq!(Locale::from_config(value), Locale::EnUs, "{value}");
    }
    for value in ["", "zh", "zh-CN", "ZH", "fr", "ja", " en-US", "中文"] {
        assert_eq!(Locale::from_config(value), Locale::ZhCn, "{value}");
    }
}

#[test]
fn locale_wire_values_remain_snake_case() {
    for (locale, wire) in [(Locale::ZhCn, "\"zh_cn\""), (Locale::EnUs, "\"en_us\"")] {
        assert_eq!(serde_json::to_string(&locale).unwrap(), wire);
        assert_eq!(serde_json::from_str::<Locale>(wire).unwrap(), locale);
    }
}
