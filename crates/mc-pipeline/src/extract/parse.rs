//! 结构化输出的解析与修复。
//!
//! 模型返回的从来不是可靠 JSON。这里实现四级降级链的前四级
//! （第五级「带错误反馈重试」在 `mod.rs`，因为它需要再发一次请求）：
//! ① 直接解析 → ② 剥离 markdown 围栏 → ③ 从废话中抽出第一个配平的对象
//! → ④ 修复尾随逗号 / 补全被截断的括号。
//!
//! 每一步都只在前一步失败时才尝试，且**全部是纯函数**：不需要网络与模型。

use serde::{Deserialize, Serialize};

/// 提示词要求的最大对象数。模型可能不遵守，因此这里也要截断。
const MAX_OBJECTS: usize = 8;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct StructuredUnderstanding {
    #[serde(default)]
    pub app: Option<String>,
    #[serde(default)]
    pub page: Option<String>,
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub issue: Option<String>,
    #[serde(default)]
    pub action: Option<String>,
    #[serde(default)]
    pub objects: Vec<String>,
    #[serde(default)]
    pub text_summary: Option<String>,
    #[serde(default)]
    pub confidence: f32,
}

/// 解析入口。返回 `Err(可读原因)` 而不是 panic。
pub fn parse(raw: &str) -> Result<StructuredUnderstanding, String> {
    let mut last_error = String::from("输入为空");

    for candidate in candidates(raw) {
        match serde_json::from_str::<serde_json::Value>(&candidate) {
            Ok(value) => match from_value(value) {
                Ok(understanding) => return Ok(understanding),
                Err(reason) => last_error = reason,
            },
            Err(error) => last_error = error.to_string(),
        }
    }

    Err(last_error)
}

/// 复用同一条修复链，取出一个 JSON 对象。
///
/// 活动推断只要一个对象，不需要 `StructuredUnderstanding` 的字段约束，
/// 但**必须共用同一套修复策略** —— 各写一份的结果是两边的解析率不一样，
/// 排查时根本无法比较。
pub fn json_object(raw: &str) -> Option<serde_json::Value> {
    for candidate in candidates(raw) {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(&candidate) {
            if value.is_object() {
                return Some(value);
            }
        }
    }
    None
}

/// 依次更宽松的候选文本。顺序很重要：先试最保守的，避免把正常 JSON 改坏。
fn candidates(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    let mut out = Vec::new();

    fn push(out: &mut Vec<String>, text: String) {
        if !text.trim().is_empty() && !out.contains(&text) {
            out.push(text);
        }
    }

    push(&mut out, trimmed.to_string());

    let unfenced = strip_code_fence(trimmed);
    push(&mut out, unfenced.clone());

    // 前后废话里可能夹着别的花括号（反而是模型真正意图之前的噪声），
    // 因此要把**所有**配平的对象都作为候选，而不是只试第一个。
    for inner in balanced_objects(&unfenced) {
        push(&mut out, inner.to_string());
        push(&mut out, remove_trailing_commas(inner));
    }
    for inner in balanced_objects(trimmed) {
        push(&mut out, inner.to_string());
        push(&mut out, remove_trailing_commas(inner));
    }

    // 被 max_tokens 截断：从第一个 `{` 开始，把没闭合的括号补齐
    push(&mut out, close_unbalanced(&unfenced));
    push(&mut out, close_unbalanced(trimmed));

    // 最后再对候选做一次尾随逗号清理
    let with_repaired: Vec<String> = out.iter().map(|c| remove_trailing_commas(c)).collect();
    for candidate in with_repaired {
        push(&mut out, candidate);
    }

    out
}

fn from_value(value: serde_json::Value) -> Result<StructuredUnderstanding, String> {
    if !value.is_object() {
        return Err(format!("期望 JSON 对象，实际是 {}", kind_of(&value)));
    }

    let mut understanding: StructuredUnderstanding =
        serde_json::from_value(value).map_err(|e| format!("字段类型不符: {e}"))?;

    if !(0.0..=1.0).contains(&understanding.confidence) {
        return Err(format!(
            "confidence 必须在 0..1 之间，实际 {}",
            understanding.confidence
        ));
    }

    // 提示词写了「最多 8 个」，但模型经常不遵守；截断而不是丢弃整条结果
    understanding.objects.truncate(MAX_OBJECTS);

    // 去掉空字符串项，避免下游把 "" 当成有效实体
    understanding.objects.retain(|o| !o.trim().is_empty());

    Ok(understanding)
}

fn kind_of(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "布尔值",
        serde_json::Value::Number(_) => "数字",
        serde_json::Value::String(_) => "字符串",
        serde_json::Value::Array(_) => "数组",
        serde_json::Value::Object(_) => "对象",
    }
}

