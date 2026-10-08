//! HTTP 传输抽象。
//!
//! 抽出来的唯一目的：让 Provider 的全部错误映射与请求形状都能在
//! **没有网络、没有真实模型**的情况下确定性测试
//! （「I/O 可替换」是整条测试策略的前提）。

use std::time::Duration;

use async_trait::async_trait;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpRequest {
    pub method: String,
    pub url: String,
    /// 顺序敏感的键值对（Authorization 等）
    pub headers: Vec<(String, String)>,
    pub body: Option<String>,
    pub timeout: Duration,
}

impl HttpRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub body: String,
    /// 来自 `Retry-After` 响应头
    pub retry_after: Option<Duration>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportError {
    Timeout,
    Connect(String),
    Other(String),
}

impl std::fmt::Display for TransportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Timeout => write!(f, "请求超时"),
            Self::Connect(detail) => write!(f, "无法连接: {detail}"),
            Self::Other(detail) => write!(f, "传输失败: {detail}"),
        }
    }
}

impl std::error::Error for TransportError {}

impl TransportError {
    /// 稳定短标识。用于日志字段 —— 日志里不写 URL 与响应原文。
    pub const fn kind(&self) -> &'static str {
        match self {
            Self::Timeout => "timeout",
            Self::Connect(_) => "connect",
            Self::Other(_) => "transport",
        }
    }
}

#[async_trait]
pub trait HttpTransport: Send + Sync {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError>;

    /// 流式发送：把**原始响应片段**按到达顺序交给回调（SSE 帧的解析在 Provider 层）。
    ///
    /// 默认实现退化为一次 `send`：整段响应作为单个片段回调。
    /// 这样「支持流式」与「不支持流式」的传输可以共存 —— 后者不会因为
    /// 没实现这个方法而编译不过，但**首字延迟等于整段生成时间**。
    async fn send_stream(
        &self,
        request: HttpRequest,
        on_chunk: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<HttpResponse, TransportError> {
        let response = self.send(request).await?;
        // 先取出一份再回调：回调持有的是本次调用的借用，不能同时把 response 交出去
        let body = response.body.clone();
        on_chunk(&body);
        Ok(response)
    }
}

/// 真实实现：基于 reqwest。
pub struct ReqwestTransport {
    client: reqwest::Client,
}

impl ReqwestTransport {
    pub fn new() -> Result<Self, TransportError> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| TransportError::Other(e.to_string()))?;
        Ok(Self { client })
    }
}

impl Default for ReqwestTransport {
    fn default() -> Self {
        Self::new().unwrap_or_else(|_| Self {
            client: reqwest::Client::new(),
        })
    }
}

#[async_trait]
impl HttpTransport for ReqwestTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
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
            .map_err(|e| TransportError::Other(e.to_string()))?;

        let mut builder = self
            .client
            .request(method, &request.url)
            .headers(headers)
            .timeout(request.timeout);

        if let Some(body) = request.body {
            builder = builder.body(body);
        }

        let response = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                TransportError::Timeout
            } else if e.is_connect() {
                TransportError::Connect(e.to_string())
            } else {
                TransportError::Other(e.to_string())
            }
        })?;

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);

        let body = response
            .text()
            .await
            .map_err(|e| TransportError::Other(e.to_string()))?;

        Ok(HttpResponse {
            status,
            body,
            retry_after,
        })
    }

    /// 真实流式：用 reqwest 的分块读取边收边回调（不缓冲整段）。
    ///
    /// 传输层不做 SSE 解析 —— 只把收到的字节片段交出去，帧的切分与解析在 Provider 层，
    /// 这样「截断的帧」「一帧被切成两半」都能在测试里构造出来。
    async fn send_stream(
        &self,
        request: HttpRequest,
        on_chunk: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<HttpResponse, TransportError> {
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
            .map_err(|e| TransportError::Other(e.to_string()))?;

        let mut builder = self
            .client
            .request(method, &request.url)
            .headers(headers)
            .timeout(request.timeout);

        if let Some(body) = request.body {
            builder = builder.body(body);
        }

        let mut response = builder.send().await.map_err(|e| {
            if e.is_timeout() {
                TransportError::Timeout
            } else if e.is_connect() {
                TransportError::Connect(e.to_string())
            } else {
                TransportError::Other(e.to_string())
            }
        })?;

        let status = response.status().as_u16();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map(Duration::from_secs);

        let mut body = String::new();
        // 分块边界可能把一个多字节字符切成两半：把不完整的尾部留在缓冲区，
        // 只把**合法 UTF-8 前缀**交出去（否则增量里会混进替换字符）。
        let mut pending: Vec<u8> = Vec::new();
        loop {
            let chunk = response
                .chunk()
                .await
                .map_err(|e| TransportError::Other(e.to_string()))?;
            let Some(chunk) = chunk else { break };
            pending.extend_from_slice(&chunk);
            let valid_up_to = match std::str::from_utf8(&pending) {
                Ok(text) => text.len(),
                Err(error) => error.valid_up_to(),
            };
            if valid_up_to == 0 {
                continue;
            }
            let text = String::from_utf8_lossy(&pending[..valid_up_to]).into_owned();
            pending.drain(..valid_up_to);
            body.push_str(&text);
            on_chunk(&text);
        }
        if !pending.is_empty() {
            let text = String::from_utf8_lossy(&pending).into_owned();
            body.push_str(&text);
            on_chunk(&text);
        }

        Ok(HttpResponse {
            status,
            body,
            retry_after,
        })
    }
}
