//! 用户自定义活动规则。
//!
//! 规则判定：用户可配置的关键词/应用/窗口规则。
//! 规则命中就不必调模型 —— 这既是省 token 的主要手段，
//! 也是让用户真正掌控「系统认为我在干什么」的唯一途径
//! （只信模型的代价是成本与稳定性都失控）。
//!
//! 匹配语义：**跨类别 AND，类别内 OR**。
//! 例如 `apps: [Chrome]` + `keywords: [需求]` 表示「Chrome 且提到需求」。
//! `exclude` 优先级最高 —— 命中排除条件时整条规则不成立。

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::observation::ObservationSummary;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AiPreference {
    /// 规则自己就能决定，不需要模型（默认）
    #[default]
    None,
    /// 先用便宜的档位辅助
    Assist,
    /// 规则要求深度分析
    Deep,
}

/// 规则匹配结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleMatch {
    None,
    Matched {
        rule_id: String,
        name: String,
        category: Option<String>,
        ai: AiPreference,
        priority: i32,
    },
    /// 多条**同优先级**规则同时命中 —— 交给 AI 消解，
    /// 而不是随便挑一条（随便挑会让结果不可预测、也难排查）。
    Conflict {
        rule_ids: Vec<String>,
    },
}

impl RuleMatch {
    pub const fn is_matched(&self) -> bool {
        matches!(self, Self::Matched { .. })
    }

    pub const fn is_conflict(&self) -> bool {
        matches!(self, Self::Conflict { .. })
    }

    pub const fn ai_preference(&self) -> AiPreference {
        match self {
            Self::Matched { ai, .. } => *ai,
            _ => AiPreference::None,
        }
    }

    pub const fn priority(&self) -> i32 {
        match self {
            Self::Matched { priority, .. } => *priority,
            _ => 0,
        }
    }

