//! RSS / Atom 订阅导入：抓取公开 feed，把条目写成笔记树文档。
//!
//! 复用链接上传的 URL 校验与 HTTP 传输；不做持续轮询——用户主动提交一次 feed URL。

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_pipeline::domains::DomainRules;
use mc_providers::transport::{HttpRequest, HttpTransport};
use mc_storage::vaults::{VaultUpsert, DOCUMENT_TYPE_VAULTS};
use mc_storage::Database;
use std::time::Duration;

use crate::link_ingest;

const FETCH_TIMEOUT: Duration = Duration::from_secs(15);
const MAX_ITEMS_DEFAULT: usize = 20;
const MAX_ITEMS_CAP: usize = 50;
const MAX_TEXT_CHARS: usize = 200_000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RssItemNote {
    pub id: i64,
    pub title: String,
    pub link: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RssIngestResult {
    pub feed_title: String,
    pub feed_url: String,
    pub source_host: String,
    pub imported: Vec<RssItemNote>,
    pub skipped: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct FeedItem {
    title: String,
    link: String,
    summary: String,
}

/// 抓取 RSS/Atom → 为条目写入 vaults。空 feed 返回结构化错误。
pub async fn ingest_rss(
    db: &Database,
    transport: &dyn HttpTransport,
    blocked_domains: &[String],
    feed_url: &str,
    parent_id: Option<i64>,
    limit: Option<usize>,
    at: Timestamp,
) -> Result<RssIngestResult, AppError> {
    let (feed_url, host) = link_ingest::validate_url(feed_url)?;
    let rules = DomainRules::new(blocked_domains);
    if rules.is_blocked(Some(&feed_url), "") {
        return Err(AppError::new(
            ErrorCode::PrivacyBlocked,
            format!("订阅源主机命中 privacy.blocked_domains：{host}"),
        ));
    }

    let body = fetch_feed(transport, &feed_url).await?;
    let feed_title = extract_feed_title(&body).unwrap_or_else(|| host.clone());
    let mut items = parse_feed_items(&body);
    if items.is_empty() {
        return Err(AppError::new(
            ErrorCode::ProviderInvalidResponse,
            "订阅源没有可导入的条目",
        ));
    }

    let limit = limit.unwrap_or(MAX_ITEMS_DEFAULT).clamp(1, MAX_ITEMS_CAP);
    let skipped = items.len().saturating_sub(limit);
    items.truncate(limit);

    let mut imported = Vec::with_capacity(items.len());
    for item in items {
        let title = if item.title.trim().is_empty() {
            item.link.clone()
        } else {
            truncate_chars(&item.title, 200)
        };
        let body_text = if item.summary.trim().is_empty() {
            format!("来源：{}\n\n（条目无摘要）", item.link)
        } else {
            format!(
                "来源：{}\n\n{}",
                item.link,
                truncate_chars(&item.summary, MAX_TEXT_CHARS)
            )
        };
        let summary: String = item.summary.chars().take(240).collect();
        let id = db.insert_vault_row(
            &VaultUpsert {
                title: title.clone(),
                summary,
                content: body_text,
                tags: vec!["rss".to_string(), host.clone()],
                parent_id,
                is_folder: false,
                document_type: DOCUMENT_TYPE_VAULTS.to_string(),
                sort_order: 0,
            },
            at,
        )?;
        imported.push(RssItemNote {
            id,
            title,
            link: item.link,
        });
    }

    Ok(RssIngestResult {
        feed_title: truncate_chars(&feed_title, 200),
        feed_url,
        source_host: host,
        imported,
        skipped,
    })
}

async fn fetch_feed(transport: &dyn HttpTransport, url: &str) -> Result<String, AppError> {
    let response = transport
        .send(HttpRequest {
            method: "GET".into(),
            url: url.to_string(),
            headers: vec![(
                "Accept".into(),
                "application/rss+xml, application/atom+xml, application/xml, text/xml".into(),
            )],
            body: None,
            timeout: FETCH_TIMEOUT,
        })
        .await
        .map_err(|error| match error {
            mc_providers::transport::TransportError::Timeout => {
                AppError::new(ErrorCode::ProviderTimeout, "抓取订阅源超时")
            }
            mc_providers::transport::TransportError::Connect(detail) => AppError::new(
                ErrorCode::ProviderConnection,
                format!("无法连接订阅源：{detail}"),
            ),
            mc_providers::transport::TransportError::Other(detail) => AppError::new(
                ErrorCode::ProviderConnection,
                format!("抓取订阅源失败：{detail}"),
            ),
        })?;

    if !(200..300).contains(&response.status) {
        return Err(AppError::new(
            ErrorCode::ProviderServerError,
            format!("抓取订阅源失败：HTTP {}", response.status),
        ));
    }
    Ok(response.body)
}

fn extract_feed_title(xml: &str) -> Option<String> {
    first_tag_text(xml, "title")
}

fn parse_feed_items(xml: &str) -> Vec<FeedItem> {
    let mut items = Vec::new();
    for block in iter_blocks(xml, "item").chain(iter_blocks(xml, "entry")) {
        let title = first_tag_text(&block, "title").unwrap_or_default();
        let link = first_tag_text(&block, "link")
            .or_else(|| atom_link_href(&block))
            .unwrap_or_default();
        let summary = first_tag_text(&block, "description")
            .or_else(|| first_tag_text(&block, "summary"))
            .or_else(|| first_tag_text(&block, "content"))
            .unwrap_or_default();
        let summary = strip_tags(&summary);
        if title.trim().is_empty() && link.trim().is_empty() && summary.trim().is_empty() {
            continue;
        }
        items.push(FeedItem {
            title: decode_entities(&title),
            link: decode_entities(&link),
            summary: collapse_ws(&decode_entities(&summary)),
        });
    }
    items
}

fn iter_blocks<'a>(xml: &'a str, tag: &'a str) -> impl Iterator<Item = String> + 'a {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let lower = xml.to_ascii_lowercase();
    let open_l = open.to_ascii_lowercase();
    let close_l = close.to_ascii_lowercase();
    let mut start = 0usize;
    let mut out = Vec::new();
    while let Some(rel) = lower[start..].find(&open_l) {
        let abs = start + rel;
        let Some(gt) = xml[abs..].find('>') else {
            break;
        };
        let content_start = abs + gt + 1;
        let Some(end_rel) = lower[content_start..].find(&close_l) else {
            break;
        };
        let content_end = content_start + end_rel;
        out.push(xml[content_start..content_end].to_string());
        start = content_end + close.len();
    }
    out.into_iter()
}

fn first_tag_text(xml: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let lower = xml.to_ascii_lowercase();
    let open_l = open.to_ascii_lowercase();
    let close_l = close.to_ascii_lowercase();
    let start = lower.find(&open_l)?;
    let after_open = &xml[start..];
    let gt = after_open.find('>')?;
    let content = &after_open[gt + 1..];
    let end = content.to_ascii_lowercase().find(&close_l)?;
    Some(content[..end].trim().to_string())
}

fn atom_link_href(block: &str) -> Option<String> {
    let lower = block.to_ascii_lowercase();
    let mut search = 0usize;
    while let Some(rel) = lower[search..].find("<link") {
        let abs = search + rel;
        let slice = &block[abs..];
        let gt = slice.find('>')?;
        let tag = &slice[..=gt];
        if let Some(href) = attr_value(tag, "href") {
            return Some(href);
        }
        search = abs + gt + 1;
    }
    None
}

fn attr_value(tag: &str, name: &str) -> Option<String> {
    let needle = format!("{name}=\"");
    let lower = tag.to_ascii_lowercase();
    let needle_l = needle.to_ascii_lowercase();
    let start = lower.find(&needle_l)? + needle.len();
    let rest = &tag[start..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn strip_tags(html: &str) -> String {
    let mut out = String::with_capacity(html.len());
    let mut in_tag = false;
    for ch in html.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    out
}

fn decode_entities(text: &str) -> String {
    text.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_space {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    text.chars().take(max).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_rss_items_reads_title_link_description() {
        let xml = r#"<?xml version="1.0"?>
<rss><channel>
<title>Dev Digest</title>
<item>
  <title>Rust Ownership</title>
  <link>https://example.com/a</link>
  <description>Borrow checker basics</description>
</item>
<item>
  <title>RSS Ingest</title>
  <link>https://example.com/b</link>
  <description>Feed to vault</description>
</item>
</channel></rss>"#;
        let items = parse_feed_items(xml);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Rust Ownership");
        assert_eq!(items[0].link, "https://example.com/a");
        assert!(items[0].summary.contains("Borrow checker"));
        assert_eq!(extract_feed_title(xml).as_deref(), Some("Dev Digest"));
    }

    #[test]
    fn parse_atom_entry_uses_href() {
        let xml = r#"<?xml version="1.0"?>
<feed>
<title>Atom Feed</title>
<entry>
  <title>Entry One</title>
  <link href="https://example.com/atom-1"/>
  <summary>Hello atom</summary>
</entry>
</feed>"#;
        let items = parse_feed_items(xml);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].link, "https://example.com/atom-1");
        assert!(items[0].summary.contains("Hello atom"));
    }
}
