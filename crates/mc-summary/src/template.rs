//! 总结模板：字段化的输出契约。
//!
//! 模板**不是**给模型的提示词片段，而是「总结里必须有哪些信息」的声明。
//! 这样换模型、换提示词都不会让总结结构变形；UI 也能按模板渲染卡片
//! （字段按模板裁剪，不是固定集合）。

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::model::SummaryLocale;

/// 字段类型。**封闭集合**：新增类型必须同时给出渲染实现，
/// 否则就会出现「模板里声明了，但总结里永远是空」。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FieldKind {
    TimeRange,
    ActivityList,
    AppList,
    HighlightList,
    ObservationCount,
    BlockedNotice,
}

impl FieldKind {
    pub const ALL: &'static [FieldKind] = &[
        FieldKind::TimeRange,
        FieldKind::ActivityList,
        FieldKind::AppList,
        FieldKind::HighlightList,
        FieldKind::ObservationCount,
        FieldKind::BlockedNotice,
    ];

    pub const fn label_zh(self) -> &'static str {
        match self {
            Self::TimeRange => "时间段",
            Self::ActivityList => "做过什么",
            Self::AppList => "涉及分类",
            Self::HighlightList => "要点",
            Self::ObservationCount => "采集量",
            Self::BlockedNotice => "隐私说明",
        }
    }

    pub const fn label_en(self) -> &'static str {
        match self {
            Self::TimeRange => "Time range",
            Self::ActivityList => "Activities",
            Self::AppList => "Categories",
            Self::HighlightList => "Highlights",
            Self::ObservationCount => "Captured",
            Self::BlockedNotice => "Privacy",
        }
    }

    pub fn label(self, locale: SummaryLocale) -> &'static str {
        if locale.is_chinese() {
            self.label_zh()
        } else {
            self.label_en()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct FieldSpec {
    pub id: String,
    pub label: String,
    pub kind: FieldKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SummaryTemplate {
    pub id: String,
    pub name: String,
    pub fields: Vec<FieldSpec>,
}

impl SummaryTemplate {
    /// 解析并校验单个模板。**未知字段直接拒绝**，不静默丢弃 ——
    /// 静默丢弃的结果是用户以为加了字段，实际总结里根本没有。
    pub fn parse_yaml(text: &str) -> Result<Self, String> {
        let template: SummaryTemplate =
            serde_yaml::from_str(text).map_err(|error| format!("模板无法解析：{error}"))?;
        template.validate()?;
        Ok(template)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("模板缺少 id".to_string());
        }
        if self.fields.is_empty() {
            return Err(format!("模板 {} 的 fields 为空：产不出可用总结", self.id));
        }

        let mut seen: Vec<&str> = Vec::new();
        for field in &self.fields {
            if field.id.trim().is_empty() {
                return Err(format!("模板 {} 里有字段缺少 id", self.id));
            }
            if seen.contains(&field.id.as_str()) {
                return Err(format!("模板 {} 里字段 id `{}` 重复", self.id, field.id));
            }
            seen.push(&field.id);
        }
        Ok(())
    }

    /// 内置模板 id 的**唯一清单**：错误消息与解析分支都从这里取，
    /// 避免「加了模板但错误消息里没列出来」这种自相矛盾。
    pub const BUILTIN_IDS: &'static [&'static str] = &["work_stage", "work_stage_detailed"];

    /// 用户模板优先：给了非空 YAML 就用它（解析失败**直接报错**），否则按 id 取
    /// 内置模板。两条路径都不静默退回 —— 用户以为生效却拿到别的模板，是这里最坏的结果。
    pub fn resolve(id: &str, yaml: Option<&str>) -> Result<Self, String> {
        match yaml.map(str::trim).filter(|text| !text.is_empty()) {
            Some(text) => Self::parse_yaml(text),
            None => Self::from_id(id),
        }
    }

    /// 按 id 取内置模板。**未知 id 直接报错并列出可用 id** —— 静默退回默认模板
    /// 会让用户以为自己配的模板生效了，实际拿到的是默认模板的产出。
    pub fn from_id(id: &str) -> Result<Self, String> {
        match id {
            "work_stage" => Ok(Self::default_work_stage()),
            "work_stage_detailed" => Ok(Self::work_stage_detailed()),
            other => Err(format!(
                "未知的总结模板 id `{other}`，可用：{}",
                Self::BUILTIN_IDS.join("、")
            )),
        }
    }

    pub fn default_work_stage() -> Self {
        Self {
            id: "work_stage".to_string(),
            name: "工作阶段".to_string(),
            fields: vec![
                spec("time_range", FieldKind::TimeRange),
                spec("activities", FieldKind::ActivityList),
                spec("categories", FieldKind::AppList),
                spec("captured", FieldKind::ObservationCount),
            ],
        }
    }

    /// 更详细的工作阶段模板：多出「主要时间花在哪」与「隐私说明」两个字段。
    ///
    /// 与 `configs/templates/summaries.example.yaml` 里的 `work_stage_detailed`
    /// 保持同一份字段集合 —— 示例文件与内置注册表说的不是一回事，
    /// 用户照着示例配就会拿到另一个模板。
    pub fn work_stage_detailed() -> Self {
        Self {
            id: "work_stage_detailed".to_string(),
            name: "工作阶段（详细）".to_string(),
            fields: vec![
                spec("time_range", FieldKind::TimeRange),
                spec("activities", FieldKind::ActivityList),
                labeled("highlights", "主要时间花在哪", FieldKind::HighlightList),
                spec("categories", FieldKind::AppList),
                spec("captured", FieldKind::ObservationCount),
                labeled("privacy", "隐私说明", FieldKind::BlockedNotice),
            ],
        }
    }
}

fn spec(id: &str, kind: FieldKind) -> FieldSpec {
    FieldSpec {
        id: id.to_string(),
        label: kind.label_zh().to_string(),
        kind,
    }
}

fn labeled(id: &str, label: &str, kind: FieldKind) -> FieldSpec {
    FieldSpec {
        id: id.to_string(),
        label: label.to_string(),
        kind,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct TemplateFile {
    #[serde(default)]
    templates: Vec<SummaryTemplate>,
}

/// 模板集合。文件坏了要报错，不允许「回退到内置模板」——
/// 用户改模板却静默不生效，比报错难查得多。
#[derive(Debug, Clone, Default)]
pub struct TemplateSet {
    templates: Vec<SummaryTemplate>,
}

impl TemplateSet {
    pub fn parse_yaml(text: &str) -> Result<Self, String> {
        let file: TemplateFile =
            serde_yaml::from_str(text).map_err(|error| format!("模板文件无法解析：{error}"))?;
        if file.templates.is_empty() {
            return Err("模板文件里没有任何模板".to_string());
        }
        for template in &file.templates {
            template.validate()?;
        }

        let mut seen: Vec<&str> = Vec::new();
        for template in &file.templates {
            if seen.contains(&template.id.as_str()) {
                return Err(format!("模板 id `{}` 重复", template.id));
            }
            seen.push(&template.id);
        }
        Ok(Self {
            templates: file.templates,
        })
    }

    pub fn get(&self, id: &str) -> Option<&SummaryTemplate> {
        self.templates.iter().find(|template| template.id == id)
    }

    pub fn len(&self) -> usize {
        self.templates.len()
    }

    pub fn is_empty(&self) -> bool {
        self.templates.is_empty()
    }
}

/// 字段 → 渲染文本的注册表：渲染只在这里发生，避免各处拼字符串。
pub type FieldValues = BTreeMap<String, String>;
