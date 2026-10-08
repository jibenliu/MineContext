//! 脚本化的 HTTP 传输替身。
//!
//! 让 Provider 的全部错误映射、请求形状、能力探测都能在**没有网络、
//! 没有真实模型**的情况下确定性测试。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use mc_common::lock::lock_or_recover;
use mc_providers::transport::{HttpRequest, HttpResponse, HttpTransport, TransportError};

/// 一次被记录的请求。
pub type RecordedRequest = HttpRequest;

#[derive(Clone)]
enum Reply {
    Response {
        status: u16,
        body: String,
        retry_after: Option<Duration>,
    },
    /// 流式响应：片段按顺序到达（模拟 SSE 逐帧）。
    Stream {
        status: u16,
        chunks: Vec<String>,
    },
    Failure(TransportError),
}

type DynamicReply = Box<dyn Fn(&HttpRequest) -> String + Send + Sync>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn poisoned_transport_preserves_queued_replies_and_records_requests() {
        let transport = Arc::new(
            ScriptedTransport::new()
                .push_json(201, "first")
                .push_sse(200, vec!["second".into()]),
        );
        let shared = Arc::clone(&transport);
        assert!(std::thread::spawn(move || {
            let _requests = shared.recorded.lock().unwrap();
            let _replies = shared.replies.lock().unwrap();
            panic!("poison transport");
        })
        .join()
        .is_err());
        let request = HttpRequest {
            method: "POST".into(),
            url: "http://localhost/test".into(),
            headers: Vec::new(),
            body: None,
            timeout: Duration::from_secs(1),
        };
        assert_eq!(transport.send(request.clone()).await.unwrap().body, "first");
        let mut received = String::new();
        transport
            .send_stream(request.clone(), &mut |chunk| received.push_str(chunk))
            .await
            .unwrap();
        assert_eq!(received, "second");
        assert_eq!(transport.call_count(), 2);
        assert_eq!(transport.recorded(), vec![request.clone(), request.clone()]);
        assert_eq!(transport.last(), request);
        let transport = Arc::try_unwrap(transport)
            .ok()
            .unwrap()
            .push_json(200, "third")
            .push_json_with_retry_after(429, "limited", Duration::from_secs(2))
            .push_sse(200, vec!["fourth".into()])
            .push_failure(TransportError::Timeout);
        let request = transport.last();
        assert_eq!(transport.send(request.clone()).await.unwrap().body, "third");
        assert_eq!(
            transport.send(request.clone()).await.unwrap().retry_after,
            Some(Duration::from_secs(2))
        );
        assert_eq!(
            transport.send(request.clone()).await.unwrap().body,
            "fourth"
        );
        assert_eq!(transport.send(request).await, Err(TransportError::Timeout));
    }
}

#[derive(Default)]
pub struct ScriptedTransport {
    replies: Mutex<Vec<Reply>>,
    recorded: Mutex<Vec<RecordedRequest>>,
    /// 按请求动态生成响应（需要「第几次调用返回什么」或「调用时触发副作用」时用）
    dynamic: Option<Arc<DynamicReply>>,
}

impl ScriptedTransport {
    pub fn new() -> Self {
        Self::default()
    }

    /// 依次入队返回值；用完后默认返回 200 `{}`。
    pub fn push_json(self, status: u16, body: impl Into<String>) -> Self {
        lock_or_recover(&self.replies).push(Reply::Response {
            status,
            body: body.into(),
            retry_after: None,
        });
        self
    }

    pub fn push_json_with_retry_after(
        self,
        status: u16,
        body: impl Into<String>,
        retry_after: Duration,
    ) -> Self {
        lock_or_recover(&self.replies).push(Reply::Response {
            status,
            body: body.into(),
            retry_after: Some(retry_after),
        });
        self
    }

    /// 用一个函数生成响应：可以数调用次数，也可以在响应时触发副作用
    /// （例如「第一块做完就取消」）。优先级高于 `push_*` 队列。
    pub fn reply_with(
        mut self,
        reply: impl Fn(&HttpRequest) -> String + Send + Sync + 'static,
    ) -> Self {
        self.dynamic = Some(Arc::new(Box::new(reply)));
        self
    }

    /// 入队一个流式响应：片段按顺序交给 `send_stream` 的回调。
    pub fn push_sse(self, status: u16, chunks: Vec<String>) -> Self {
        lock_or_recover(&self.replies).push(Reply::Stream { status, chunks });
        self
    }

    pub fn push_failure(self, error: TransportError) -> Self {
        lock_or_recover(&self.replies).push(Reply::Failure(error));
        self
    }

    pub fn recorded(&self) -> Vec<RecordedRequest> {
        lock_or_recover(&self.recorded).clone()
    }

    pub fn call_count(&self) -> usize {
        lock_or_recover(&self.recorded).len()
    }

    pub fn last(&self) -> RecordedRequest {
        lock_or_recover(&self.recorded)
            .last()
            .cloned()
            .expect("应当至少发过一次请求")
    }
}

#[async_trait::async_trait]
impl HttpTransport for ScriptedTransport {
    async fn send(&self, request: HttpRequest) -> Result<HttpResponse, TransportError> {
        lock_or_recover(&self.recorded).push(request.clone());

        // 动态回应优先：需要「第几次调用返回什么」或「调用时触发副作用」时用它
        if let Some(dynamic) = &self.dynamic {
            return Ok(HttpResponse {
                status: 200,
                body: dynamic(&request),
                retry_after: None,
            });
        }

        let reply = {
            let mut replies = lock_or_recover(&self.replies);
            if replies.is_empty() {
                None
            } else {
                Some(replies.remove(0))
            }
        };

        match reply {
            Some(Reply::Response {
                status,
                body,
                retry_after,
            }) => Ok(HttpResponse {
                status,
                body,
                retry_after,
            }),
            Some(Reply::Stream { status, chunks }) => Ok(HttpResponse {
                status,
                // 非流式调用拿到的是拼起来的整体（顺序与流式一致）
                body: chunks.concat(),
                retry_after: None,
            }),
            Some(Reply::Failure(error)) => Err(error),
            None => Ok(HttpResponse {
                status: 200,
                body: "{}".to_string(),
                retry_after: None,
            }),
        }
    }

    async fn send_stream(
        &self,
        request: HttpRequest,
        on_chunk: &mut (dyn for<'a> FnMut(&'a str) + Send),
    ) -> Result<HttpResponse, TransportError> {
        lock_or_recover(&self.recorded).push(request.clone());

        let reply = {
            let mut replies = lock_or_recover(&self.replies);
            if replies.is_empty() {
                None
            } else {
                Some(replies.remove(0))
            }
        };

        match reply {
            Some(Reply::Stream { status, chunks }) => {
                let mut body = String::new();
                for chunk in &chunks {
                    on_chunk(chunk);
                    body.push_str(chunk);
                }
                Ok(HttpResponse {
                    status,
                    body,
                    retry_after: None,
                })
            }
            Some(Reply::Response {
                status,
                body,
                retry_after,
            }) => {
                on_chunk(&body);
                Ok(HttpResponse {
                    status,
                    body,
                    retry_after,
                })
            }
            Some(Reply::Failure(error)) => Err(error),
            None => {
                on_chunk("{}");
                Ok(HttpResponse {
                    status: 200,
                    body: "{}".to_string(),
                    retry_after: None,
                })
            }
        }
    }
}
