//! Tauri 外壳：窗口 + daemon 生命周期。
//!
//! 外壳只管窗口与守护进程，业务全在 `mc-daemon` 里；渲染层通过
//! `window.mcRuntime.get()` 拿端口与 token，
//! 因此这里用一个**初始化脚本**把它接到 Tauri 命令上 —— 渲染层与适配层
//! 一行都不用改。

use std::path::PathBuf;
use std::process::{Child, Command};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};
use tauri::{Emitter, Manager, RunEvent, WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent};

/// `runtime.json` 里渲染层需要的最小字段。
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeInfo {
    pub port: u16,
    pub token: String,
}

struct ShellState {
    runtime: Option<RuntimeInfo>,
    daemon: Option<Child>,
    data_dir: PathBuf,
}

/// 托盘句柄：渲染层上报的录制状态要能改到托盘上（菜单文案 + 悬停提示）。
struct TrayHandles {
    toggle: MenuItem<tauri::Wry>,
}

/// 数据目录：`MC_DATA_DIR` → `~/Library/Application Support/MineContext`。
fn data_dir() -> PathBuf {
    if let Ok(explicit) = std::env::var("MC_DATA_DIR") {
        return PathBuf::from(explicit);
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join("Library/Application Support/MineContext")
}

/// 找 `mc-daemon`：打包后在 `resources/backend/`，开发时在仓库 `target/`。
fn daemon_candidates(resource_dir: Option<&std::path::Path>) -> Vec<PathBuf> {
    let name = "mc-daemon";
    let mut out = Vec::new();
    if let Ok(explicit) = std::env::var("MC_DAEMON_BIN") {
        out.push(PathBuf::from(explicit));
    }
    if let Some(dir) = resource_dir {
        out.push(dir.join("backend").join(name));
    }
    // 开发态：从 src-tauri/ 往上是仓库根
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    out.push(repo.join("target/release").join(name));
    out.push(repo.join("target/debug").join(name));
    out
}

fn read_runtime(data_dir: &std::path::Path) -> Option<RuntimeInfo> {
    let text = std::fs::read_to_string(data_dir.join("runtime.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(RuntimeInfo {
        port: value.get("port")?.as_u64()? as u16,
        token: value.get("token")?.as_str()?.to_string(),
    })
}

/// 起 daemon 进程；`runtime.json` 的等待由 `wait_for_runtime` 负责。
fn start_daemon(data_dir: &std::path::Path, resource_dir: Option<&std::path::Path>) -> Option<Child> {
    let binary = daemon_candidates(resource_dir)
        .into_iter()
        .find(|path| path.is_file())?;
    let _ = std::fs::remove_file(data_dir.join("runtime.json"));
    let child = Command::new(binary)
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--port")
        .arg("0")
        .spawn()
        .ok()?;
    Some(child)
}

fn wait_for_runtime(data_dir: &std::path::Path, timeout: Duration) -> Option<RuntimeInfo> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(info) = read_runtime(data_dir) {
            return Some(info);
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

#[tauri::command]
fn get_runtime(state: tauri::State<'_, Mutex<ShellState>>) -> Option<RuntimeInfo> {
    state.lock().ok()?.runtime.clone()
}

/// 渲染层上报录制状态：更新托盘提示与菜单文案。
///
/// 只做展示，不做业务判断 —— 「该不该在录」由 daemon 回答，外壳不持第二份真相。
#[tauri::command]
fn tray_recording_status(app: tauri::AppHandle, recording: bool) -> Result<(), String> {
    let handles = app.state::<TrayHandles>();
    handles
        .toggle
        .set_text(if recording { "暂停录制" } else { "开始录制" })
        .map_err(|error| error.to_string())?;
    if let Some(tray) = app.tray_by_id("main") {
        tray.set_tooltip(Some(if recording {
            "MineContext · 录制中"
        } else {
            "MineContext · 已暂停"
        }))
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

/// 渲染层日志落盘：写进与外壳同一份日志文件（`<日志目录>/renderer.log`）。
///
/// 只做转发，不做业务判断；**失败也不回抛** —— 日志写不进去不该让界面功能跟着坏。
#[tauri::command]
fn renderer_log(level: String, message: String) {
    match level.as_str() {
        "error" => log::error!(target: "renderer", "{message}"),
        "warn" => log::warn!(target: "renderer", "{message}"),
        "debug" => log::debug!(target: "renderer", "{message}"),
        _ => log::info!(target: "renderer", "{message}"),
    }
}

#[tauri::command]
fn launch_at_login(app: tauri::AppHandle) -> Result<bool, String> {
    app.autolaunch().is_enabled().map_err(|error| error.to_string())
}

/// 打开/关闭开机自启；写完再读回一次，避免「调用成功但系统没生效」。
#[tauri::command]
fn set_launch_at_login(app: tauri::AppHandle, enabled: bool) -> Result<bool, String> {
    let manager = app.autolaunch();
    let outcome = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    outcome.map_err(|error| error.to_string())?;
    manager.is_enabled().map_err(|error| error.to_string())
}

/// 写入系统剪贴板（设置页复制 API Key 等）。
///
/// WebView 的 `navigator.clipboard` 在非安全上下文 / 权限拒绝时会失败；
/// 外壳用系统 API 写入，避免「复制失败还只能选到脱敏串」。
#[tauri::command]
fn clipboard_write_text(text: String) -> Result<(), String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|error| error.to_string())?;
    clipboard.set_text(text).map_err(|error| error.to_string())
}


/// 进程级单实例锁：`<数据目录>/.shell.lock` 存 pid。
///
/// 不做「唤起已有窗口」的 IPC（那需要额外的单实例插件与频道）；这一片只要
/// **不出现两个外壳同时拉起两个 daemon**，第二个实例直接退出并说明原因。
fn acquire_single_instance(data_dir: &std::path::Path) -> bool {
    let path = data_dir.join(".shell.lock");
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(pid) = text.trim().parse::<i32>() {
            // `kill -0`：进程还在就说明有另一个外壳
            let alive = unsafe { libc_kill(pid, 0) } == 0;
            if alive && pid != std::process::id() as i32 {
                eprintln!("MineContext 已在运行（pid {pid}），本次启动退出");
                return false;
            }
        }
    }
    let _ = std::fs::write(&path, std::process::id().to_string());
    true
}

// 只用到 `kill(pid, 0)` 的存在性检查，避免为一个调用引入完整 libc 依赖绑定。
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

/// 关掉 daemon 并清掉 `runtime.json`（优雅退出与信号退出共用）。
fn shutdown(state: &mut ShellState) {
    if let Some(mut child) = state.daemon.take() {
        let _ = child.kill();
        let _ = child.wait();
    }
    let _ = std::fs::remove_file(state.data_dir.join("runtime.json"));
    let _ = std::fs::remove_file(state.data_dir.join(".shell.lock"));
}

/// 关窗 = 收进托盘：产品要「关掉窗口但后台继续记录」，
/// 直接退出会让采集停摆。真退出走托盘菜单或 Cmd+Q。
fn attach_close_to_tray(window: &WebviewWindow) {
    let handle = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = handle.hide();
        }
    });
}

