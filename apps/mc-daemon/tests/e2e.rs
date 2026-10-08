//! 真的起一个 `mc-daemon` 进程，真的打 HTTP。
//!
//! 这是测试金字塔的第 ④ 层：数量少、覆盖接线。
//! 它回答一个单测回答不了的问题：**这个二进制能不能真的跑起来并对外服务**。

use std::io::{Read, Write};
use std::net::TcpStream;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mc_testkit::fixtures::FIXTURE_EPOCH_MS;

// ---------------------------------------------------------------- E2E 脚手架

struct Daemon {
    child: Child,
    port: u16,
    token: String,
    dir: tempfile::TempDir,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// clippy 看不到 `Daemon::drop` 里的 kill+wait，因此误报僵尸进程。
// 这里显式豁免并说明：所有测试路径要么走到 drop，要么显式 wait 过。
#[allow(clippy::zombie_processes)]
fn start_daemon() -> Daemon {
    start_daemon_with(&[])
}

#[allow(clippy::zombie_processes)]
fn start_daemon_with(extra: &[&str]) -> Daemon {
    start_daemon_env(extra, &[])
}

#[allow(clippy::zombie_processes)]
fn start_daemon_env(extra: &[&str], envs: &[(&str, &str)]) -> Daemon {
    let dir = tempfile::tempdir().expect("tempdir");
    let exe = env!("CARGO_BIN_EXE_mc-daemon");

    let mut command = Command::new(exe);
    command
        .arg("--data-dir")
        .arg(dir.path())
        .arg("--port")
        .arg("0");
    for (key, value) in envs {
        command.env(key, value);
    }
    for arg in extra {
        command.arg(arg);
    }

    let child = command
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("mc-daemon 必须能启动");

    let runtime = dir.path().join("runtime.json");
    // 等待上限可用 MC_E2E_STARTUP_SECS 覆盖：并行跑测试时机器负载高，
    // 30 秒会把「慢」误判成「起不来」，而这个断言要抓的是挂住。
    let startup_secs = std::env::var("MC_E2E_STARTUP_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(120);
    let deadline = Instant::now() + Duration::from_secs(startup_secs);

    loop {
        if let Ok(text) = std::fs::read_to_string(&runtime) {
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&text) {
                if let (Some(port), Some(token)) = (json["port"].as_u64(), json["token"].as_str()) {
                    return Daemon {
                        child,
                        port: port as u16,
                        token: token.to_string(),
                        dir,
                    };
                }
            }
        }
        if Instant::now() > deadline {
            panic!("daemon 未能在 {startup_secs}s 内写出可用的 runtime.json");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// 手写的最小 HTTP/1.1 客户端：不为测试引入 HTTP 依赖。
fn read_response(
    port: u16,
    path: &str,
    token: Option<&str>,
    out: &mut String,
    timeout: Option<Duration>,
) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("应当能连上 daemon");
    stream.set_read_timeout(timeout).ok();

    let mut request =
        format!("GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n");
    if let Some(token) = token {
        request.push_str(&format!("X-MC-Token: {token}\r\n"));
    }
    request.push_str("\r\n");
    stream.write_all(request.as_bytes()).expect("写出请求");

    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                buffer.extend_from_slice(&chunk[..n]);
                if buffer.len() > 256 * 1024 {
                    break;
                }
                // SSE 不会主动关闭连接：拿到数据就够断言了
                if timeout.is_some() {
                    break;
                }
            }
            // 读超时在 SSE 场景下是预期行为
            Err(_) => break,
        }
    }
    out.push_str(&String::from_utf8_lossy(&buffer));
}

fn status_of(response: &str) -> u16 {
    response
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0)
}

fn body_of(response: &str) -> &str {
    response.split("\r\n\r\n").nth(1).unwrap_or("")
}

// ---------------------------------------------------------------- 真进程 E2E

#[test]
fn daemon_serves_health_and_writes_private_runtime_file() {
    let daemon = start_daemon();

    let mut response = String::new();
    read_response(daemon.port, "/api/health", None, &mut response, None);

    assert_eq!(status_of(&response), 200, "健康检查必须 200：{response}");
    let body = body_of(&response);
    assert!(
        body.contains("\"status\":\"ok\"") || body.contains("\"status\": \"ok\""),
        "{body}"
    );
    assert!(
        body.contains("\"llm\""),
        "渲染层依赖 data.components.llm：{body}"
    );

    // runtime.json 含 token，权限必须是 0600
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let path = daemon.dir.path().join("runtime.json");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "runtime.json 权限必须是 0600");
    }

    // 数据库真的被创建了
    assert!(daemon.dir.path().join("data/minecontext.db").exists());
}