    pub fn rule_id(&self) -> Option<&str> {
        match self {
            Self::Matched { rule_id, .. } => Some(rule_id),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------- 反序列化形态

/// 当前支持的规则文件版本。
pub const SUPPORTED_RULES_VERSION: u32 = 1;

#[derive(Debug, Deserialize)]
struct RulesFile {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    activities: Vec<RuleSpec>,
}

#[derive(Debug, Deserialize)]
struct RuleSpec {
    #[serde(default)]
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    priority: i32,
    #[serde(default)]
    ai: AiPreference,
    #[serde(default = "default_min_duration")]
    minimum_duration_secs: u64,
    #[serde(default)]
    triggers: TriggerSpec,
    #[serde(default)]
    exclude: TriggerSpec,
}

const fn default_min_duration() -> u64 {
    60
}

#[derive(Debug, Deserialize, Default)]
struct TriggerSpec {
    #[serde(default)]
    apps: Vec<String>,
    #[serde(default)]
    window_patterns: Vec<String>,
    #[serde(default)]
    domains: Vec<String>,
    #[serde(default)]
    keywords: Vec<String>,
}

impl TriggerSpec {
    fn is_empty(&self) -> bool {
        self.apps.is_empty()
            && self.window_patterns.is_empty()
            && self.domains.is_empty()
            && self.keywords.is_empty()
    }
}

// ---------------------------------------------------------------- 编译后的规则

#[derive(Debug)]
struct CompiledTriggers {
    apps: Vec<String>,
    patterns: Vec<Regex>,
    domains: Vec<String>,
    keywords: Vec<String>,
}

impl CompiledTriggers {
    fn compile(spec: &TriggerSpec, rule_id: &str) -> Result<Self, String> {
        let mut patterns = Vec::new();
        for pattern in &spec.window_patterns {
            let regex = Regex::new(pattern)
                .map_err(|e| format!("规则 `{rule_id}` 的窗口正则 `{pattern}` 非法：{e}"))?;
            patterns.push(regex);
        }

        Ok(Self {
            apps: spec.apps.iter().map(|a| a.to_lowercase()).collect(),
            patterns,
            domains: spec.domains.iter().map(|d| d.to_lowercase()).collect(),
            keywords: spec.keywords.iter().map(|k| k.to_lowercase()).collect(),
        })
    }

    fn is_empty(&self) -> bool {
        self.apps.is_empty()
            && self.patterns.is_empty()
            && self.domains.is_empty()
            && self.keywords.is_empty()
    }

    fn matches(&self, observation: &ObservationSummary) -> bool {
        let app = observation.app().to_lowercase();
        let title = observation.window_title.as_deref().unwrap_or("");
        let haystack = observation.haystack().to_lowercase();
        let domain = observation.domain.as_deref().unwrap_or("").to_lowercase();

        // 已指定的类别都必须命中（AND）；类别内任一项命中即可（OR）
        if !self.apps.is_empty() && !self.apps.iter().any(|want| app_matches(&app, want)) {
            return false;
        }

        if !self.patterns.is_empty()
            && !self
                .patterns
                .iter()
                .any(|re| re.is_match(title) || re.is_match(&haystack))
        {
            return false;
        }

        if !self.domains.is_empty()
            && !self
                .domains
                .iter()
                .any(|want| domain_matches(&domain, &haystack, want))
        {
            return false;
        }

        if !self.keywords.is_empty() && !self.keywords.iter().any(|want| haystack.contains(want)) {
            return false;
        }

        true
    }
}

/// 常见应用的别名。
///
/// 必要性：系统报的进程名往往是全称（`Visual Studio Code`），而用户在规则里
/// 习惯写简称（`VSCode`）。纯字符串包含关系处理不了这种缩写，
/// 于是「规则命不中 → 退化成调模型」—— 恰好是我们最想避免的路径。
///
/// 只列高频项；用户遇到未覆盖的应用时，可以在规则里把多个写法都写进 `apps`。
const APP_ALIASES: &[(&str, &[&str])] = &[
    ("vscode", &["visual studio code", "code - insiders"]),
    ("code", &["visual studio code"]),
    ("chrome", &["google chrome"]),
    ("edge", &["microsoft edge"]),
    ("goland", &["goland", "go land"]),
    ("intellij", &["intellij idea"]),
    ("terminal", &["iterm2", "iterm", "warp", "alacritty"]),
    ("wechat", &["wechat", "微信"]),
];

/// 应用名匹配：精确、互相包含，或命中别名表。
fn app_matches(actual: &str, wanted: &str) -> bool {
    if actual.is_empty() {
        return false;
    }
    if actual == wanted || actual.contains(wanted) || wanted.contains(actual) {
        return true;
    }

    let mut candidates: Vec<&str> = vec![wanted];
    for (canonical, aliases) in APP_ALIASES {
        if *canonical == wanted || aliases.contains(&wanted) {
            candidates.push(canonical);
            candidates.extend_from_slice(aliases);
        }
    }

    candidates
        .iter()
        .any(|candidate| actual == *candidate || actual.contains(*candidate))
}

/// 域名匹配：显式 domain 字段优先，其次看窗口标题里是否出现该域名。
fn domain_matches(explicit: &str, haystack: &str, wanted: &str) -> bool {
    if !explicit.is_empty() && (explicit == wanted || explicit.ends_with(&format!(".{wanted}"))) {
        return true;
    }
    haystack.contains(wanted)
}

#[derive(Debug)]
pub struct Rule {
    pub id: String,
    pub name: String,
    pub category: Option<String>,
    pub priority: i32,
    pub ai: AiPreference,
    pub minimum_duration_secs: u64,
    triggers: CompiledTriggers,
    exclude: CompiledTriggers,
}

impl Rule {
    fn matches(&self, observation: &ObservationSummary) -> bool {
        // 排除条件优先级最高：命中即整条规则不成立
        if !self.exclude.is_empty() && self.exclude.matches(observation) {
            return false;
        }
        self.triggers.matches(observation)
    }
}

#[derive(Debug, Default)]
pub struct RuleSet {
    rules: Vec<Rule>,
}

impl RuleSet {
    pub fn parse_yaml(text: &str) -> Result<Self, String> {
        let file: RulesFile =
            serde_yaml::from_str(text).map_err(|e| format!("规则文件无法解析：{e}"))?;

        // 版本校验：拒绝未知版本，而不是按当前语义「猜着解析」——
        // 猜错的后果是规则静默失效，用户根本查不出原因。
        // 0 表示未声明版本号（旧规则文件），按 v1 处理。
        if file.version > SUPPORTED_RULES_VERSION {
            return Err(format!(
                "规则文件版本 {} 高于本版本支持的 {}，请升级应用",
                file.version, SUPPORTED_RULES_VERSION
            ));
        }

        let mut rules = Vec::new();
        let mut seen: Vec<String> = Vec::new();

        for spec in file.activities {
            if spec.id.trim().is_empty() {
                return Err(format!("规则 `{}` 缺少 id 字段", spec.name));
            }
            if seen.contains(&spec.id) {
                return Err(format!("规则 id 重复：{}", spec.id));
            }
            if spec.triggers.is_empty() {
                return Err(format!(
                    "规则 `{}` 没有任何触发条件 —— 那会命中一切，拒绝加载",
                    spec.id
                ));
            }

            let triggers = CompiledTriggers::compile(&spec.triggers, &spec.id)?;
            let exclude = CompiledTriggers::compile(&spec.exclude, &spec.id)?;

            seen.push(spec.id.clone());
            rules.push(Rule {
                id: spec.id,
                name: spec.name,
                category: spec.category,
                priority: spec.priority,
                ai: spec.ai,
                minimum_duration_secs: spec.minimum_duration_secs,
                triggers,
                exclude,
            });
        }

        Ok(Self { rules })
    }

    pub fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.rules.len()
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// 判定一条观测属于哪个活动。
    pub fn classify(&self, observation: &ObservationSummary) -> RuleMatch {
        let hits: Vec<&Rule> = self
            .rules
            .iter()
            .filter(|rule| rule.matches(observation))
            .collect();

        if hits.is_empty() {
            return RuleMatch::None;
        }

        let best_priority = hits.iter().map(|rule| rule.priority).max().unwrap_or(0);
        let mut best: Vec<&Rule> = hits
            .into_iter()
            .filter(|rule| rule.priority == best_priority)
            .collect();

        if best.len() > 1 {
            let mut rule_ids: Vec<String> = best.iter().map(|r| r.id.clone()).collect();
            rule_ids.sort();
            return RuleMatch::Conflict { rule_ids };
        }

        let rule = best.pop().expect("至少一条");
        RuleMatch::Matched {
            rule_id: rule.id.clone(),
            name: rule.name.clone(),
            category: rule.category.clone(),
            ai: rule.ai,
            priority: rule.priority,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_rules_version_is_rejected() {
        let yaml = r#"
version: 99
activities:
  - id: a
    name: "甲"
    triggers: { apps: ["X"] }
"#;
        let error = RuleSet::parse_yaml(yaml).unwrap_err();
        assert!(error.contains("99"), "错误信息应指出版本号：{error}");
    }

    #[test]
    fn undeclared_version_is_treated_as_v1() {
        let yaml = r#"
activities:
  - id: a
    name: "甲"
    triggers: { apps: ["X"] }
"#;
        assert!(RuleSet::parse_yaml(yaml).is_ok(), "未声明版本时按 v1 处理");
    }

    #[test]
    fn app_matching_accepts_common_aliases() {
        // 系统报全称、规则写简称 —— 这是最常见的失配来源
        assert!(app_matches("visual studio code", "vscode"));
        assert!(app_matches("vscode", "visual studio code"));
        assert!(app_matches("google chrome", "chrome"));
        assert!(app_matches("iterm2", "terminal"));
        // 中文应用名
        assert!(app_matches("微信", "wechat"));

        assert!(app_matches("chrome", "chrome"));
        assert!(!app_matches("", "chrome"));
        assert!(!app_matches("safari", "chrome"));
    }

    #[test]
    fn domain_matching_handles_subdomains() {
        assert!(domain_matches("jira.company.com", "", "jira.company.com"));
        assert!(domain_matches("a.jira.company.com", "", "jira.company.com"));
        assert!(!domain_matches("evil.com", "", "jira.company.com"));
        assert!(domain_matches(
            "",
            "APEX - jira.company.com",
            "jira.company.com"
        ));
    }
}
