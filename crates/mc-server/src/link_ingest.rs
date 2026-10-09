//! 链接上传：把公开网页抓成笔记树文档，复用既有 vault → 检索 / 向量索引通路。
//!
//! 不做独立「浏览器采集源」：`SourceKind::Browser` 仍是预留的连续采集位；
//! 本路径是用户主动提交一条 URL，结果与手写笔记同形（`document_type=vaults`）。

use std::sync::Arc;
use std::time::Duration;

use mc_common::error::{AppError, ErrorCode};
use mc_common::time::Timestamp;
use mc_pipeline::domains::DomainRules;
use mc_providers::transport::{HttpRequest, HttpTransport, TransportError};
use mc_storage::vaults::{VaultUpsert, DOCUMENT_TYPE_VAULTS};
use mc_storage::Database;

/// 抓取响应体上限：再大的页面也只取前几兆，避免把控制面内存打爆。
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// 落库正文上限：检索与编辑器都吃得消的体量。
const MAX_TEXT_CHARS: usize = 200_000;
const FETCH_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedPage {
    pub title: String,
    pub text: String,
    pub summary: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkIngestResult {
    pub id: i64,
    pub title: String,
    pub url: String,
    pub source_host: String,
}

/// 规范化并校验用户提交的 URL：只允许 http(s)，拒绝回环与私网目标。
pub fn validate_url(raw: &str) -> Result<(String, String), AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::new(ErrorCode::DomainInvalidRange, "链接不能为空"));
    }

    let (scheme, rest) = trimmed.split_once("://").ok_or_else(|| {
        AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("链接必须是 http(s) URL，收到 {trimmed:?}"),
        )
    })?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("只支持 http/https，收到 scheme={scheme}"),
        ));
    }

    let host = host_of(trimmed).ok_or_else(|| {
        AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("无法解析链接主机名：{trimmed:?}"),
        )
    })?;

    if is_non_public_host(&host) {
        return Err(AppError::new(
            ErrorCode::DomainInvalidRange,
            format!("拒绝抓取本机或私网地址：{host}"),
        ));
    }

    // 去掉 fragment；保留 path/query。缺 path 时补 `/`，便于稳定去重展示。
    let without_fragment = trimmed.split('#').next().unwrap_or(trimmed);
    let normalized = if rest.contains('/') || rest.contains('?') {
        without_fragment.to_string()
    } else {
        format!("{without_fragment}/")
    };

    Ok((normalized, host))
}

pub fn extract_page(html: &str, source_url: &str) -> ExtractedPage {
    let without_noise = strip_noise_blocks(html);
    let title = extract_title(&without_noise)
        .map(|value| decode_entities(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| host_of(source_url).unwrap_or_else(|| source_url.to_string()));
    let text = truncate_chars(
        &collapse_ws(&decode_entities(&strip_tags(&without_noise))),
        MAX_TEXT_CHARS,
    );
    let summary: String = text.chars().take(240).collect();
    ExtractedPage {
        title: truncate_chars(&title, 200),
        text,
        summary,
    }
}

/// 抓取 → 抽取 → 写入 vaults。抓取失败时不落库。
pub async fn ingest_link(
    db: &Database,
    transport: &dyn HttpTransport,
    blocked_domains: &[String],
    url: &str,
    parent_id: Option<i64>,
    at: Timestamp,
) -> Result<LinkIngestResult, AppError> {
    let (url, host) = validate_url(url)?;
    let rules = DomainRules::new(blocked_domains);
    if rules.is_blocked(Some(&url), "") {
        return Err(AppError::new(
            ErrorCode::PrivacyBlocked,
            format!("链接主机命中 privacy.blocked_domains：{host}"),
        ));
    }

    let html = fetch_html(transport, &url).await?;
    let page = extract_page(&html, &url);
    if page.text.trim().is_empty() {
        return Err(AppError::new(
            ErrorCode::ProviderInvalidResponse,
            "页面没有可提取的正文",
        ));
    }

    let content = format!("来源：{url}\n\n{}", page.text);
    let tags = vec!["link".to_string(), host.clone()];
    let id = db.insert_vault_row(
        &VaultUpsert {
            title: page.title.clone(),
            summary: page.summary,
            content,
            tags,
            parent_id,
            is_folder: false,
            document_type: DOCUMENT_TYPE_VAULTS.to_string(),
            sort_order: 0,
        },
        at,
    )?;

    Ok(LinkIngestResult {
        id,
        title: page.title,
        url,
        source_host: host,
    })
}

/// 生产默认传输：不跟随重定向，避免跳到私网地址绕过主机校验。
pub fn default_link_transport() -> Result<Arc<dyn HttpTransport>, AppError> {
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(FETCH_TIMEOUT)
        .user_agent("MineContext-LinkUpload/1.0")
        .build()
        .map_err(|error| {
            AppError::new(
                ErrorCode::ProviderConnection,
                format!("无法创建链接抓取客户端：{error}"),
            )
        })?;
    Ok(Arc::new(ReqwestLinkTransport { client }))
}

struct ReqwestLinkTransport {
    client: reqwest::Client,
}

#[async_trait::async_trait]
impl HttpTransport for ReqwestLinkTransport {
    async fn send(
        &self,
        request: HttpRequest,
    ) -> Result<mc_providers::transport::HttpResponse, TransportError> {
        use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            if let (Ok(name), Ok(value)) = (
                HeaderName::from_bytes(name.as_bytes()),
                HeaderValue::from_str(value),
            ) {
                headers.insert(name, value);
            }
        }
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|error| TransportError::Other(error.to_string()))?;
        let mut builder = self
            .client
            .request(method, &request.url)
            .headers(headers)
            .timeout(request.timeout);
        if let Some(body) = &request.body {
            builder = builder.body(body.clone());
        }
        let response = builder.send().await.map_err(|error| {
            if error.is_timeout() {
                TransportError::Timeout
            } else if error.is_connect() {
                TransportError::Connect(error.to_string())
            } else {
                TransportError::Other(error.to_string())
            }
        })?;
        let status = response.status().as_u16();
        let body = response
            .text()
            .await
            .map_err(|error| TransportError::Other(error.to_string()))?;
        let body = if body.len() > MAX_BODY_BYTES {
            body.chars().take(MAX_BODY_BYTES).collect()
        } else {
            body
        };
        Ok(mc_providers::transport::HttpResponse {
            status,
            body,
            retry_after: None,
        })
    }
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
        .map_err(map_transport_error)?;

    if !(200..300).contains(&response.status) {
        return Err(AppError::new(
            ErrorCode::ProviderServerError,
            format!("抓取链接失败：HTTP {}", response.status),
        ));
    }
    Ok(response.body)
}

