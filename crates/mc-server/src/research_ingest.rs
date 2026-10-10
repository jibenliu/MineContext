//! Deep Research（轻量）：用户给出主题与若干公开 URL，抓取正文汇编成一篇研究笔记。
//!
//! 不做独立搜索引擎接入（需外部检索账号）；有 URL 即可在本地闭环完成。

use std::time::Duration;

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_pipeline::domains::DomainRules;
use mc_providers::transport::{HttpRequest, HttpTransport};
use mc_storage::vaults::{VaultUpsert, DOCUMENT_TYPE_VAULTS};
use mc_storage::Database;

use crate::link_ingest;

const MAX_URLS: usize = 8;
const MAX_TEXT_CHARS: usize = 200_000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchIngestResult {
    pub id: i64,
    pub title: String,
    pub sources: Vec<String>,
}

/// 抓取 URL 正文并汇编为一篇研究笔记（不落单条链接笔记）。
pub async fn ingest_research(
    db: &Database,
    transport: &dyn HttpTransport,
    blocked_domains: &[String],
    topic: &str,
    urls: &[String],
    parent_id: Option<i64>,
    at: Timestamp,
) -> Result<ResearchIngestResult, AppError> {
    let topic = topic.trim();
    if topic.is_empty() {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            "研究主题不能为空",
        ));
    }
    if urls.is_empty() {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            "至少提供一个公开 URL 作为研究来源",
        ));
    }
    if urls.len() > MAX_URLS {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("一次最多 {MAX_URLS} 个来源 URL"),
        ));
    }

    let rules = DomainRules::new(blocked_domains);
    let mut sections = Vec::new();
    let mut sources = Vec::new();

    for raw in urls {
        let (url, host) = link_ingest::validate_url(raw)?;
        if rules.is_blocked(Some(&url), "") {
            return Err(AppError::new(
                ErrorCode::PrivacyBlocked,
                format!("研究来源命中 privacy.blocked_domains：{host}"),
            ));
        }
        let html = fetch_html(transport, &url).await?;
        let page = link_ingest::extract_page(&html, &url);
        if page.text.trim().is_empty() {
            return Err(AppError::new(
                ErrorCode::ProviderInvalidResponse,
                format!("来源没有可提取的正文：{url}"),
            ));
        }
        sections.push(format!("## {}\n来源：{url}\n\n{}", page.title, page.text));
        sources.push(url);
    }

    let body = truncate_chars(&sections.join("\n\n---\n\n"), MAX_TEXT_CHARS);
    let title = truncate_chars(&format!("研究：{topic}"), 200);
    let summary: String = body.chars().take(240).collect();
    let content = format!("主题：{topic}\n\n{body}");
    let id = db.insert_vault_row(
        &VaultUpsert {
            title: title.clone(),
            summary,
            content,
            tags: vec!["research".to_string(), "deep-research".to_string()],
            parent_id,
            is_folder: false,
            document_type: DOCUMENT_TYPE_VAULTS.to_string(),
            sort_order: 0,
        },
        at,
    )?;

    Ok(ResearchIngestResult { id, title, sources })
}

async fn fetch_html(transport: &dyn HttpTransport, url: &str) -> Result<String, AppError> {
    let response = transport
        .send(HttpRequest {
            method: "GET".into(),
            url: url.to_string(),
            headers: vec![("Accept".into(), "text/html,application/xhtml+xml".into())],
            body: None,
            timeout: FETCH_TIMEOUT,
        })
        .await
        .map_err(|error| match error {
            mc_providers::transport::TransportError::Timeout => {
                AppError::new(ErrorCode::ProviderTimeout, "抓取研究来源超时")
            }
            mc_providers::transport::TransportError::Connect(detail) => AppError::new(
                ErrorCode::ProviderConnection,
                format!("无法连接研究来源：{detail}"),
            ),
            mc_providers::transport::TransportError::Other(detail) => AppError::new(
                ErrorCode::ProviderConnection,
                format!("抓取研究来源失败：{detail}"),
            ),
        })?;
    if !(200..300).contains(&response.status) {
        return Err(AppError::new(
            ErrorCode::ProviderServerError,
            format!("抓取研究来源失败：HTTP {}", response.status),
        ));
    }
    Ok(response.body)
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}