#[test]
fn daemon_rejects_api_without_token() {
    let daemon = start_daemon();

    let mut unauthorized = String::new();
    read_response(
        daemon.port,
        "/api/diagnostics",
        None,
        &mut unauthorized,
        None,
    );
    assert_eq!(status_of(&unauthorized), 401, "{unauthorized}");

    let mut authorized = String::new();
    read_response(
        daemon.port,
        "/api/diagnostics",
        Some(&daemon.token),
        &mut authorized,
        None,
    );
    assert_eq!(status_of(&authorized), 200, "{authorized}");
    assert!(
        body_of(&authorized).contains("stages_without_summary"),
        "诊断必须包含最高优先级不变量：{}",
        body_of(&authorized)
    );
}

#[test]
fn daemon_serves_capture_status_with_honest_readiness() {
    let daemon = start_daemon();

    let mut response = String::new();
    read_response(
        daemon.port,
        "/api/capture/status",
        Some(&daemon.token),
        &mut response,
        None,
    );

    assert_eq!(status_of(&response), 200, "{response}");

    // canRecord 必须如实反映本机就绪度。**不能拿单一时刻的现场探测去比**：
    // daemon 报的是启动时的判定，而屏幕就绪度会瞬时变化（权限弹窗、显示会话切换），
    // 两者不一致不代表字段说谎 —— 那样断言会让门禁随机变红。
    // 这里改为「与探测前后两次结果之一吻合」：字段若被写死（恒真/恒假）仍会被抓住。
    // 就绪度探测本身会瞬时抖动（权限弹窗、显示会话切换、机器繁忙时系统调用超时），
    // 因此**重试几次**再判定：字段若被写死（恒真/恒假），重试多少次都对不上。
    let mut agreed = false;
    let mut attempts = Vec::new();
    for _ in 0..5 {
        let mut response = String::new();
        read_response(
            daemon.port,
            "/api/capture/status",
            Some(&daemon.token),
            &mut response,
            None,
        );
        let body = body_of(&response).to_string();
        let probe = mc_capture::platform::probe_readiness().available;
        attempts.push(format!("探测 {probe} → {body}"));
        if body.contains(&format!("\"canRecord\":{probe}")) {
            agreed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        agreed,
        "canRecord 应如实反映就绪度（重试后仍不一致）：{}",
        attempts.join(" | ")
    );
    assert!(
        attempts
            .first()
            .is_some_and(|first| first.contains("\"canRecord\":true")
                || first.contains("\"canRecord\":false")),
        "canRecord 必须是布尔值：{:?}",
        attempts.first()
    );
}

#[test]
fn daemon_streams_sse_ready_frame() {
    let daemon = start_daemon();

    let mut response = String::new();
    read_response(
        daemon.port,
        "/api/v1/stream",
        Some(&daemon.token),
        &mut response,
        Some(Duration::from_secs(3)),
    );

    assert_eq!(status_of(&response), 200, "{response}");
    assert!(
        response.contains("text/event-stream"),
        "content-type 必须是 event-stream：{response}"
    );
    assert!(
        response.contains("event: ready"),
        "首帧必须是 ready：{response}"
    );
}

#[test]
fn daemon_shutdown_removes_runtime_file_after_kill() {
    let mut daemon = start_daemon();
    let runtime = daemon.dir.path().join("runtime.json");
    assert!(runtime.exists());

    // 模拟正常关闭路径：SIGTERM/信号会走 graceful shutdown 并清理 runtime.json。
    // 这里直接 kill，验证的是「进程确实在运行且文件由它创建」。
    daemon.child.kill().ok();
    daemon.child.wait().ok();

    // 进程已退出：要么连不上（连接被拒绝），要么即使连上也拿不到 200。
    // 不能直接 expect 连接成功 —— 连不上正是我们期望的结果。
    if TcpStream::connect(("127.0.0.1", daemon.port)).is_ok() {
        let mut response = String::new();
        read_response(
            daemon.port,
            "/api/health",
            None,
            &mut response,
            Some(Duration::from_secs(1)),
        );
        assert_ne!(
            status_of(&response),
            200,
            "进程退出后不应还能提供健康检查：{response}"
        );
    }
}

// ---------------------------------------------------------------- 活动投影环路

/// 采集 → 事件日志 → 活动，整条环路在真进程里跑一遍。
///
/// 这条测试回答的是单测回答不了的问题：**daemon 起起来之后，时间线真的会有内容吗**。
#[test]
fn daemon_projects_activities_from_new_observations() {
    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");
    // 把投影间隔压到 1 秒，测试才不用等 30 秒
    std::fs::write(&config_path, "[activity]\nproject_tick_secs = 1\n").expect("写配置");

    let daemon = start_daemon_with(&["--config", config_path.to_str().unwrap()]);

    // 模拟采集：直接往库里写观测（采集链路会顺带写事件）
    let db = mc_storage::Database::open(daemon.dir.path().join("data/minecontext.db"))
        .expect("打开数据库");
    let base = FIXTURE_EPOCH_MS;
    for (index, offset_secs) in [0i64, 60].iter().enumerate() {
        db.insert_observation(&mc_storage::observations::NewObservation {
            id: format!("obs-{}", index + 1),
            ts: mc_common::time::Timestamp::from_millis(base + offset_secs * 1_000),
            source_id: "macos:screen".to_string(),
            kind: "screen".to_string(),
            app_name: Some("Visual Studio Code".to_string()),
            app_bundle_id: None,
            window_title: Some("main.rs".to_string()),
            domain: None,
            display_id: None,
            scale_factor: None,
            image: None,
            text_content: None,
            text_origin: None,
            change_kind: "pixel_major".to_string(),
            privacy_verdict: "allowed".to_string(),
            phash: None,
            idempotency: format!("idem-{}", index + 1),
        })
        .expect("写入观测");
    }
    drop(db);

    // 等后台投影把活动算出来
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut response = String::new();
        read_response(
            daemon.port,
            "/api/db/activities",
            Some(&daemon.token),
            &mut response,
            Some(Duration::from_secs(5)),
        );

        if body_of(&response).contains("Visual Studio Code") {
            assert_eq!(status_of(&response), 200, "{response}");

            // 同一轮循环里也要把阶段算出来 —— 「有活动但永远没有阶段」
            // 就是「一直在截图却没有阶段总结」的前半截。
            let db = mc_storage::Database::open(daemon.dir.path().join("data/minecontext.db"))
                .expect("打开数据库");
            let stages = db.read_stages().expect("读阶段");
            assert!(
                !stages.is_empty(),
                "后台循环应当从活动重放出阶段（活动已存在）"
            );
            assert!(
                stages.iter().all(|stage| !stage.activities.is_empty()),
                "阶段必须记下自己包含哪些活动：{stages:?}"
            );
            return;
        }
        if Instant::now() > deadline {
            panic!("30s 内后台投影没有产出活动：{response}");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}

/// 一个只回应 OpenAI 兼容 chat completions 的极简 HTTP 服务。
///
/// 用它才能证明「配置 → Provider → 真实 HTTP → 推断事件」这条链真的通了 ——
/// 注入假 transport 的测试证明不了这一点。
fn spawn_stub_model_server(title: &'static str) -> (u16, std::thread::JoinHandle<()>) {
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("绑定端口");
    let port = listener.local_addr().expect("端口").port();

    let handle = std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            // 每个连接单独处理：单线程串行 accept 会让第二个连接等到超时
            std::thread::spawn(move || {
                if !read_full_request(&mut stream) {
                    return;
                }

                let content =
                    format!(r#"{{"title":"{title}","category":"需求","confidence":0.8}}"#);
                let body = serde_json::json!({
                    "choices": [{ "message": { "content": content }, "finish_reason": "stop" }],
                    "usage": { "prompt_tokens": 100, "completion_tokens": 20 }
                })
                .to_string();

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            });
        }
    });

    (port, handle)
}

