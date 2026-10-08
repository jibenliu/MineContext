//! 开发工具：从源码抽取契约夹具。
//!
//! 只有一个职责 —— 保证 `fixtures/contract/` 里的清单是**生成的**而不是手写的，
//! 因此它读源码、写 JSON，不做别的事。
//!
//! 用法：`cargo run -p xtask -- extract-used-ipc-channels`

mod channel_refs;
mod comment_style;
mod dep_direction;
mod log_redaction;
mod macos_artifacts;
mod macos_target;
mod no_naive_datetime;
mod no_test_fixtures;
mod prompt_embedded;
mod provider_purity;
mod security_tests;
mod shell_vars;
mod summary_invariant;
mod theme_tokens;
mod troubleshooting;
mod values;
mod version_consistency;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use regex::Regex;
use serde::Serialize;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("extract-used-ipc-channels") => match extract_used_ipc_channels() {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        },
        Some("check-version-consistency") => {
            match repo_root().and_then(|root| version_consistency::check(&root)) {
                Ok(()) => {
                    println!("版本号一致（Cargo workspace / 外壳 crate / package.json / tauri.conf.json）");
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-shell-vars") => match repo_root().and_then(|root| shell_vars::check(&root)) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        },
        Some("check-troubleshooting-freshness") => {
            match repo_root().and_then(|root| troubleshooting::check(&root)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-macos-target") => match repo_root().and_then(|root| macos_target::check(&root))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        },
        Some("check-dep-direction") => {
            match repo_root().and_then(|root| dep_direction::check(&root)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-no-test-fixtures") => {
            match repo_root().and_then(|root| no_test_fixtures::check(&root)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-no-naive-datetime") => {
            match repo_root().and_then(|root| no_naive_datetime::check(&root)) {
                Ok(()) => {
                    println!("时间用法检查通过");
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-prompt-embedded") => {
            match repo_root().and_then(|root| prompt_embedded::check(&root)) {
                Ok(()) => {
                    let has_prompts = repo_root()
                        .map(|root| prompt_embedded::has_prompts(&root))
                        .unwrap_or(false);
                    if has_prompts {
                        println!("提示词内嵌检查通过");
                    } else {
                        println!("通过：当前尚无提示词文件（提示词目录出现后此检查会自动生效）");
                    }
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-provider-purity") => {
            match repo_root().and_then(|root| provider_purity::check(&root)) {
                Ok(()) => {
                    println!("Provider 纯净性检查通过");
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-summary-invariant") => {
            match repo_root().and_then(|root| summary_invariant::check(&root)) {
                Ok(()) => {
                    println!("阶段总结不变量哨兵检查完成（两个关键测试都在）");
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    // 未开启强制时只提示、不阻断（与旧脚本行为一致：只在
                    // MC_ENFORCE_SUMMARY_INVARIANT=1 时失败）
                    if std::env::var(summary_invariant::ENFORCE_ENV).as_deref() == Ok("1") {
                        eprintln!("{message}");
                        ExitCode::FAILURE
                    } else {
                        println!("{message}");
                        println!("阶段总结不变量哨兵检查完成");
                        ExitCode::SUCCESS
                    }
                }
            }
        }
        Some("check-security-tests") => {
            match repo_root().and_then(|root| security_tests::check(&root)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-log-redaction") => {
            match repo_root().and_then(|root| log_redaction::check(&root)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("now-ms") => {
            println!("{}", values::now_ms());
            ExitCode::SUCCESS
        }
        Some("diag-fields") => match args.get(1) {
            Some(json) => match values::diag_fields(json) {
                Ok(message) => {
                    println!("{message}");
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            },
            None => {
                eprintln!("用法：cargo run -p xtask -- diag-fields <json>");
                ExitCode::from(2)
            }
        },
        Some("percentiles") => {
            let values: Vec<u64> = args[1..]
                .iter()
                .filter_map(|value| value.parse().ok())
                .collect();
            println!("{}", values::percentiles(&values));
            ExitCode::SUCCESS
        }
        Some("json-field") => match (args.get(1), args.get(2)) {
            (Some(path), Some(key)) => match values::json_field(Path::new(path), key) {
                Ok(value) => {
                    println!("{value}");
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            },
            _ => {
                eprintln!("用法：cargo run -p xtask -- json-field <file> <key>");
                ExitCode::from(2)
            }
        },
        Some("check-macos-artifacts") => {
            let minimum = args
                .iter()
                .position(|arg| arg == "--min")
                .and_then(|index| args.get(index + 1))
                .map(|value| {
                    let mut parts = value.split('.');
                    let major = parts.next().and_then(|v| v.parse().ok()).unwrap_or(13);
                    let minor = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
                    (major, minor)
                })
                .unwrap_or((13, 0));
            match repo_root().and_then(|root| macos_artifacts::check(&root, minimum)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-comment-style") => {
            match repo_root().and_then(|root| comment_style::check(&root)) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    eprintln!("{message}");
                    ExitCode::FAILURE
                }
            }
        }
        Some("check-theme-tokens") => match repo_root().and_then(|root| theme_tokens::check(&root))
        {
            Ok(()) => {
                println!("前端样式变量可解析（无未定义变量、无裸用 Arco 三元组）");
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("{message}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!(
                "用法：cargo run -p xtask -- <extract-used-ipc-channels|check-shell-vars|check-troubleshooting-freshness|check-macos-target|check-dep-direction|check-no-test-fixtures|check-security-tests|check-log-redaction|check-macos-artifacts [--min X.Y]|check-comment-style|check-theme-tokens|check-version-consistency|json-field|now-ms|percentiles|diag-fields>"
            );
            ExitCode::from(2)
        }
    }
}

fn extract_used_ipc_channels() -> Result<(), String> {
    let root = repo_root()?;
    let out = root.join("fixtures/contract/used-ipc-channels.json");

    // 渠道的**唯一来源**是适配层的渠道表（所有表都在同一个文件里）。
    let channel_map_file = root.join("frontend/src/renderer/src/adapters/channel-map.ts");
    let channel_text = fs::read_to_string(&channel_map_file)
        .map_err(|error| format!("读不到 {}：{error}", channel_map_file.display()))?;
    let literal_key = Regex::new(r"(?m)^\s{2}'([^']+)':").expect("内置正则");

    let mut used: BTreeMap<String, String> = BTreeMap::new();

    for capture in literal_key.captures_iter(&channel_text) {
        used.insert(
            capture[1].to_string(),
            "adapters/channel-map.ts".to_string(),
        );
    }

    // 另一半：业务代码的调用点。渠道表覆盖不到它们，这里反查并如实列出。
    let known: BTreeSet<String> = used.keys().cloned().collect();
    let sources = channel_refs::renderer_sources(&root)?;
    let enums = channel_refs::enum_members(&root)?;
    let unknown = channel_refs::unresolved(&sources, &enums, &known);

    let document = Manifest {
        note: "由 `cargo run -p xtask -- extract-used-ipc-channels` 从适配层渠道表抽取。\
               这是适配层必须覆盖的渠道清单；不得手工编辑。",
        channels: used
            .iter()
            .map(|(channel, source)| Channel {
                channel: channel.clone(),
                source: source.clone(),
            })
            .collect(),
        unknown_references: unknown,
    };

    let channel_count = document.channels.len();
    let unknown_count = document.unknown_references.len();
    let unknown_sample = document.unknown_references.clone();

    if let Some(parent) = out.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("建目录失败：{error}"))?;
    }
    let mut body =
        serde_json::to_string_pretty(&document).map_err(|error| format!("序列化失败：{error}"))?;
    body.push('\n');
    fs::write(&out, body).map_err(|error| format!("写不出 {}：{error}", out.display()))?;

    let relative = out
        .strip_prefix(&root)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| out.display().to_string());
    println!("已写出 {relative}：{channel_count} 个被使用的渠道");
    if unknown_count > 0 {
        return Err(format!(
            "{unknown_count} 个引用无法解析：{unknown_sample:?}"
        ));
    }
    Ok(())
}

/// 生成的清单：字段顺序即文件里的顺序，因此 `note` 在最前。
#[derive(Serialize)]
struct Manifest {
    note: &'static str,
    channels: Vec<Channel>,
    unknown_references: Vec<String>,
}

#[derive(Serialize)]
struct Channel {
    channel: String,
    source: String,
}

/// 仓库根：从可执行文件位置往上找 `Cargo.toml` 与 `frontend/` 同时存在的那一层。
fn repo_root() -> Result<PathBuf, String> {
    let mut dir = std::env::current_dir().map_err(|error| format!("取不到当前目录：{error}"))?;
    loop {
        if dir.join("Cargo.toml").is_file() && dir.join("frontend").is_dir() {
            return Ok(dir);
        }
        if !dir.pop() {
            return Err("找不到仓库根（Cargo.toml 与 frontend/ 同时存在的那一层）".to_string());
        }
    }
}
