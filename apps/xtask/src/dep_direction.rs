//! `check-dep-direction`：workspace 内部 crate 只能依赖「更底层」的 crate ——
//! mc-domain 保持纯逻辑（无 IO），mc-common 不依赖任何内部 crate。
//!
//! 判定用的是 `cargo metadata --no-deps` 的真实依赖图，**dev/build 依赖不参与**
//! （它们是测试脚手架，不影响发布产物）。

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// 层级：数字越小越底层。生产依赖只能「向下」或同层。
const LAYERS: &[(&str, u64)] = &[
    ("mc-common", 0),
    ("mc-config", 1),
    ("mc-domain", 1),
    ("mc-storage", 2),
    ("mc-providers", 2),
    ("mc-capture", 2),
    ("mc-pipeline", 3),
    ("mc-summary", 3),
    ("mc-search", 3),
    ("mc-memory", 3),
    ("mc-server", 4),
    ("mc-daemon", 5),
    ("mc-cli", 5),
    // 测试脚手架可以依赖任何东西（它永远不进发布产物）
    ("mc-testkit", 9),
];

/// mc-domain 必须是纯逻辑：不得出现 IO/运行时依赖。
const FORBIDDEN_FOR_DOMAIN: &[&str] = &[
    "tokio", "rusqlite", "reqwest", "axum", "hyper", "notify", "sqlx",
];

pub fn check(root: &Path) -> Result<(), String> {
    let output = Command::new("cargo")
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(root)
        .output()
        .map_err(|error| format!("无法执行 cargo metadata：{error}"))?;
    if !output.status.success() {
        return Err(format!(
            "cargo metadata 失败：{}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    let metadata: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("cargo metadata 输出无法解析：{error}"))?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or_else(|| "cargo metadata 里没有 packages".to_string())?;

    let workspace: Vec<&str> = packages
        .iter()
        .filter_map(|package| package["name"].as_str())
        .collect();

    let mut edges = Vec::new();
    for package in packages {
        let Some(name) = package["name"].as_str() else {
            continue;
        };
        let Some(dependencies) = package["dependencies"].as_array() else {
            continue;
        };
        for dependency in dependencies {
            let (Some(dep_name), kind) = (dependency["name"].as_str(), dependency["kind"].as_str())
            else {
                continue;
            };
            edges.push(Edge {
                from: name.to_string(),
                to: dep_name.to_string(),
                kind: kind.map(|value| value.to_string()),
            });
        }
    }

    let violations = evaluate(&workspace, &edges);

    if violations.is_empty() {
        println!("依赖方向检查通过（{} 个 crate）", workspace.len());
        return Ok(());
    }

    let mut message = String::from("依赖方向检查失败：\n");
    for violation in &violations {
        message.push_str(&format!("  - {violation}\n"));
    }
    Err(message)
}

/// 一条生产依赖边。
pub struct Edge {
    pub from: String,
    pub to: String,
    /// `None` 表示普通依赖；dev/build 不参与判定。
    pub kind: Option<String>,
}

/// 判定依赖图的违规项。
///
/// **与 `cargo metadata` 解耦**：违规必须被拦这件事也要能被单元测试覆盖 —— 否则
/// 这条守卫只有「干净路径通过」一类证据（往真实 `Cargo.toml` 里塞一条向上依赖
/// 会连带破坏构建，代价太大）。
pub fn evaluate(workspace: &[&str], edges: &[Edge]) -> Vec<String> {
    let layers: BTreeMap<&str, u64> = LAYERS.iter().copied().collect();
    let mut violations = Vec::new();
    for edge in edges {
        // kind == "dev" / "build" 不影响生产依赖图，跳过
        if !matches!(edge.kind.as_deref(), None | Some("normal")) {
            continue;
        }
        let Some(layer) = layers.get(edge.from.as_str()) else {
            continue;
        };
        if workspace.contains(&edge.to.as_str()) {
            if let Some(dep_layer) = layers.get(edge.to.as_str()) {
                if dep_layer > layer {
                    violations.push(format!(
                        "{} (层 {layer}) 依赖了更上层的 {} (层 {dep_layer})",
                        edge.from, edge.to
                    ));
                }
            }
        }
        if edge.from == "mc-domain" && FORBIDDEN_FOR_DOMAIN.contains(&edge.to.as_str()) {
            violations.push(format!(
                "mc-domain 不得依赖 {}（必须保持纯逻辑、可极速测试）",
                edge.to
            ));
        }
    }
    violations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(from: &str, to: &str, kind: Option<&str>) -> Edge {
        Edge {
            from: from.to_string(),
            to: to.to_string(),
            kind: kind.map(|value| value.to_string()),
        }
    }

    #[test]
    fn upward_dependency_is_a_violation() {
        let crates = ["mc-common", "mc-server"];
        let violations = evaluate(&crates, &[edge("mc-common", "mc-server", None)]);

        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("更上层"), "{violations:?}");
    }

    #[test]
    fn domain_may_not_depend_on_runtime_crates() {
        let crates = ["mc-domain"];
        let violations = evaluate(&crates, &[edge("mc-domain", "tokio", None)]);

        assert_eq!(violations.len(), 1, "{violations:?}");
        assert!(violations[0].contains("不得依赖 tokio"), "{violations:?}");
    }

    #[test]
    fn dev_and_build_dependencies_are_ignored() {
        let crates = ["mc-common", "mc-server"];

        assert!(evaluate(&crates, &[edge("mc-common", "mc-server", Some("dev"))]).is_empty());
        assert!(evaluate(&crates, &[edge("mc-common", "mc-server", Some("build"))]).is_empty());
    }

    #[test]
    fn downward_dependency_and_unknown_crate_are_fine() {
        let crates = ["mc-common", "mc-server"];

        assert!(evaluate(&crates, &[edge("mc-server", "mc-common", None)]).is_empty());
        // 不在层级表里的 crate 不参与方向判定
        assert!(evaluate(&crates, &[edge("xtask", "mc-server", None)]).is_empty());
    }
}