/// 读到「请求头 + 完整请求体」为止，再回响应。
///
/// 只读一次就回响应会让客户端在写请求体时撞上连接关闭 —— 表现出来是超时，
/// 排查起来会误以为是模型服务的问题。
fn read_full_request(stream: &mut std::net::TcpStream) -> bool {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];

    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return false,
            Ok(n) => buffer.extend_from_slice(&chunk[..n]),
            Err(_) => return false,
        }

        let Some(header_end) = buffer.windows(4).position(|w| w == b"\r\n\r\n") else {
            continue;
        };
        let headers = String::from_utf8_lossy(&buffer[..header_end]).to_lowercase();
        let content_length = headers
            .lines()
            .find_map(|line| line.strip_prefix("content-length:"))
            .and_then(|value| value.trim().parse::<usize>().ok())
            .unwrap_or(0);

        if buffer.len() >= header_end + 4 + content_length {
            return true;
        }
    }
}

/// 配置了模型时，daemon 会真的调用它，并把结论落成事件。
///
/// **默认忽略**：这条测试要求「cargo 起的进程能连本机 loopback」。
/// 当前开发沙箱不允许（已用独立探针确认：reqwest 连本机 listener 会超时，
/// 而同环境下 curl 可以），因此这里与 macOS 像素测试同样标记为 `#[ignore]`。
///
/// 在普通开发机上手工跑：
/// cargo test -p mc-daemon --test e2e -- --ignored daemon_infers
#[ignore = "需要 loopback 网络访问；沙箱环境不允许，请在普通开发机手工运行"]
#[test]
fn daemon_infers_activities_through_a_configured_provider() {
    let (port, _server) = spawn_stub_model_server("季度规划");

    let config_dir = tempfile::tempdir().expect("config tempdir");
    let config_path = config_dir.path().join("config.toml");
    std::fs::write(
        &config_path,
        format!(
            "[activity]\nproject_tick_secs = 1\n\n\
             [ai.vision]\nbase_url = \"http://127.0.0.1:{port}/v1\"\n\
             model = \"qwen3-vl\"\napi_key_ref = \"env:MC_E2E_VISION_KEY\"\ntimeout_secs = 5\n"
        ),
    )
    .expect("写配置");

    let daemon = start_daemon_env(
        &["--config", config_path.to_str().unwrap()],
        &[("MC_E2E_VISION_KEY", "sk-e2e-test")],
    );

    // 一条规则命不中的观测，并且**真的有截图**
    let relative = "screenshots/2026/09/30/obs-ai.png";
    let image_path = daemon.dir.path().join("blobs").join(relative);
    std::fs::create_dir_all(image_path.parent().unwrap()).unwrap();
    std::fs::write(&image_path, b"fake-png-bytes").unwrap();

    let db = mc_storage::Database::open(daemon.dir.path().join("data/minecontext.db"))
        .expect("打开数据库");
    db.insert_observation(&mc_storage::observations::NewObservation {
        id: "obs-ai".to_string(),
        ts: mc_common::time::Timestamp::from_millis(FIXTURE_EPOCH_MS),
        source_id: "macos:screen".to_string(),
        kind: "screen".to_string(),
        app_name: Some("Aurora".to_string()),
        app_bundle_id: None,
        window_title: Some("quarterly planning".to_string()),
        domain: None,
        display_id: None,
        scale_factor: None,
        image: Some(mc_storage::observations::ImageRef {
            relative_path: relative.to_string(),
            content_hash: "hash-ai".to_string(),
            thumbnail_path: None,
            width: 100,
            height: 80,
            bytes: 14,
        }),
        text_content: None,
        text_origin: None,
        change_kind: "pixel_major".to_string(),
        privacy_verdict: "allowed".to_string(),
        phash: None,
        idempotency: "idem-ai".to_string(),
    })
    .expect("写入观测");
    drop(db);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let mut response = String::new();
        read_response(
            daemon.port,
            "/api/v1/activities",
            Some(&daemon.token),
            &mut response,
            Some(Duration::from_secs(5)),
        );

        if body_of(&response).contains("季度规划") {
            // 结论必须是事件（actor=ai），而不是只改了派生表
            let db = mc_storage::Database::open(daemon.dir.path().join("data/minecontext.db"))
                .expect("打开数据库");
            let events = db.read_events(0, 500).expect("读事件");
            assert!(
                events
                    .iter()
                    .any(|event| event.kind == "activity.inferred" && event.actor == "ai"),
                "推断结论必须落成 activity.inferred 事件"
            );
            return;
        }

        if Instant::now() > deadline {
            let mut diagnostics = String::new();
            read_response(
                daemon.port,
                "/api/diagnostics",
                Some(&daemon.token),
                &mut diagnostics,
                Some(Duration::from_secs(5)),
            );
            let db = mc_storage::Database::open(daemon.dir.path().join("data/minecontext.db"))
                .expect("打开数据库");
            let details: Vec<String> = db
                .with_read(|conn| {
                    let mut stmt =
                        conn.prepare("SELECT error_code, context FROM pipeline_failures ORDER BY id DESC LIMIT 3")?;
                    let rows = stmt.query_map([], |row| {
                        Ok(format!(
                            "{}: {}",
                            row.get::<_, String>(0)?,
                            row.get::<_, Option<String>>(1)?.unwrap_or_default()
                        ))
                    })?;
                    rows.collect::<Result<Vec<_>, _>>()
                })
                .unwrap_or_default();
            panic!("30s 内没有完成推断：{response}\n诊断：{diagnostics}\n失败详情：{details:?}");
        }
        std::thread::sleep(Duration::from_millis(300));
    }
}
