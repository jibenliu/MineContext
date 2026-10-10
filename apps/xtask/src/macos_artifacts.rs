//! `check-macos-artifacts`：校验**真实 Mach-O 产物**的 minos 落在
//! `[最低要求, 声明值]` 区间内、构建期与运行期的声明一致、且没有链接
//! ScreenCaptureKit。
//!
//! 不能只看 `.cargo/config.toml`：配置说 13、产物 minos 是 10.12 这种漂移
//! 只有查产物本身才发现得了。
//!
//! 产物最多 80+ 个、每个要跑 vtool/otool —— 这里**并行检查**：串行跑一次
//! 几十分钟，慢到没人愿意等就等于没有这条守卫。

use std::cmp::Reverse;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use regex::Regex;

const WORKERS: usize = 8;
const TOOL_TIMEOUT: Duration = Duration::from_secs(30);

pub fn check(root: &Path, minimum: (u64, u64)) -> Result<(), String> {
    let mut problems = Vec::new();

    // ---- 声明值：构建期 ----
    let config = root.join(".cargo/config.toml");
    let text = fs::read_to_string(&config)
        .map_err(|error| format!("无法读取 {}：{error}", config.display()))?;
    let declared_pattern = Regex::new(r#"MACOSX_DEPLOYMENT_TARGET\s*=\s*"([0-9]+)\.([0-9]+)""#)
        .expect("内置正则应当合法");
    let Some(captures) = declared_pattern.captures(&text) else {
        return Err(
            "macOS 产物校验失败：.cargo/config.toml 里没有 MACOSX_DEPLOYMENT_TARGET".to_string(),
        );
    };
    let declared = pair(&captures);
    if declared < minimum {
        problems.push(format!(
            "声明的部署目标 {}.{} 低于最低要求 {}.{}",
            declared.0, declared.1, minimum.0, minimum.1
        ));
    }

    // ---- 声明值：运行期（必须与构建期一致）----
    let platform = root.join("crates/mc-common/src/platform.rs");
    let platform_src = fs::read_to_string(&platform)
        .map_err(|error| format!("无法读取 {}：{error}", platform.display()))?;
    let runtime_pattern = Regex::new(
        r"MIN_SUPPORTED_MACOS:\s*MacOsVersion\s*=\s*MacOsVersion::new\((\d+),\s*(\d+)\)",
    )
    .expect("内置正则应当合法");
    match runtime_pattern.captures(&platform_src) {
        None => problems.push("找不到 mc_common::platform::MIN_SUPPORTED_MACOS 的定义".to_string()),
        Some(captures) => {
            let runtime = pair(&captures);
            if runtime != declared {
                problems.push(format!(
                    "构建期 {}.{} 与运行期 {}.{} 不一致",
                    declared.0, declared.1, runtime.0, runtime.1
                ));
            }
        }
    }

    // ---- 真实产物 ----
    let candidates = candidates(root)?;
    let results = inspect(&candidates);

    let mut checked = 0usize;
    for (binary, version, has_screen_capture_kit) in results {
        let Some(version) = version else {
            continue;
        };
        checked += 1;
        if version < minimum {
            problems.push(format!(
                "{} 的 minos 是 {}.{}，低于最低要求 {}.{}",
                binary.display(),
                version.0,
                version.1,
                minimum.0,
                minimum.1
            ));
        }
        if version > declared {
            problems.push(format!(
                "{} 的 minos 是 {}.{}，高于声明的 {}.{}（说明有依赖按更高版本编译）",
                binary.display(),
                version.0,
                version.1,
                declared.0,
                declared.1
            ));
        }
        if has_screen_capture_kit {
            problems.push(format!(
                "{} 链接了 ScreenCaptureKit：采集路径应当只依赖 CoreGraphics",
                binary.display()
            ));
        }
    }
    if checked == 0 {
        // Linux / 未打包环境没有 Mach-O：与 launch-check-tauri 一样 SKIP，
        // 不把「还没在本机构建」当成业务冒烟失败。macOS 上缺产物仍要失败。
        if cfg!(target_os = "macos") {
            problems.push("没有找到可校验的 Mach-O 产物（先在 macOS 上构建一次）".to_string());
        } else {
            println!("SKIP: 没有可校验的 Mach-O 产物（非 macOS 或尚未构建）");
            return Ok(());
        }
    }

    if problems.is_empty() {
        println!(
            "macOS 产物校验通过（{checked} 个产物，minos {}.{}，构建期与运行期一致，未链接 ScreenCaptureKit）",
            declared.0, declared.1
        );
        return Ok(());
    }

    let mut message = String::from("macOS 产物校验失败：\n");
    for problem in &problems {
        message.push_str(&format!("  - {problem}\n"));
    }
    Err(message)
}

fn pair(captures: &regex::Captures<'_>) -> (u64, u64) {
    let value = |index: usize| {
        captures
            .get(index)
            .and_then(|part| part.as_str().parse().ok())
            .unwrap_or(0)
    };
    (value(1), value(2))
}

/// 候选产物：两个主二进制（若存在）+ 两个 deps 目录里最近的 40 个。
fn candidates(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut found = Vec::new();
    for pattern in ["target/debug/mc-daemon", "target/debug/mc-cli"] {
        let path = root.join(pattern);
        if path.is_file() {
            found.push(path);
        }
    }

    // 测试二进制：取最近构建的若干个（覆盖到依赖 xcap 的产物）。
    // 上限 40 个：既能覆盖采集相关产物，又不至于让门禁变慢。
    for directory in ["target/debug/deps", "target/release/deps"] {
        let deps = root.join(directory);
        if !deps.is_dir() {
            continue;
        }
        let entries =
            fs::read_dir(&deps).map_err(|error| format!("无法读取 {}：{error}", deps.display()))?;
        let mut built: Vec<(std::time::SystemTime, PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .filter(|path| path.extension().is_none_or(|ext| ext != "d"))
            .filter(|path| path.extension().is_none_or(|ext| ext != "rlib"))
            .filter_map(|path| {
                let modified = fs::metadata(&path).ok()?.modified().ok()?;
                Some((modified, path))
            })
            .collect();
        built.sort_by_key(|(modified, _)| Reverse(*modified));
        found.extend(built.into_iter().take(40).map(|(_, path)| path));
    }
    Ok(found)
}

/// 单个产物的检查结果：路径、读到的 minos、是否链接了 ScreenCaptureKit。
type Inspected = (PathBuf, Option<(u64, u64)>, bool);

/// 并行检查每个产物，返回结果（按输入顺序，问题列表才稳定）。
fn inspect(candidates: &[PathBuf]) -> Vec<Inspected> {
    let next = AtomicUsize::new(0);
    let mut collected: Vec<(usize, Inspected)> = Vec::new();

    thread::scope(|scope| {
        let next = &next;
        let mut handles = Vec::new();
        for _ in 0..WORKERS.min(candidates.len().max(1)) {
            handles.push(scope.spawn(move || {
                let mut local = Vec::new();
                loop {
                    let index = next.fetch_add(1, Ordering::SeqCst);
                    let Some(binary) = candidates.get(index) else {
                        break;
                    };
                    let version = minos_of(binary);
                    let has_screen_capture_kit = version.is_some()
                        && frameworks_of(binary)
                            .iter()
                            .any(|name| name == "ScreenCaptureKit");
                    local.push((index, (binary.clone(), version, has_screen_capture_kit)));
                }
                local
            }));
        }
        for handle in handles {
            if let Ok(local) = handle.join() {
                collected.extend(local);
            }
        }
    });

    collected.sort_by_key(|(index, _)| *index);
    collected
        .into_iter()
        .map(|(_, inspected)| inspected)
        .collect()
}

/// 读取 Mach-O 的 minos。读不出来返回 None（不是 Mach-O）。
fn minos_of(binary: &Path) -> Option<(u64, u64)> {
    let mut output = run("vtool", &["-show-build".to_string(), path_arg(binary)]);
    if output.trim().is_empty() {
        output = run("otool", &["-l".to_string(), path_arg(binary)]);
    }
    let pattern = Regex::new(r"minos\s+(\d+)\.(\d+)").expect("内置正则应当合法");
    pattern.captures(&output).map(|captures| pair(&captures))
}

fn frameworks_of(binary: &Path) -> Vec<String> {
    let output = run("otool", &["-L".to_string(), path_arg(binary)]);
    output
        .lines()
        .skip(1)
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| {
            line.split('/')
                .next_back()
                .and_then(|tail| tail.split(' ').next())
                .map(|name| name.trim().to_string())
        })
        .collect()
}

fn path_arg(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

/// 跑一个工具并取 stdout；失败或超时一律返回空串，调用方按「读不出来」处理。
fn run(tool: &str, args: &[String]) -> String {
    let Ok(mut child) = Command::new(tool)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return String::new();
    };

    let deadline = Instant::now() + TOOL_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return String::new();
            }
        }
    }

    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut output);
    }
    output
}