/// 剥离 ```` ```json ... ``` ```` 围栏。没有围栏时原样返回。
pub fn strip_code_fence(text: &str) -> String {
    let trimmed = text.trim();
    if !trimmed.starts_with("```") {
        return trimmed.to_string();
    }

    // 去掉第一行（``` 或 ```json）
    let after_open = match trimmed.find('\n') {
        Some(index) => &trimmed[index + 1..],
        None => return String::new(),
    };

    match after_open.rfind("```") {
        Some(index) => after_open[..index].trim().to_string(),
        None => after_open.trim().to_string(),
    }
}

/// 找出文本中**所有**顶层配平的 `{...}`（按出现顺序）。
///
/// 之所以返回全部而不是第一个：模型常在真正的 JSON 之前先写一句带花括号的
/// 说明（例如 `注意 {这里的} 不是 JSON`），只试第一个会永远解析不出来。
/// 会跳过字符串内部的括号与转义。
pub fn balanced_objects(text: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut search_from = 0usize;

    while let Some(relative) = text[search_from..].find('{') {
        let start = search_from + relative;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escaped = false;
        let mut end = None;

        for (offset, ch) in text[start..].char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }

            match ch {
                '"' => in_string = true,
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(start + offset + ch.len_utf8());
                        break;
                    }
                }
                _ => {}
            }
        }

        match end {
            Some(end) => {
                out.push(&text[start..end]);
                search_from = end;
            }
            // 这个 `{` 之后再没有闭合 → 后面的都补不齐，交给 close_unbalanced
            None => break,
        }
    }

    out
}

/// 第一个配平的对象（便捷方法）。
pub fn first_balanced_object(text: &str) -> Option<&str> {
    balanced_objects(text).into_iter().next()
}

/// 去掉 `,}` 与 `,]` 形态的尾随逗号（同样跳过字符串内部）。
pub fn remove_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_string = false;
    let mut escaped = false;
    let chars: Vec<char> = text.chars().collect();

    for (index, ch) in chars.iter().enumerate() {
        if in_string {
            out.push(*ch);
            if escaped {
                escaped = false;
            } else if *ch == '\\' {
                escaped = true;
            } else if *ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => {
                in_string = true;
                out.push(*ch);
            }
            ',' => {
                // 向后看第一个非空白字符
                let next = chars[index + 1..].iter().find(|c| !c.is_whitespace());
                if matches!(next, Some('}') | Some(']')) {
                    // 丢弃这个逗号
                } else {
                    out.push(*ch);
                }
            }
            _ => out.push(*ch),
        }
    }

    out
}

/// 补全未闭合的字符串、数组与对象（应对 `max_tokens` 截断）。
pub fn close_unbalanced(text: &str) -> String {
    let trimmed = text.trim();
    let Some(start) = trimmed.find('{') else {
        return trimmed.to_string();
    };

    let body = &trimmed[start..];
    let mut stack: Vec<char> = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for ch in body.chars() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => stack.push('}'),
            '[' => stack.push(']'),
            '}' | ']' => {
                stack.pop();
            }
            _ => {}
        }
    }

    let mut repaired = body.to_string();

    if in_string {
        // 截断在字符串中间：先补引号，再补括号
        repaired.push('"');
    }

    // 去掉因截断产生的悬空逗号
    repaired = remove_trailing_commas(&repaired);
    let trimmed_repaired = repaired.trim_end();
    if trimmed_repaired.ends_with(',') || trimmed_repaired.ends_with(':') {
        repaired = trimmed_repaired.trim_end_matches([',', ':']).to_string();
    }

    while let Some(close) = stack.pop() {
        repaired.push(close);
    }

    repaired
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_fence_handles_missing_newline() {
        assert_eq!(strip_code_fence("```"), "");
    }

    #[test]
    fn balanced_object_ignores_braces_inside_strings() {
        let text = r#"{"text":"a } b","n":1}"#;
        assert_eq!(first_balanced_object(text), Some(text));
    }

    #[test]
    fn balanced_object_handles_escaped_quote() {
        let text = r#"{"text":"say \"hi\"","n":1}"#;
        assert_eq!(first_balanced_object(text), Some(text));
    }

    #[test]
    fn balanced_objects_returns_all_top_level_objects() {
        let text = r#"注意 {这不是 json} 然后 {"app":"Chrome"} 结束 {"x":1}"#;
        let found = balanced_objects(text);
        assert_eq!(found.len(), 3, "应当返回全部三个配平对象");
        assert_eq!(found[1], r#"{"app":"Chrome"}"#);
    }

    #[test]
    fn trailing_comma_removal_keeps_strings_intact() {
        assert_eq!(
            remove_trailing_commas(r#"{"a":"x,","b":1,}"#),
            r#"{"a":"x,","b":1}"#
        );
    }

    #[test]
    fn close_unbalanced_appends_missing_brackets() {
        assert_eq!(close_unbalanced(r#"{"a":[1,2"#), r#"{"a":[1,2]}"#);
    }
}
