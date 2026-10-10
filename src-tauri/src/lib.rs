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

mod update_check;

/// `runtime.json` 里渲染层需要的最小字段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RuntimeInfo {
    pub port: u16,
    pub token: String,
}

// 只用到 `kill(pid, 0)` 的存在性检查，避免为一个调用引入完整 libc 依赖绑定。
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

struct ShellState {
    runtime: Option<RuntimeInfo>,
    daemon: Option<Child>,
    data_dir: PathBuf,
    /// 打包后 `Contents/Resources`；`get_runtime` 补拉 daemon 时需要。
    resource_dir: Option<PathBuf>,
    /// 限制重启频率，避免迁移失败时每个 poll 都 spawn。
    last_daemon_start: Option<Instant>,
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
///
/// 找不到二进制或 spawn 失败时打日志：打包版里这是「无法连接本地服务」的主因之一，
/// 静默 `None` 会让渲染层空等后误报硬错误。
fn start_daemon(data_dir: &std::path::Path, resource_dir: Option<&std::path::Path>) -> Option<Child> {
    let candidates = daemon_candidates(resource_dir);
    let Some(binary) = candidates.iter().find(|path| path.is_file()) else {
        eprintln!(
            "[shell] 找不到 mc-daemon（resource_dir={resource_dir:?}，candidates={candidates:?}）"
        );
        return None;
    };
    // 先清旧 runtime，避免 wait/get 读到已死进程的过期端口。
    let _ = std::fs::remove_file(data_dir.join("runtime.json"));
    match Command::new(binary)
        .arg("--data-dir")
        .arg(data_dir)
        .arg("--port")
        .arg("0")
        .spawn()
    {
        Ok(child) => Some(child),
        Err(error) => {
            eprintln!("[shell] 启动 mc-daemon 失败（{}）：{error}", binary.display());
            None
        }
    }
}

fn daemon_has_exited(daemon: &mut Option<Child>) -> bool {
    match daemon.as_mut() {
        None => true,
        Some(child) => match child.try_wait() {
            Ok(None) => false,
            Ok(Some(status)) => {
                eprintln!("[shell] mc-daemon 已退出（{status}），将按需重启");
                true
            }
            Err(error) => {
                eprintln!("[shell] 检查 mc-daemon 状态失败：{error}");
                true
            }
        },
    }
}

/// 磁盘 `runtime.json` 里外壳自愈需要的字段（含 pid，用于接管 / 判死）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct DiskRuntime {
    port: u16,
    token: String,
    pid: Option<u32>,
}

impl DiskRuntime {
    fn to_runtime(&self) -> RuntimeInfo {
        RuntimeInfo {
            port: self.port,
            token: self.token.clone(),
        }
    }
}

/// `get_runtime` 自愈决策：检测死进程 / 端口冲突，决定接管、重启或等待。
///
/// 与 #42 的 handoff/spawn 收敛：子进程还在时绝不重启；pid+端口仍活则接管；
/// 过期或端口被无关进程占用则清掉 runtime 再拉起，并靠冷却避免重启风暴。
#[derive(Debug, Clone, PartialEq, Eq)]
enum HealAction {
    Use(RuntimeInfo),
    Adopt(RuntimeInfo),
    Restart,
    WaitCooldown,
    WaitRuntime,
}

#[derive(Debug, Clone)]
struct HealFacts {
    cached: Option<RuntimeInfo>,
    disk: Option<DiskRuntime>,
    child_running: bool,
    disk_pid_alive: bool,
    port_reachable: bool,
    cooldown_elapsed: bool,
}

fn decide_daemon_heal(facts: &HealFacts) -> HealAction {
    if facts.child_running {
        if let Some(info) = facts.cached.clone().or_else(|| {
            facts.disk.as_ref().map(DiskRuntime::to_runtime)
        }) {
            return HealAction::Use(info);
        }
        return HealAction::WaitRuntime;
    }

    // 子进程已死/丢失：若磁盘 runtime 的 pid 与端口仍可用，接管（勿再 spawn）。
    if let Some(disk) = facts.disk.as_ref() {
        if facts.disk_pid_alive && facts.port_reachable {
            return HealAction::Adopt(disk.to_runtime());
        }
    }

    // 过期缓存/死端口/端口被无关进程占用 → 重启；冷却中则空等，避免循环拉起。
    if !facts.cooldown_elapsed {
        return HealAction::WaitCooldown;
    }
    HealAction::Restart
}