/// 托盘菜单：显示窗口 / 开始-暂停录制 / 屏幕监控 / 退出。图标用 bundle 里的那份。
///
/// 动作**不在外壳里实现业务**：外壳只把事件发给渲染层
/// （`push:tray-toggle-recording` / `push:tray-navigate-to-screen-monitor`），
/// 由渲染层调 daemon 接口 —— 否则「托盘切换录制」会变成第二份采集开关逻辑。
/// 两个动作都要把窗口显示出来：渲染层的处理是「跳到屏幕监控页并切换」，窗口还藏
/// 着的话用户看到的是「点了没反应」。
fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
    let toggle = MenuItem::with_id(app, "toggle-recording", "开始 / 暂停录制", true, None::<&str>)?;
    let monitor = MenuItem::with_id(app, "show-screen-monitor", "屏幕监控", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &toggle, &monitor, &quit])?;

    // 菜单文案要能改（开始/暂停），所以句柄进状态；托盘本身按 id 取
    app.manage(TrayHandles { toggle });

    TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().cloned().unwrap_or_else(|| {
            // 没有默认图标时托盘仍然要能创建：用空图标而不是 panic
            tauri::image::Image::new_owned(Vec::new(), 0, 0)
        }))
        .menu(&menu)
        .tooltip("MineContext")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => show_main_window(app),
            // 渲染层收到事件后会跳到屏幕监控页并切换录制，所以这里也要把窗口显示出来；
            // 「窗口收起来也能操作」由托盘状态回显（提示 + 菜单文案）负责反馈。
            "toggle-recording" => {
                show_main_window(app);
                let _ = app.emit("push:tray-toggle-recording", ());
            }
            "show-screen-monitor" => {
                // 这个是「去看一眼」，所以顺带把窗口显示出来，否则用户以为没反应
                show_main_window(app);
                let _ = app.emit("push:tray-navigate-to-screen-monitor", ());
            }
            "quit" => {
                let state_handle = app.state::<Mutex<ShellState>>();
                if let Ok(mut state) = state_handle.lock() {
                    shutdown(&mut state);
                }
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// 渲染层读 `window.mcRuntime.get()`；这里把它接到上面的命令。
const MCRUNTIME_SCRIPT: &str = r#"
(() => {
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args)
  window.mcRuntime = {
    get: () => invoke('get_runtime'),
    // 外壳能力声明：渲染层适配层（adapters/tauri-shell.ts）据此决定
    // 走真系统通知，还是降级为应用内提示。
    shell: { notification: true }
  }
})()
"#;

