//! 请求守卫：loopback Host 校验 + token 鉴权。

use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use mc_common::error::{AppError, ErrorCode};

use crate::envelope::error_response;
use crate::state::ServerState;

pub const TOKEN_HEADER: &str = "x-mc-token";

/// 无需 token 的路径：健康检查是启动握手用的，且只返回最小信息。
const PUBLIC_PATHS: &[&str] = &["/api/health", "/health"];

pub async fn guard(
    State(state): State<Arc<ServerState>>,
    request: Request,
    next: Next,
) -> Response {
    // 1) DNS rebinding 防护：浏览器发起的跨站请求会带上攻击者的 Host。
    //    缺少 Host 头（HTTP/1.0 风格或进程内调用）不视为攻击。
    if let Some(host) = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
    {
        if !is_loopback_host(host) {
            let error = AppError::new(
                ErrorCode::ConfigInvalid,
                format!("拒绝非本机 Host 头：{host}"),
            );
            return error_response(StatusCode::FORBIDDEN, &error);
        }
    }

    // 2) 鉴权
    let path = request.uri().path();
    if path.starts_with("/api") && !PUBLIC_PATHS.contains(&path) {
        let provided = request
            .headers()
            .get(TOKEN_HEADER)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();

        if !constant_time_eq(provided, &state.token) {
            let error = AppError::new(ErrorCode::ConfigInvalid, "缺少或错误的 X-MC-Token");
            return error_response(StatusCode::UNAUTHORIZED, &error);
        }
    }

    next.run(request).await
}

fn is_loopback_host(host: &str) -> bool {
    let without_port = match host.rfind(':') {
        // 只在「冒号后面是端口」时才剥离，避免误伤 IPv6 字面量
        Some(idx) if host[idx + 1..].chars().all(|c| c.is_ascii_digit()) => &host[..idx],
        _ => host,
    };
    matches!(without_port, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// 常量时间比较，避免通过响应时间逐字节猜 token。
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_are_accepted() {
        for host in [
            "127.0.0.1",
            "127.0.0.1:17331",
            "localhost",
            "localhost:8080",
            "[::1]:8080",
        ] {
            assert!(is_loopback_host(host), "{host} 应被接受");
        }
    }

    #[test]
    fn non_loopback_hosts_are_rejected() {
        for host in [
            "evil.example.com",
            "10.0.0.5:80",
            "0.0.0.0",
            "127.0.0.1.evil.com",
        ] {
            assert!(!is_loopback_host(host), "{host} 应被拒绝");
        }
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
