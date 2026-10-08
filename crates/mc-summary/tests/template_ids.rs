// 模板按 id 解析：已知 id 可取到，未知 id 必须报错并列出可用值。
//
// 静默退回默认模板是最坏的选择：用户改了配置、以为模板生效了，实际拿到的是
// 默认模板的产出，而且没有任何提示。

use mc_summary::template::SummaryTemplate;

#[test]
fn resolves_the_builtin_work_stage_template() {
    let template = SummaryTemplate::from_id("work_stage").expect("内置模板应当可用");

    assert_eq!(template.id, "work_stage");
    assert!(!template.fields.is_empty(), "内置模板应当带字段定义");
}

#[test]
fn resolves_the_detailed_work_stage_template() {
    let template = SummaryTemplate::from_id("work_stage_detailed").expect("内置模板应当可用");

    assert_eq!(template.id, "work_stage_detailed");
    // 与 work_stage 必须是**可区分的**产出：否则「换模板」在记录上看得出来、
    // 在总结里看不出来，等于没换。
    assert!(
        template.fields.iter().any(|field| field.id == "highlights"),
        "详细模板必须多出 work_stage 没有的字段：{template:?}"
    );
}

/// 清单与解析分支必须一致：清单里列了却解析不出来（或反过来），
/// 错误消息就会骗人。
#[test]
fn every_listed_builtin_id_resolves() {
    for id in SummaryTemplate::BUILTIN_IDS {
        let template = SummaryTemplate::from_id(id)
            .unwrap_or_else(|error| panic!("清单里的 `{id}` 必须能解析：{error}"));
        assert_eq!(&template.id, id, "解析出来的模板 id 要和请求的一致");
    }
}

#[test]
fn unknown_id_is_rejected_and_lists_available_ids() {
    let error = SummaryTemplate::from_id("mine").expect_err("未知 id 必须报错");

    assert!(error.contains("mine"), "错误要指出是哪个 id：{error}");
    assert!(error.contains("work_stage"), "错误要列出可用 id：{error}");
    assert!(
        error.contains("work_stage_detailed"),
        "可用 id 要全部列出来，不能只列第一个：{error}"
    );
}

// 用户自定义模板（YAML）优先于内置 id；解析失败必须报错，不静默退回内置模板。
#[test]
fn user_yaml_wins_over_the_builtin_id() {
    let yaml = "id: mine\nname: 我的模板\nfields:\n  - id: time_range\n    label: 时间段\n    kind: time_range\n";
    let template = SummaryTemplate::resolve("work_stage", Some(yaml)).expect("合法 YAML 应当可用");

    assert_eq!(template.id, "mine");
}

#[test]
fn blank_yaml_falls_back_to_the_builtin_id() {
    let template =
        SummaryTemplate::resolve("work_stage_detailed", Some("   ")).expect("空白视为未配置");

    assert_eq!(template.id, "work_stage_detailed");
}

#[test]
fn broken_yaml_is_rejected_loudly() {
    let error =
        SummaryTemplate::resolve("work_stage", Some("id: [未闭合")).expect_err("坏 YAML 必须报错");

    assert!(error.contains("模板"), "错误要说清是模板的问题：{error}");
}