fn read_disk_runtime(data_dir: &std::path::Path) -> Option<DiskRuntime> {
    let text = std::fs::read_to_string(data_dir.join("runtime.json")).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    Some(DiskRuntime {
        port: value.get("port")?.as_u64()? as u16,
        token: value.get("token")?.as_str()?.to_string(),
        pid: value
            .get("pid")
            .and_then(|v| v.as_u64())
            .map(|v| v as u32),
    })
}

fn process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    (unsafe { libc_kill(pid as i32, 0) }) == 0
}

fn port_reachable(port: u16) -> bool {
    use std::net::{SocketAddr, TcpStream};
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok()
}

const DAEMON_RESTART_COOLDOWN: Duration = Duration::from_secs(2);

/// 缓存 / 磁盘补读；子进程已死时按策略接管或重启（不阻塞长等）。
///
/// setup 超时开窗后，旧逻辑只读盘：daemon 若已崩，渲染层会空等到硬错误。
/// 这里让每次 `get_runtime` 都有机会把守护进程拉起来或接管仍活着的实例，
/// 由前端短轮询收敛。**不得**在缓存命中时跳过死进程检测。
fn ensure_runtime(state: &mut ShellState) -> Option<RuntimeInfo> {
    let data_dir = state.data_dir.clone();
    let child_running = !daemon_has_exited(&mut state.daemon);
    if !child_running {
        state.daemon = None;
    }

    let disk = read_disk_runtime(&data_dir);
    let disk_pid_alive = disk
        .as_ref()
        .and_then(|d| d.pid)
        .map(process_alive)
        .unwrap_or(false);
    let port_ok = disk
        .as_ref()
        .map(|d| port_reachable(d.port))
        .unwrap_or(false);
    let cooldown_elapsed = state
        .last_daemon_start
        .map(|at| at.elapsed() >= DAEMON_RESTART_COOLDOWN)
        .unwrap_or(true);

    let facts = HealFacts {
        cached: state.runtime.clone(),
        disk: disk.clone(),
        child_running,
        disk_pid_alive,
        port_reachable: port_ok,
        cooldown_elapsed,
    };

    match decide_daemon_heal(&facts) {
        HealAction::Use(info) | HealAction::Adopt(info) => {
            state.runtime = Some(info.clone());
            Some(info)
        }
        HealAction::WaitRuntime | HealAction::WaitCooldown => {
            // 子进程已死时不得继续缓存过期 port/token，否则冷却结束后仍可能误用。
            if !child_running {
                state.runtime = None;
            }
            None
        }
        HealAction::Restart => {
            state.runtime = None;
            let _ = std::fs::remove_file(data_dir.join("runtime.json"));
            let resource = state.resource_dir.clone();
            if let Some(child) = start_daemon(&data_dir, resource.as_deref()) {
                state.daemon = Some(child);
            }
            state.last_daemon_start = Some(Instant::now());
            // 刚 spawn：本次不阻塞长等，交给前端短轮询下一次 get_runtime。
            None
        }
    }
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

/// 优先内存缓存；空时补读磁盘。
///
/// setup 里 `wait_for_runtime` 超时后仍会开窗口：若 daemon 随后才写好
/// `runtime.json`，只读缓存会让渲染层永远 `backendReady=false`，误报
/// 「无法连接本地服务」。补读一次即可收敛。
fn runtime_from_cache_or_disk(
    runtime: &mut Option<RuntimeInfo>,
    data_dir: &std::path::Path,
) -> Option<RuntimeInfo> {
    if let Some(info) = runtime.as_ref() {
        return Some(info.clone());
    }
    if let Some(info) = read_runtime(data_dir) {
        *runtime = Some(info.clone());
        return Some(info);
    }
    None
}

/// 从外壳状态取 runtime（含必要时重启 daemon）。
fn take_runtime(state: &Mutex<ShellState>) -> Option<RuntimeInfo> {
    let mut guard = state.lock().ok()?;
    ensure_runtime(&mut guard)
}

#[tauri::command]
fn get_runtime(state: tauri::State<'_, Mutex<ShellState>>) -> Option<RuntimeInfo> {
    take_runtime(&state)
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

/// 对照 GitHub Releases 检查是否有新版本（不下载、不安装）。
#[tauri::command]
fn check_for_update() -> Result<mc_update::CheckForUpdateResult, String> {
    update_check::fetch_and_evaluate(env!("CARGO_PKG_VERSION"))
}

/// 用系统默认浏览器 / 下载器打开发布页或 dmg 链接。
#[tauri::command]
fn open_external_url(url: String) -> Result<(), String> {
    update_check::open_external_url(&url)
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

/// 渲染层读 `window.mcRuntime.get()`。
///
/// setup 若已拿到 runtime，把它**种子化进初始化脚本**：打包版上 invoke ACL
/// 或首屏时序一旦抖动，仍能在第一次 `get()` 拿到 port/token，避免 daemon
/// 已 listening 而界面误报「无法连接本地服务」。live invoke 优先（daemon 重启
/// 后端口会变）；失败或空结果再回落种子。
fn mcruntime_script(runtime: &Option<RuntimeInfo>) -> String {
    let seeded = runtime
        .as_ref()
        .and_then(|info| serde_json::to_string(info).ok())
        .unwrap_or_else(|| "null".to_string());
    format!(
        r#"
(() => {{
  const invoke = (cmd, args) => window.__TAURI__.core.invoke(cmd, args)
  const seeded = {seeded}
  window.mcRuntime = {{
    get: async () => {{
      try {{
        const live = await invoke('get_runtime')
        // null = 外壳自愈中/尚未就绪；不得回落种子（种子可能是已死端口）。
        if (live && live.port && live.token) return live
        return null
      }} catch (error) {{
        // 仅 invoke 失败（如 ACL）才回落种子，与 #42 首屏 handoff 收敛。
        console.warn('[mcRuntime] get_runtime invoke failed, falling back to seed', error)
        if (seeded && seeded.port && seeded.token) return seeded
        return null
      }}
    }},
    shell: {{ notification: true }}
  }}
}})()
"#
    )
}

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
        resource_dir: None,
        last_daemon_start: None,
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
            clipboard_write_text,
            check_for_update,
            open_external_url
        ])
        .setup(move |app| {
            let resource_dir = app.path().resource_dir().ok();
            {
                let state_handle = app.state::<Mutex<ShellState>>();
                let mut state = state_handle.lock().unwrap();
                state.resource_dir = resource_dir.clone();
                // 先把路径取出来再 IO：持锁空等 20s 会挡住后续 get_runtime。
                let dir = state.data_dir.clone();
                let resource = state.resource_dir.clone();
                drop(state);
                let daemon = start_daemon(&dir, resource.as_deref());
                let started_at = Instant::now();
                let runtime = wait_for_runtime(&dir, Duration::from_secs(20));
                let mut state = state_handle.lock().unwrap();
                state.daemon = daemon;
                state.runtime = runtime;
                state.last_daemon_start = Some(started_at);
            }

            let seeded = {
                let state_handle = app.state::<Mutex<ShellState>>();
                state_handle
                    .lock()
                    .ok()
                    .and_then(|guard| guard.runtime.clone())
            };
            if seeded.is_none() {
                eprintln!(
                    "[shell] 开窗前仍无 runtime（daemon 可能尚未写出 runtime.json）；渲染层将轮询 get_runtime / 种子回落"
                );
            }
            let window = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("MineContext")
                .inner_size(1280.0, 800.0)
                .min_inner_size(960.0, 640.0)
                .initialization_script(mcruntime_script(&seeded))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn cache_hit_does_not_require_disk() {
        let dir = tempfile_dir();
        let mut cached = Some(RuntimeInfo {
            port: 4242,
            token: "cached".into(),
        });
        let got = runtime_from_cache_or_disk(&mut cached, &dir).expect("cache");
        assert_eq!(got.port, 4242);
        assert_eq!(got.token, "cached");
    }

    #[test]
    fn empty_cache_reads_runtime_json_from_disk() {
        let dir = tempfile_dir();
        fs::write(
            dir.join("runtime.json"),
            r#"{"port":43117,"token":"from-disk"}"#,
        )
        .unwrap();
        let mut cached = None;
        let got = runtime_from_cache_or_disk(&mut cached, &dir).expect("disk");
        assert_eq!(got.port, 43117);
        assert_eq!(got.token, "from-disk");
        assert!(cached.is_some(), "补读后应写入缓存");
    }

    #[test]
    fn empty_cache_and_missing_file_stays_none() {
        let dir = tempfile_dir();
        let mut cached = None;
        assert!(runtime_from_cache_or_disk(&mut cached, &dir).is_none());
        assert!(cached.is_none());
    }

    /// 钉住 get_runtime 同款路径：经 MutexGuard 取 runtime 时不得 E0502。
    #[test]
    fn take_runtime_through_mutex_adopts_live_disk_when_cache_empty() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let pid = std::process::id();
        let dir = tempfile_dir();
        fs::write(
            dir.join("runtime.json"),
            format!(r#"{{"port":{port},"token":"mutex-path","pid":{pid}}}"#),
        )
        .unwrap();
        let state = Mutex::new(ShellState {
            runtime: None,
            daemon: None,
            data_dir: dir,
            resource_dir: None,
            last_daemon_start: None,
        });
        let got = take_runtime(&state).expect("adopt via mutex");
        assert_eq!(got.port, port);
        assert_eq!(got.token, "mutex-path");
        assert!(state.lock().unwrap().runtime.is_some());
        drop(listener);
    }

    #[test]
    fn daemon_has_exited_when_missing_or_finished() {
        let mut none = None;
        assert!(daemon_has_exited(&mut none));

        let mut child = Some(Command::new("true").spawn().expect("spawn true"));
        let _ = child.as_mut().unwrap().wait();
        assert!(daemon_has_exited(&mut child));
    }

    fn sample_runtime(port: u16) -> RuntimeInfo {
        RuntimeInfo {
            port,
            token: format!("tok-{port}"),
        }
    }

    fn sample_disk(port: u16, pid: Option<u32>) -> DiskRuntime {
        DiskRuntime {
            port,
            token: format!("tok-{port}"),
            pid,
        }
    }

    #[test]
    fn heal_uses_cache_while_child_still_running() {
        let action = decide_daemon_heal(&HealFacts {
            cached: Some(sample_runtime(4001)),
            disk: None,
            child_running: true,
            disk_pid_alive: false,
            port_reachable: false,
            cooldown_elapsed: true,
        });
        assert_eq!(action, HealAction::Use(sample_runtime(4001)));
    }

    #[test]
    fn heal_never_restarts_while_child_running_without_runtime() {
        let action = decide_daemon_heal(&HealFacts {
            cached: None,
            disk: None,
            child_running: true,
            disk_pid_alive: false,
            port_reachable: false,
            cooldown_elapsed: true,
        });
        assert_eq!(action, HealAction::WaitRuntime);
    }

    #[test]
    fn heal_adopts_live_disk_runtime_when_child_handle_lost() {
        let action = decide_daemon_heal(&HealFacts {
            cached: None,
            disk: Some(sample_disk(4002, Some(4242))),
            child_running: false,
            disk_pid_alive: true,
            port_reachable: true,
            cooldown_elapsed: true,
        });
        assert_eq!(action, HealAction::Adopt(sample_runtime(4002)));
    }

    #[test]
    fn heal_restarts_when_daemon_dead_and_port_unreachable() {
        let action = decide_daemon_heal(&HealFacts {
            cached: Some(sample_runtime(4003)),
            disk: Some(sample_disk(4003, Some(9))),
            child_running: false,
            disk_pid_alive: false,
            port_reachable: false,
            cooldown_elapsed: true,
        });
        assert_eq!(action, HealAction::Restart);
    }

    #[test]
    fn heal_restarts_on_port_conflict_when_pid_dead_but_port_busy() {
        // 旧 pid 已死，端口却被无关进程占用：不得 Adopt，应清掉过期 runtime 再拉起。
        let action = decide_daemon_heal(&HealFacts {
            cached: Some(sample_runtime(4004)),
            disk: Some(sample_disk(4004, Some(9))),
            child_running: false,
            disk_pid_alive: false,
            port_reachable: true,
            cooldown_elapsed: true,
        });
        assert_eq!(action, HealAction::Restart);
    }

    #[test]
    fn heal_cooldown_blocks_restart_loop() {
        let action = decide_daemon_heal(&HealFacts {
            cached: None,
            disk: None,
            child_running: false,
            disk_pid_alive: false,
            port_reachable: false,
            cooldown_elapsed: false,
        });
        assert_eq!(action, HealAction::WaitCooldown);
    }

    #[test]
    fn ensure_runtime_clears_stale_cache_when_child_exited() {
        let dir = tempfile_dir();
        fs::write(
            dir.join("runtime.json"),
            r#"{"port":43133,"token":"stale","pid":429496729}"#,
        )
        .unwrap();
        let mut finished = Some(Command::new("true").spawn().expect("spawn true"));
        let _ = finished.as_mut().unwrap().wait();
        let mut state = ShellState {
            runtime: Some(RuntimeInfo {
                port: 43133,
                token: "stale".into(),
            }),
            daemon: finished,
            data_dir: dir.clone(),
            resource_dir: Some(PathBuf::from("/no-such-minecontext-resources")),
            last_daemon_start: None,
        };
        // 子进程已退出 + 死 pid + 端口不可达 → Restart；无 binary 时仍须清过期缓存。
        let got = ensure_runtime(&mut state);
        assert!(got.is_none(), "自愈中不得继续返回过期 runtime");
        assert!(state.runtime.is_none(), "过期缓存必须清掉");
        assert!(
            !dir.join("runtime.json").exists(),
            "过期 runtime.json 应被移除以便新 daemon 重写"
        );
    }

    #[test]
    fn ensure_runtime_adopts_disk_runtime_when_pid_and_port_live() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().unwrap().port();
        let pid = std::process::id();
        let dir = tempfile_dir();
        fs::write(
            dir.join("runtime.json"),
            format!(r#"{{"port":{port},"token":"ensure-disk","pid":{pid}}}"#),
        )
        .unwrap();
        let mut state = ShellState {
            runtime: None,
            daemon: None,
            data_dir: dir,
            resource_dir: None,
            // 冷却中：若误判成 Restart 会去 spawn；接管路径不得 spawn。
            last_daemon_start: Some(Instant::now()),
        };
        let got = ensure_runtime(&mut state).expect("adopt");
        assert_eq!(got.port, port);
        assert_eq!(got.token, "ensure-disk");
        assert!(state.daemon.is_none(), "接管不得再 spawn");
        drop(listener);
    }

    #[test]
    fn ensure_runtime_cooldown_skips_spawn_thrash_when_still_empty() {
        let dir = tempfile_dir();
        let mut state = ShellState {
            runtime: None,
            daemon: None,
            data_dir: dir,
            resource_dir: Some(PathBuf::from("/no-such-minecontext-resources")),
            last_daemon_start: Some(Instant::now()),
        };
        assert!(ensure_runtime(&mut state).is_none());
        assert!(state.daemon.is_none());
    }

    #[test]
    fn mcruntime_script_embeds_seeded_runtime_for_first_paint() {
        let script = mcruntime_script(&Some(RuntimeInfo {
            port: 64592,
            token: "seed-token".into(),
        }));
        assert!(script.contains("64592"), "port must be in init script");
        assert!(script.contains("seed-token"), "token must be in init script");
        assert!(script.contains("get_runtime"), "live invoke still preferred");
        assert!(script.contains("falling back to seed"));
        // live 返回 null（自愈中）不得回落种子，否则会把已死端口再交给前端。
        assert!(
            script.contains("return null"),
            "null live must not fall back to seed"
        );
    }

    #[test]
    fn mcruntime_script_null_seed_when_setup_timed_out() {
        let script = mcruntime_script(&None);
        assert!(script.contains("const seeded = null"));
    }

    fn tempfile_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mc-shell-runtime-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
}