pub fn run() {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);

    if !acquire_single_instance(&dir) {
        std::process::exit(0);
    }

    let state = ShellState {
        runtime: None,
        daemon: None,
        data_dir: dir.clone(),
    };

    let app = tauri::Builder::default()
        .manage(Mutex::new(state))
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(tauri_plugin_notification::init())
        // 渲染层日志落盘：打包版里 webview 控制台是看不到的，缺了这条就只剩
        // 「用户说某个功能没反应，而我们没有任何日志可查」。
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("renderer".to_string()),
                    }),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                ])
                .level(log::LevelFilter::Info)
                .build(),
        )
        .invoke_handler(tauri::generate_handler![
            get_runtime,
            launch_at_login,
            set_launch_at_login,
            tray_recording_status,
            renderer_log,
            clipboard_write_text
        ])
        .setup(move |app| {
            let resource_dir = app.path().resource_dir().ok();
            {
                let state_handle = app.state::<Mutex<ShellState>>();
                let mut state = state_handle.lock().unwrap();
                // 先把数据目录取出来，避免在持锁期间做 IO（daemon 要几秒才写好 runtime）
                let dir = state.data_dir.clone();
                let daemon = start_daemon(&dir, resource_dir.as_deref());
                let runtime = wait_for_runtime(&dir, Duration::from_secs(20));
                state.daemon = daemon;
                state.runtime = runtime;
            }

            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("MineContext")
                .inner_size(1280.0, 800.0)
                .min_inner_size(960.0, 640.0)
                .initialization_script(MCRUNTIME_SCRIPT)
                .build()?;
            attach_close_to_tray(&window);
            setup_tray(app.handle())?;
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Tauri 应用初始化失败");

    attach_signal_shutdown(&app.handle().clone(), &dir);

    app.run(|handle, event| {
        if let RunEvent::ExitRequested { .. } | RunEvent::Exit = event {
            let state_handle = handle.state::<Mutex<ShellState>>();
            let mut guard = state_handle.lock();
            if let Ok(state) = guard.as_mut() {
                shutdown(state);
            }
        }
    });
}

/// SIGTERM / SIGINT 也要能收摊：`pkill`、进程管理器、容器停止发的都是 SIGTERM，
/// 只处理窗口关闭的话 `runtime.json` 会留在磁盘上，下次读到过期端口。
///
/// 退出必须走 [`tauri::AppHandle::exit`]（事件循环在主线程上收摊），**不能**在这个
/// 线程里调 `std::process::exit`：macOS 上 AppKit 的拆除要主线程，从信号线程直接
/// `exit` 会让进程卡在多个满负载的线程里不退出（启动检查只验文件清理，不验进程真的退出）。
fn attach_signal_shutdown(app: &tauri::AppHandle, dir: &std::path::Path) {
    let Ok(mut signals) = signal_hook::iterator::Signals::new([
        signal_hook::consts::SIGTERM,
        signal_hook::consts::SIGINT,
    ]) else {
        return;
    };
    let app = app.clone();
    let dir = dir.to_path_buf();

    std::thread::spawn(move || {
        if signals.forever().next().is_none() {
            return;
        }
        // 先做与文件/子进程有关的收摊（幂等），再请事件循环退出：
        // 万一事件循环还没起来（exit 排不上队），这几步至少已经做完了。
        let _ = std::fs::remove_file(dir.join("runtime.json"));
        let _ = std::fs::remove_file(dir.join(".shell.lock"));
        let _ = std::process::Command::new("pkill")
            .args(["-f", "mc-daemon --data-dir"])
            .status();

        app.exit(0);

        // 兜底：给主线程 5 秒把事件循环收掉；仍不退才硬退（此时已无更安全的选项）。
        std::thread::sleep(Duration::from_secs(5));
        std::process::exit(0);
    });
}
