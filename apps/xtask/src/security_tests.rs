//! `check-security-tests`：规划里承诺的安全与隐私属性，必须真的各有测试。
//!
//! 承诺写在文档里、测试写在代码里，两者会漂移 —— 改名、删文件、加 `#[ignore]`
//! 都不会让任何测试变红，只会让「我们验证过这条」变成一句空话。这里把
//! 「属性 → 具体测试」的映射钉住：要么映射到真实存在且未被忽略的测试，要么
//! **显式登记豁免**并写明原因。只做静态断言，不重复跑测试。
//!
//! 表与逻辑在同一个文件里，改一处不会漏掉另一处。

use std::fs;
use std::path::Path;

use regex::Regex;

const EXEMPT: &str = "EXEMPT";

/// 属性名 → （测试文件，测试函数名）。第二项为 `EXEMPT` 时第三项是豁免原因。
const SECURITY_TESTS: &[(&str, &str, &str)] = &[
    (
        "upload_disabled_by_default",
        "crates/mc-server/tests/privacy_upload.rs",
        "no_provider_is_built_when_upload_is_disabled",
    ),
    (
        "blocked_app_image_not_written_to_disk",
        "crates/mc-server/tests/privacy_blocklist.rs",
        "blocked_app_frames_are_not_persisted",
    ),
    (
        "blocked_app_never_sent_to_provider",
        "crates/mc-server/tests/retrieval_chat.rs",
        "chat_never_reads_blocked_content",
    ),
    (
        "fail_closed_on_rule_engine_error",
        "crates/mc-server/tests/privacy_redaction.rs",
        "invalid_patterns_do_not_produce_a_working_redactor",
    ),
    (
        "api_key_never_appears_in_config_file",
        "apps/mc-cli/tests/cli.rs",
        "migrate_writes_toml_and_never_leaks_plaintext_secret",
    ),
    (
        "api_key_never_appears_in_logs",
        "crates/mc-server/tests/security_surface.rs",
        "api_key_never_reaches_diagnostics_or_errors",
    ),
    (
        "diagnostics_export_excludes_content",
        "crates/mc-server/tests/diagnostics_export.rs",
        "export_never_contains_content_or_secrets",
    ),
    (
        "token_required_on_all_v1_routes",
        "crates/mc-server/tests/security_surface.rs",
        "every_api_route_requires_the_token",
    ),
    (
        "host_header_must_be_loopback",
        "crates/mc-server/tests/control_plane.rs",
        "host_header_must_be_loopback",
    ),
    (
        "runtime_json_is_0600",
        "crates/mc-server/tests/control_plane.rs",
        "runtime_json_written_with_0600",
    ),
    (
        "capture_paths_not_exposed_in_api",
        "crates/mc-server/tests/security_surface.rs",
        "responses_never_expose_the_data_directory",
    ),
];

pub fn check(root: &Path) -> Result<(), String> {
    let mut problems = Vec::new();
    let mut covered = 0;
    let mut exempted = 0;

    for (property, path, test_name) in SECURITY_TESTS {
        if *path == EXEMPT {
            exempted += 1;
            println!("  · {property}：豁免（{test_name}）");
            continue;
        }

        let file = root.join(path);
        if !file.is_file() {
            problems.push(format!("{property}: 测试文件不存在 {path}"));
            continue;
        }

        let text = fs::read_to_string(&file)
            .map_err(|error| format!("无法读取 {}：{error}", file.display()))?;
        // 测试函数名可能换行写（`async fn name(`），因此按词匹配
        let defined = Regex::new(&format!(r"\bfn\s+{}\b", regex::escape(test_name)))
            .expect("内置正则应当合法");
        if !defined.is_match(&text) {
            problems.push(format!("{property}: {path} 里没有测试 {test_name}()"));
            continue;
        }

        // 被 `#[ignore]` 的安全测试等于没有：它不会在门禁里跑
        let ignored = Regex::new(&format!(
            r"#\[ignore[^\]]*\]\s*(?:#\[[^\]]*\]\s*)*\s*(?:async\s+)?fn\s+{}\b",
            regex::escape(test_name)
        ))
        .expect("内置正则应当合法");
        if ignored.is_match(&text) {
            problems.push(format!(
                "{property}: 测试 {test_name}() 被 #[ignore]，门禁里不会执行"
            ));
            continue;
        }

        covered += 1;
    }

    if problems.is_empty() {
        println!(
            "安全属性门禁检查通过（{covered} 条有测试，{exempted} 条登记豁免，共 {} 条）",
            SECURITY_TESTS.len()
        );
        return Ok(());
    }

    let mut message = String::from("安全属性没有对应的测试：\n");
    for problem in &problems {
        message.push_str(&format!("  - {problem}\n"));
    }
    message.push_str("\n补测试，或在 apps/xtask/src/security_tests.rs 里显式登记豁免并写明原因。");
    Err(message)
}
