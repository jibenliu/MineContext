use mc_common::locale::Locale;
use mc_summary::model::SummaryLocale;

#[test]
fn summary_uses_shared_config_language_policy() {
    for value in [
        "en", "en-US", "EN", "eN-gB", "en_US", "zh-CN", "", "fr", " en-US",
    ] {
        assert_eq!(
            SummaryLocale::from_config(value).is_chinese(),
            Locale::from_config(value) == Locale::ZhCn,
            "{value}"
        );
    }
}

#[test]
fn summary_wire_values_remain_lowercase_without_separator() {
    for (locale, wire) in [
        (SummaryLocale::ZhCn, "\"zhcn\""),
        (SummaryLocale::EnUs, "\"enus\""),
    ] {
        assert_eq!(serde_json::to_string(&locale).unwrap(), wire);
        assert_eq!(serde_json::from_str::<SummaryLocale>(wire).unwrap(), locale);
    }
}