fn map_transport_error(error: TransportError) -> AppError {
    match error {
        TransportError::Timeout => AppError::new(ErrorCode::ProviderTimeout, "抓取链接超时"),
        TransportError::Connect(detail) => AppError::new(
            ErrorCode::ProviderConnection,
            format!("无法连接链接：{detail}"),
        ),
        TransportError::Other(detail) => AppError::new(
            ErrorCode::ProviderConnection,
            format!("抓取链接失败：{detail}"),
        ),
    }
}

fn host_of(url: &str) -> Option<String> {
    let rest = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);
    let authority = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default()
        .rsplit('@')
        .next()
        .unwrap_or_default();
    let host = if let Some(stripped) = authority.strip_prefix('[') {
        stripped.split(']').next().unwrap_or_default().to_string()
    } else {
        authority.split(':').next().unwrap_or_default().to_string()
    };
    let host = host.trim().trim_end_matches('.').to_lowercase();
    (!host.is_empty()).then_some(host)
}

fn is_non_public_host(host: &str) -> bool {
    if host == "localhost" || host.ends_with(".localhost") || host.ends_with(".local") {
        return true;
    }
    if host == "::1" || host == "0:0:0:0:0:0:0:1" {
        return true;
    }
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => {
                v4.is_loopback()
                    || v4.is_private()
                    || v4.is_link_local()
                    || v4.is_unspecified()
                    || v4.octets()[0] == 100 && (v4.octets()[1] & 0b1100_0000) == 0b0100_0000
            }
            std::net::IpAddr::V6(v6) => {
                v6.is_loopback() || v6.is_unspecified() || (v6.segments()[0] & 0xfe00) == 0xfc00
            }
        };
    }
    false
}

fn extract_title(html: &str) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    let start = lower.find("<title")?;
    let after = html.get(start + 6..)?;
    let after_gt = after.find('>')?;
    let content = after.get(after_gt + 1..)?;
    let end_rel = content.to_ascii_lowercase().find("</title>")?;
    Some(content[..end_rel].trim().to_string())
}

fn strip_noise_blocks(html: &str) -> String {
    let mut out = html.to_string();
    for tag in ["script", "style", "noscript"] {
        out = strip_tag_blocks(&out, tag);
    }
    out
}

fn strip_tag_blocks(html: &str, tag: &str) -> String {
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut rest = html;
    let mut out = String::with_capacity(html.len());
    loop {
        let lower = rest.to_ascii_lowercase();
        let Some(start) = lower.find(&open) else {
            out.push_str(rest);
            break;
        };
        out.push_str(&rest[..start]);
        let after_open = &rest[start..];
        let lower_after = after_open.to_ascii_lowercase();
        let Some(close_at) = lower_after.find(&close) else {
            break;
        };
        rest = &after_open[close_at + close.len()..];
    }
    out
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
    fn validate_url_accepts_https_and_normalizes_bare_host() {
        let (url, host) = validate_url("https://Example.COM").unwrap();
        assert_eq!(url, "https://Example.COM/");
        assert_eq!(host, "example.com");
    }

    #[test]
    fn validate_url_rejects_file_and_loopback() {
        assert!(validate_url("file:///tmp/x").is_err());
        assert!(validate_url("http://127.0.0.1/").is_err());
        assert!(validate_url("https://192.168.1.1/").is_err());
    }

    #[test]
    fn extract_page_strips_script_and_reads_title() {
        let page = extract_page(
            r#"<html><head><title>Hello &amp; Co</title></head>
               <body><script>bad()</script><p>Visible   text</p></body></html>"#,
            "https://example.com/a",
        );
        assert_eq!(page.title, "Hello & Co");
        assert!(page.text.contains("Visible text"));
        assert!(!page.text.contains("bad()"));
    }
}
