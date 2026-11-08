//! Pawch Agent 桌面壳—— 窗口 + 托盘 + 服务保活 + **child webview 宿主**
//!
//! 设计（窄而刚 v1）：
//! - WebView 直接加载 http://127.0.0.1:5173（前端零改动，pawch-agent.exe 是唯一真源）；
//! - 启动时探测 5173，未起则 spawn `<repo>/target/release/pawch-agent.exe`（start.bat 内置化）；
//! - 托盘：左键显示/聚焦窗口，右键菜单（打开主窗口 / 退出壳）；
//! - 壳退出**不杀**服务（服务独立生命周期）；
//! - pawch-agent.exe 路径先找壳同级目录（安装版），再按开发机布局
//!   `<repo>/desktop/target/release/壳` → `<repo>/target/release/服务`。
//!
//! 内置浏览器改造：浏览器不再由服务另起一个无头 Edge 进程，而是**把网页变成
//! 壳窗口里的 child webview**（真页面、真交互、复用 App 自己的 WebView2 运行时）。
//! - 主窗口 WebView2 环境带 `--remote-debugging-port`（tauri.conf.json 的
//!   additionalBrowserArguments），child webview 带同一份 args 以复用同一环境——
//!   服务端据此用页面级 CDP 驱动这些 webview（快照/点击/输入/截图协议不变）。
//! - 壳**不决策**标签页生命周期：服务是唯一 owner，通过 127.0.0.1:9225 的
//!   控制通道（换行分隔 JSON）下发 create/close/rect/visible/devtools 五类指令；
//!   壳只负责执行（create 从控制线程发起——add_child 内部 run_on_main_thread +
//!   recv，主线程调用会死锁）。
//! - 单实例守卫：两个壳会抢 9223 的 CDP 端口，第二个壳的 webview 将不可见。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::thread;
use std::time::Duration;

use serde_json::{Value, json};
use tauri::{LogicalPosition, LogicalSize, Manager, WebviewBuilder, WebviewUrl};
use tauri_runtime::ResizeDirection;

const SERVICE_URL: &str = "127.0.0.1:5173";
/// 壳 WebView2 环境的 CDP 端口（与 src/browser/shell.rs 的 SHELL_CDP_PORT 保持一致）
const CDP_PORT: u16 = 9223;
/// 主窗口与 child webview 共用的 browser args（与 tauri.conf.json 的
/// additionalBrowserArguments 保持一致——环境已存在时 options 以首次创建为准，
/// child 带同一份是为防 WebView2 按 options 差异另起环境）。
/// 只带调试端口与地址，**不带** --force-device-scale-factor 之类会改变 App 自身
/// 渲染缩放的 flag（外部方案踩过的坑）。
const BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
 --remote-debugging-port=9223 --remote-debugging-address=127.0.0.1";
/// 服务端 webview 控制通道（pawch-agent.exe 监听，壳来连）
const CTL_ADDR: (&str, u16) = ("127.0.0.1", 9225);
/// 单实例守卫端口（第二个壳直接退出，不抢 CDP 端口）
const GUARD_PORT: u16 = 9224;
/// child webview 弹窗防护：window.open 直接掐掉（on_new_window 再兜一层）
const POPUP_GUARD_JS: &str = "window.open = function () { return null; };";
/// 网页面板滚动条跟随应用主题（2026-11-08 用户实锤：浅色模式下深色页面/未声明
/// color-scheme 的页面把滚动条画成深色，跟浅色面板不搭）。首选色系（prefers-color-
/// scheme）跟着窗口主题走、窗口主题跟应用主题走，所以这里监听它切换即可：
/// 浅色 = 注入一套浅色滚动条样式（!important 压过页面自带的深色滚动条），
/// 深色 = 摘掉样式还页面本来面目。只在网页面板的 webview 注入，menu 弹层不注。
const SCROLLBAR_THEME_JS: &str = r#"(function () {
  var CSS = '::-webkit-scrollbar{width:14px !important;height:14px !important}'
    + '::-webkit-scrollbar-track{background:#efedea !important}'
    + '::-webkit-scrollbar-thumb{background:#c9c4bd !important;border-radius:7px !important;border:3px solid #efedea !important}'
    + '::-webkit-scrollbar-thumb:hover{background:#b3ada4 !important}'
    + '::-webkit-scrollbar-corner{background:#efedea !important}';
  var style = null;
  function apply() {
    var light = false;
    try { light = window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches; } catch (e) {}
    if (!light) {
      if (style) { style.parentNode && style.parentNode.removeChild(style); style = null; }
      return;
    }
    if (style) return;
    // 绝不能在 document 上 append：document_start 时 head/documentElement 还没建出来，
    // 挂到 document 上的 <style> 会成为文档根元素，解析器随后建不出 html/body——
    // 整页白屏（2026-11-08 真机实锤：file:// 页面全白，http 页碰巧没中）。
    // 没有可挂的父节点就等后续机会（readystatechange / DOMContentLoaded 一定会来）。
    var parent = document.head || document.documentElement;
    if (!parent) return;
    style = document.createElement('style');
    style.id = 'pawch-light-scrollbar';
    style.textContent = CSS;
    parent.appendChild(style);
  }
  apply();
  document.addEventListener('DOMContentLoaded', apply);
  document.addEventListener('readystatechange', apply);
  try {
    window.matchMedia('(prefers-color-scheme: light)').addEventListener('change', apply);
  } catch (e) {
    try { window.matchMedia('(prefers-color-scheme: light)').addListener(apply); } catch (e2) {}
  }
})();"#;
/// 主 webview 的身份标记：给本壳窗口里所有 `/api/browser/*` 请求打上
/// `X-Agent-Client: shell`。服务端据此分辨「壳里那个客户端」和「用户在普通浏览器里
/// 开的第二个客户端」——浏览器客户端量的 x/y 是它自己窗口的坐标，不认出来的话会被
/// 当成壳窗口坐标去挪壳里的 child webview，页面整体漂到错误位置（真机实测「界面卡位」）。
/// **为什么走壳注入而不是前端字段**：前端可以只升级壳这一个页面，用户在浏览器里开着的
/// 那个页面还是旧版、不带标记，靠它自觉申报是拦不住的；壳注入对任何版本的前端都生效。
/// 幂等（页面刷新后重新 eval 一次即可，故按周期补打）。
const SHELL_CLIENT_JS: &str = r#"(function () {
  if (window.__agentShellClient) return;
  window.__agentShellClient = true;
  var hit = function (u) { try { return String(u).indexOf('/api/browser/') >= 0; } catch (e) { return false; } };
  var of = window.fetch;
  if (of) {
    window.fetch = function (u, o) {
      try {
        if (hit(u)) {
          o = o || {};
          var h = o.headers;
          if (h && typeof h.set === 'function') h.set('X-Agent-Client', 'shell');
          else o.headers = Object.assign({}, h, { 'X-Agent-Client': 'shell' });
        }
      } catch (e) {}
      return of.apply(this, arguments);
    };
  }
  var oo = XMLHttpRequest.prototype.open;
  XMLHttpRequest.prototype.open = function (m, u) {
    this.__agentApi = hit(u);
    return oo.apply(this, arguments);
  };
  var os = XMLHttpRequest.prototype.send;
  XMLHttpRequest.prototype.send = function () {
    try { if (this.__agentApi) this.setRequestHeader('X-Agent-Client', 'shell'); } catch (e) {}
    return os.apply(this, arguments);
  };
})();"#;
/// 「⋯」菜单浮层的 webview label（与 src/browser/shell.rs 的 MENU_LABEL 一致）
const MENU_LABEL: &str = "wv-menu";

/// 窗口控制 API 注入（无边框窗：原生黑条撤掉，─/▢/✕ 做进内容行）。
/// 与 SHELL_CLIENT_JS 同一周期补打机制——前端不依赖 npm 包，壳注入即用；
/// 浏览器里打开的第二个客户端没有这个 API，前端据此隐藏窗口按钮（不显示按了没反应的按钮）。
const WIN_CTRL_JS: &str = r#"(function () {
  if (window.agentWinApi) return;
  var inv = window.__TAURI_INTERNALS__ && window.__TAURI_INTERNALS__.invoke;
  if (!inv) return;
  try {
    window.agentWinApi = {
      minimize: function () { return inv('win_minimize', {}); },
      toggleMax: function () { return inv('win_toggle_maximize', {}); },
      hide: function () { return inv('win_hide', {}); },
      startResize: function (dir) { return inv('win_start_resize', { dir: dir }); },
      isMaximized: function () { return inv('win_is_maximized', {}); }
    };
    window.dispatchEvent(new Event('agentwinready'));
  } catch (e) {}
})();"#;

#[tauri::command]
fn win_minimize(win: tauri::Window) {
    let r = win.minimize();
    slog(&format!("win_minimize: result={:?}", r));
}

#[tauri::command]
fn win_toggle_maximize(win: tauri::Window) {
    let was_max = win.is_maximized().unwrap_or(false);
    let r = if was_max { win.unmaximize() } else { win.maximize() };
    slog(&format!(
        "win_toggle_maximize: label={} was_max={} size={:?} result={:?}",
        win.label(),
        was_max,
        win.outer_size().map(|s| (s.width, s.height)).ok(),
        r
    ));
}

/// 关闭 = 藏到托盘（与 CloseRequested 的处理同一语义：壳是 webview 宿主，一销毁
/// 浏览器宿主连同 CDP 端口一起没；服务独立生命周期，退出壳不动服务）。
#[tauri::command]
fn win_hide(win: tauri::Window) {
    let _ = win.hide();
}

/// 当前是否最大化（前端据此把按钮切成还原图标）
#[tauri::command]
fn win_is_maximized(win: tauri::Window) -> bool {
    let m = win.is_maximized().unwrap_or(false);
    slog(&format!("win_is_maximized: {m}"));
    m
}

/// 无边框窗边缘拉伸（Windows 没有原生 resize 边框，边缘热区按下时从这里起拖）。
#[tauri::command]
fn win_start_resize(win: tauri::Window, dir: String) {
    use ResizeDirection::*;
    let d = match dir.as_str() {
        "East" => East,
        "West" => West,
        "North" => North,
        "South" => South,
        "NorthEast" => NorthEast,
        "NorthWest" => NorthWest,
        "SouthEast" => SouthEast,
        "SouthWest" => SouthWest,
        _ => return,
    };
    let _ = win.start_resize_dragging(d);
}

/// 服务 exe 路径：先看壳**同级目录**（安装器把两个 exe 放在一起，安装版走这条），
/// 再按开发机布局 `<repo>/desktop/target/release/壳` → `<repo>/target/release/服务`。
///
/// 2026-11-02 安装实测修：原实现只认仓库布局（ancestors 第 4 个 + target/release），
/// 装到 `%LOCALAPPDATA%\Programs\Pawch Agent` 或 D 盘任意目录时 ancestors 根本没有
/// 第 4 个，回落到相对路径 `target/release/pawch-agent.exe` → 找不到服务、壳拉不起
/// 后端，窗口只能报 127.0.0.1 拒绝连接。
fn service_exe_for(exe: &Path) -> PathBuf {
    if let Some(dir) = exe.parent() {
        let p = dir.join("pawch-agent.exe");
        if p.is_file() {
            return p;
        }
    }
    // 开发布局：<repo>/desktop/target/release/<壳>
    let mut it = exe.ancestors();
    it.next(); // 壳 exe 本身
    let release = it.next().and_then(|p| p.file_name());
    let target = it.next().and_then(|p| p.file_name());
    let desktop = it.next().and_then(|p| p.file_name());
    let is_dev = release == Some(std::ffi::OsStr::new("release"))
        && target == Some(std::ffi::OsStr::new("target"))
        && desktop == Some(std::ffi::OsStr::new("desktop"));
    if is_dev {
        if let Some(repo) = exe.ancestors().nth(4) {
            return repo.join("target").join("release").join("pawch-agent.exe");
        }
    }
    // 兜底：同级（安装版语义；服务缺失时报这个路径更贴近实际安装位置）
    exe.parent()
        .map(|d| d.join("pawch-agent.exe"))
        .unwrap_or_else(|| PathBuf::from("pawch-agent.exe"))
}

fn service_exe() -> PathBuf {
    std::env::current_exe()
        .map(|e| service_exe_for(&e))
        .unwrap_or_else(|_| PathBuf::from("pawch-agent.exe"))
}

fn service_alive() -> bool {
    std::net::TcpStream::connect(SERVICE_URL).is_ok()
}

/// 探测服务，未起则后台拉起（CREATE_NO_WINDOW，不随壳退出）。
fn ensure_service() -> Option<u32> {
    if service_alive() {
        println!("[desktop] 服务已在运行");
        return None;
    }
    let exe = service_exe();
    if !exe.exists() {
        eprintln!(
            "[desktop] 服务未运行且未找到 {}（先在项目根 cargo build --release）",
            exe.display()
        );
        return None;
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let pid = match Command::new(&exe)
            // 2026-10-24 修：必须带 --minimized（后台子进程入口，隐藏控制台直接跑服务）。
            // 不带参数会落进「双击」语义——再 spawn 一个分离的 --minimized 子进程后还
            // 打开一次系统默认浏览器（冷启动壳会平白弹个浏览器窗口到用户脸上）。
            .arg("--minimized")
            // 2026-10-08 修：cwd 固定到 exe 所在目录（target/release），与服务自身
            // spawn_detached 的 current_dir(exe.parent()) 一致——原来多跳一级到
            // target/，两个拉起面的工作目录语义分叉（app_root 按 exe 路径兜住了主要
            // 资源，但任何显式读 cwd 的路径会拿错地方）。
            .current_dir(exe.parent().unwrap_or(Path::new(".")))
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn()
        {
            Ok(child) => {
                println!("[desktop] 已拉起服务 pid={}", child.id());
                Some(child.id())
            }
            Err(e) => {
                eprintln!("[desktop] 拉起服务失败: {e}");
                None
            }
        };
        // 2026-10-27 修：spawn 成功 ≠ 5173 已监听。原来立刻建窗口，webview 在服务
        // 起来之前加载 → 白屏/空会话，看起来像历史与执行轨迹全丢。这里最多等 20s，
        // 端口可连再开窗；超时也开（前端还有 waitForService/重试兜底）。
        if pid.is_some() {
            let deadline = std::time::Instant::now() + Duration::from_secs(20);
            while std::time::Instant::now() < deadline {
                if service_alive() {
                    println!("[desktop] 服务已就绪");
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
            if !service_alive() {
                eprintln!("[desktop] 服务 20 秒内未就绪，仍打开窗口（前端会提示重试）");
            }
        }
        pid
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// 壳日志（release 下无控制台，写文件兜底；只记 webview 宿主的诊断信息）。
fn slog(msg: &str) {
    let path = std::env::temp_dir().join("pawch-agent-shell.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let _ = writeln!(f, "[{now}] {msg}");
    }
}

/// tauri/wry 内部错误（如 CreateWebview 失败）默认走 log::error! 且无 logger=直接蒸发；
/// 接进 slog（2026-10-24 排查 child webview 静默失败时实测：add_child 的 Ok 只代表
/// 「已投递」，真失败在事件循环里只 log 一声，不装 logger 永远看不见）。
struct SlogLogger;
impl log::Log for SlogLogger {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Warn
    }
    fn log(&self, record: &log::Record) {
        if self.enabled(record.metadata()) {
            slog(&format!("[tauri:{}] {}", record.level(), record.args()));
        }
    }
    fn flush(&self) {}
}

fn install_logger() {
    let _ = log::set_boxed_logger(Box::new(SlogLogger));
    log::set_max_level(log::LevelFilter::Warn);
}

/// 单实例守卫：占住环回端口即视为本机唯一壳。
/// 端口被占时读 PID 锁判断「占用者是不是另一个壳」：是 → 本实例退出（两个壳会抢
/// 9223 CDP 端口，第二个的 webview 不可见也不可驱动）；不是（锁是死的/根本没有锁，
/// 9224 被无关程序占了）→ **放弃守卫继续启动**——不能因为一个无关端口把壳拦死。
/// 返回的 listener 必须保活（drop 即释放）。
/// 主窗口句柄（setup 时登记）：单实例守卫生效时，第二个壳靠它把已有窗口顶到前面。
static APP_HANDLE: std::sync::OnceLock<tauri::AppHandle> = std::sync::OnceLock::new();

/// 显示并聚焦主窗口（托盘左键/菜单「打开主窗口」与「第二个壳敲守卫端口」共用）。
fn show_main_window() {
    let Some(app) = APP_HANDLE.get() else { return };
    match app.get_webview_window("main") {
        Some(w) => {
            let _ = w.show();
            let _ = w.unminimize();
            let _ = w.set_focus();
        }
        // 窗口对象不在（被销毁/尚未建好）：静默返回 = 用户点托盘毫无反应
        // （2026-11-08 偶发实锤，疑似强杀壳的幽灵托盘图标+窗口销毁的组合）。
        // 留一行日志，下次复发有迹可查。
        None => slog("托盘打开：主窗口对象不在（已销毁或尚未建好）"),
    }
}

/// 壳自持文件清理（2026-11-03 待修项）：退出时删 %TEMP% 的单实例 lock 与诊断 log。
/// 只在「自己持有守卫」的退出路径调（托盘退出/服务守护退出/main 末尾）——第二个壳的
/// 提前退出绝不能删：lock 里是第一个壳的 PID，删了它的单实例守卫就瞎了。
fn cleanup_shell_files() {
    let _ = std::fs::remove_file(std::env::temp_dir().join("pawch-agent-shell.lock"));
    let _ = std::fs::remove_file(std::env::temp_dir().join("pawch-agent-shell.log"));
}

fn instance_guard() -> Option<TcpListener> {
    let lock = std::env::temp_dir().join("pawch-agent-shell.lock");
    // 先读后写：写在自己 bind 成功之后，否则第二个壳会把第一个的 PID 覆盖掉、把自己
    // 误判成"没有别的壳在跑"
    let other_pid = std::fs::read_to_string(&lock)
        .ok()
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|p| *p != std::process::id());
    let addr = ("127.0.0.1", GUARD_PORT);
    if let Ok(l) = TcpListener::bind(addr) {
        let _ = std::fs::write(&lock, std::process::id().to_string());
        // 守卫端口同时当「打开主窗口」的接收端：第二个壳连上来就是一句「把窗口顶出来」。
        // 没有这一段，用户双击桌面图标时第二个壳静默 exit(0)——窗口藏在托盘/被别的
        // 窗口盖住，看着就是「点不开」（2026-11-01 真机实测）。
        if let Ok(acc) = l.try_clone() {
            thread::spawn(move || {
                for _ in acc.incoming() {
                    show_main_window();
                }
            });
        }
        return Some(l);
    }
    if other_pid.map(pid_alive).unwrap_or(false) {
        // 已有壳在跑：先请求它显示窗口，再退出——用户点图标 = 打开窗口，不是没反应
        if let Ok(mut s) = TcpStream::connect(addr) {
            let _ = s.write_all(b"show\n");
            let _ = s.flush();
        }
        slog("另一个 Pawch Agent 壳已在运行，已请求它显示窗口，本实例退出");
        println!("[desktop] 另一个 Pawch Agent 壳已在运行（已请求显示窗口）");
        std::process::exit(0);
    }
    slog(&format!("单实例端口 {GUARD_PORT} 被非本程序占用，放弃守卫继续启动"));
    TcpListener::bind(addr).ok()
}

/// PID 是否活着（tasklist 探活；只在第二个壳启动的罕见路径上跑）。
fn pid_alive(pid: u32) -> bool {
    use std::os::windows::process::CommandExt;
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .creation_flags(0x0800_0000)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).contains(&pid.to_string()))
        .unwrap_or(true)
}

/// child webview 注册表（label → webview 句柄）。
#[derive(Default)]
struct WebviewHost {
    tabs: Mutex<HashMap<String, tauri::Webview<tauri::Wry>>>,
}

impl WebviewHost {
    fn get(&self, label: &str) -> Option<tauri::Webview<tauri::Wry>> {
        self.tabs.lock().ok()?.get(label).cloned()
    }
    fn insert(&self, label: String, wv: tauri::Webview<tauri::Wry>) {
        if let Ok(mut t) = self.tabs.lock() {
            t.insert(label, wv);
        }
    }
    fn remove(&self, label: &str) -> Option<tauri::Webview<tauri::Wry>> {
        self.tabs.lock().ok()?.remove(label)
    }
}

/// 控制通道客户端：连上服务端 9225，按行收 JSON 指令并执行；断线 1s 重连。
fn spawn_ctl_client(app: tauri::AppHandle) {
    thread::spawn(move || {
        let (host, port) = CTL_ADDR;
        loop {
            if let Ok(sock) = TcpStream::connect((host, port)) {
                slog(&format!("控制通道已连接 {host}:{port}"));
                let mut w = sock.try_clone().ok();
                if let Some(w) = w.as_mut() {
                    let hello = json!({ "op": "hello", "pid": std::process::id(), "cdp": CDP_PORT });
                    let _ = w.write_all(format!("{hello}\n").as_bytes());
                }
                let mut r = BufReader::new(sock);
                let mut line = String::new();
                loop {
                    line.clear();
                    match r.read_line(&mut line) {
                        Ok(0) => break, // 服务端关了通道
                        Ok(_) => {
                            let t = line.trim();
                            if t.is_empty() {
                                continue;
                            }
                            match serde_json::from_str::<Value>(t) {
                                Ok(op) => dispatch(&app, &op),
                                Err(e) => slog(&format!("指令解析失败: {e} <- {t}")),
                            }
                        }
                        Err(_) => break,
                    }
                }
                slog("控制通道断开，1s 后重连");
            }
            thread::sleep(Duration::from_secs(1));
        }
    });
}

/// 控制指令的取值助手（省掉每个分支里重复的 op.get().and_then().unwrap_or()）。
fn op_str<'a>(op: &'a Value, key: &str, default: &'a str) -> &'a str {
    op.get(key).and_then(Value::as_str).unwrap_or(default)
}
fn op_f64(op: &Value, key: &str, default: f64) -> f64 {
    op.get(key).and_then(Value::as_f64).unwrap_or(default)
}
fn op_bool(op: &Value, key: &str, default: bool) -> bool {
    op.get(key).and_then(Value::as_bool).unwrap_or(default)
}
/// 背景色字段（[r,g,b]；缺省 = 不设置，WebView2 自己的画布色跟系统深浅色走——
/// 系统深色时 about:blank 等空白页就是黑的，应用浅色主题下必须显式给亮色）。
fn op_bg(op: &Value) -> Option<tauri::utils::config::Color> {
    op.get("bg")
        .and_then(Value::as_array)
        .filter(|a| a.len() == 3)
        .map(|a| {
            let v = |i: usize| a.get(i).and_then(Value::as_u64).unwrap_or(0) as u8;
            tauri::utils::config::Color(v(0), v(1), v(2), 255)
        })
}
/// 尺寸字段（宽高有下限，防 0 尺寸的 webview）。
fn op_size(op: &Value) -> (f64, f64) {
    (
        op_f64(op, "w", 900.0).max(64.0),
        op_f64(op, "h", 600.0).max(64.0),
    )
}

/// 现存的资源管理器窗口（CabinetWClass，且可见）。
fn cabinet_windows() -> Vec<isize> {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    struct Ctx {
        out: Vec<isize>,
    }
    unsafe extern "system" fn cb(hwnd: isize, lparam: isize) -> i32 {
        let ctx = &mut *(lparam as *mut Ctx);
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut cls = [0u16; 64];
            let n = GetClassNameW(hwnd, cls.as_mut_ptr(), cls.len() as i32);
            if n > 0 && String::from_utf16_lossy(&cls[..n as usize]) == "CabinetWClass" {
                ctx.out.push(hwnd);
            }
        }
        1
    }
    let mut ctx = Ctx { out: Vec::new() };
    unsafe {
        EnumWindows(Some(cb), &mut ctx as *mut _ as isize);
    }
    ctx.out
}

fn window_title(hwnd: isize) -> String {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32) };
    if n <= 0 {
        return String::new();
    }
    String::from_utf16_lossy(&buf[..n as usize])
}

/// 找一个这次新出现的资源管理器窗口：先按标题（文件夹名）精确匹配，找不到就取任意新窗口。
fn find_new_cabinet(before: &[isize], title: &str) -> Option<isize> {
    let now = cabinet_windows();
    for h in &now {
        if !before.contains(h) && window_title(*h) == title {
            return Some(*h);
        }
    }
    now.into_iter().find(|h| !before.contains(h))
}

/// 由壳（前台进程）打开一个目录，并把新窗口顶到前台。
/// 后台进程（服务）自己拉起的资源管理器窗口会落在应用后面——真机实测。
fn open_path_foreground(path: &str) {
    use std::os::windows::process::CommandExt;
    use windows_sys::Win32::UI::WindowsAndMessaging::{BringWindowToTop, SetForegroundWindow};
    let before = cabinet_windows();
    if Command::new("explorer.exe")
        .arg(path)
        .creation_flags(0)
        .spawn()
        .is_err()
    {
        slog(&format!("open {path} 失败：explorer 起不来"));
        return;
    }
    let title = Path::new(path)
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    for _ in 0..20 {
        std::thread::sleep(Duration::from_millis(100));
        if let Some(hwnd) = find_new_cabinet(&before, &title) {
            unsafe {
                SetForegroundWindow(hwnd);
                BringWindowToTop(hwnd);
            }
            return;
        }
    }
}

/// 执行一条控制指令。create 由本（非主）线程发起：add_child 内部
/// run_on_main_thread + recv，在主线程调用会死锁。
fn dispatch(app: &tauri::AppHandle, op: &Value) {
    let kind = op_str(op, "op", "");
    let label = op_str(op, "label", "").to_string();
    if !matches!(kind, "hello" | "reset") && label.is_empty() {
        return;
    }
    let host = app.state::<WebviewHost>();
    match kind {
        "open" => {
            // 服务进程是后台进程，它拉起的资源管理器窗口会落在应用后面；改由壳
            // （前台进程，有 SetForegroundWindow 权限）来开，并主动顶到前台。
            let path = op_str(op, "path", "").to_string();
            if path.is_empty() {
                return;
            }
            std::thread::spawn(move || open_path_foreground(&path));
        }
        "create" => {
            let url = op_str(op, "url", "about:blank");
            let (x, y) = (op_f64(op, "x", 0.0), op_f64(op, "y", 0.0));
            let (w, h) = op_size(op);
            let visible = op_bool(op, "visible", false);
            let Some(win) = app.get_window("main") else {
                slog("create 失败：没有 main 窗口");
                return;
            };
            // 同名 label 先关旧的（服务侧重试时会出现）
            if let Some(old) = host.remove(&label) {
                let _ = old.close();
            }
            let Ok(parsed) = url.parse::<tauri::Url>() else {
                slog(&format!("create 失败：URL 解析不了 {url}"));
                return;
            };
            let mut builder = WebviewBuilder::new(&label, WebviewUrl::External(parsed))
                .additional_browser_args(BROWSER_ARGS)
                .initialization_script(POPUP_GUARD_JS)
                .initialization_script(SCROLLBAR_THEME_JS)
                .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny);
            // 空白页（about:blank/未落地的页面）画布色跟主题走，别让系统深色把
            // 浅色主题的网页面板衬出一块黑（2026-11-07 用户实锤）
            let bg = op_bg(op);
            if let Some(c) = bg {
                builder = builder.background_color(c);
                // 窗口主题跟**应用**主题（不是系统）：child webview 的
                // PreferredColorScheme 继承窗口主题——系统深色时 about:blank 这类
                // 没有自身背景色的页面会被 UA 直接画成 #121212 深色画布，
                // background_color 只在页面不画的地方生效，根本轮不到（真机 2026-11-07
                // CDP 截图实锤）。set_theme 同时经 ThemeChanged 事件同步全部存量 webview。
                let theme =
                    if c.0 > 128 { tauri::Theme::Light } else { tauri::Theme::Dark };
                let _ = win.set_theme(Some(theme));
            }
            match win.add_child(builder, LogicalPosition::new(x, y), LogicalSize::new(w, h)) {
                Ok(wv) => {
                    if !visible {
                        let _ = wv.hide();
                    }
                    host.insert(label.clone(), wv);
                    slog(&format!(
                        "create {label} {url} {w}x{h} visible={visible} bg={:?}",
                        bg.map(|c| [c.0, c.1, c.2])
                    ));
                }
                Err(e) => slog(&format!("create {label} 失败: {e}")),
            }
        }
        "reset" => {
            // 服务端会话重建：清掉全部残留 child webview（上一个会话的，不清会常驻泄漏）
            let labels: Vec<String> = {
                let g = host.tabs.lock().unwrap_or_else(|e| e.into_inner());
                g.keys().cloned().collect()
            };
            let n = labels.len();
            for l in labels {
                if let Some(wv) = host.remove(&l) {
                    let _ = wv.close();
                }
            }
            slog(&format!("reset：关了 {n} 个残留 webview"));
        }
        "close" => {
            if let Some(wv) = host.remove(&label) {
                let _ = wv.close();
                slog(&format!("close {label}"));
            }
        }
        "rect" => {
            let (x, y) = (op_f64(op, "x", 0.0), op_f64(op, "y", 0.0));
            let (w, h) = op_size(op);
            if let Some(wv) = host.get(&label) {
                let _ = wv.set_position(LogicalPosition::new(x, y));
                let _ = wv.set_size(LogicalSize::new(w, h));
                slog(&format!("rect {label} {x},{y} {w}x{h}"));
            }
        }
        "visible" => {
            let visible = op_bool(op, "visible", false);
            if let Some(wv) = host.get(&label) {
                if let Err(e) = if visible { wv.show() } else { wv.hide() } {
                    slog(&format!("visible {label}={visible} 失败: {e}"));
                } else {
                    slog(&format!("visible {label}={visible} ok"));
                }
            }
        }
        "devtools" => {
            let open = op_bool(op, "open", true);
            if let Some(wv) = host.get(&label) {
                if open {
                    wv.open_devtools();
                } else {
                    wv.close_devtools();
                }
                slog(&format!("devtools {label} open={open}"));
            }
        }
        "menu" => {
            // 「⋯」菜单浮层（补丁四）：透明置顶的 child webview 铺在面板矩形上，
            // 菜单页由服务同源提供（URL query 带锚点与 tab）。它压在下层内容 webview
            // 之上——菜单因此浮在网页上面，网页纹丝不动（HTML 菜单做不到：HWND 永远
            // 盖在主界面之上）。按需创建 = 最新的子窗口 = 天然置顶；同名先关旧的。
            let Some(win) = app.get_window("main") else {
                slog("menu 失败：没有 main 窗口");
                return;
            };
            if let Some(old) = host.remove(MENU_LABEL) {
                let _ = old.close();
            }
            let (x, y) = (op_f64(op, "x", 0.0), op_f64(op, "y", 0.0));
            let (w, h) = op_size(op);
            // 锚点换算成相对面板左上角（webview 内坐标）
            let (ax, ay) = (op_f64(op, "ax", 0.0) - x, op_f64(op, "ay", 0.0) - y);
            let (aw, ah) = (op_f64(op, "aw", 26.0), op_f64(op, "ah", 26.0));
            let tab = op_str(op, "tab", "");
            // 窗口主题对齐**当前**应用主题：主题是窗口级设置，浅色时代建过网页会把
            // 窗口主题卡在 Light，之后切深色 → 菜单/DevTools 全跟浅色（真机 2026-11-08）。
            // 顺手随 URL 传给菜单页——比菜单页自己读 localStorage 更权威（无时序差）。
            let theme = op_str(op, "theme", "dark");
            let _ = win.set_theme(Some(if theme == "light" { tauri::Theme::Light } else { tauri::Theme::Dark }));
            let url = format!(
                "http://{SERVICE_URL}/menu.html?ax={ax}&ay={ay}&aw={aw}&ah={ah}&tab={tab}&theme={theme}"
            );
            let Ok(parsed) = url.parse::<tauri::Url>() else {
                slog("menu 失败：URL 解析不了");
                return;
            };
            let builder = WebviewBuilder::new(MENU_LABEL, WebviewUrl::External(parsed))
                .additional_browser_args(BROWSER_ARGS)
                .transparent(true)
                .initialization_script(POPUP_GUARD_JS)
                .on_new_window(|_url, _features| tauri::webview::NewWindowResponse::Deny);
            match win.add_child(builder, LogicalPosition::new(x, y), LogicalSize::new(w, h)) {
                Ok(wv) => {
                    let _ = wv.set_focus();
                    host.insert(MENU_LABEL.to_string(), wv);
                    slog(&format!("menu {MENU_LABEL} {x},{y} {w}x{h} anchor={ax},{ay}"));
                }
                Err(e) => slog(&format!("menu {MENU_LABEL} 失败: {e}")),
            }
        }
        "menu_close" => {
            if let Some(wv) = host.remove(MENU_LABEL) {
                let _ = wv.close();
                slog(&format!("menu_close {MENU_LABEL}"));
            }
        }
        "hello" => {}
        other => slog(&format!("未知指令: {other}")),
    }
}

/// 默认数据目录（与服务的 config::default_data_root 同口径）：
/// `%USERPROFILE%\.pawch-agent`（`.pawch-agent` 就是数据目录）；没有用户目录时
/// 回落 exe 同级 `.pawch-agent`。
fn default_data_dir() -> PathBuf {
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) {
        if !home.is_empty() {
            return PathBuf::from(home).join(".pawch-agent");
        }
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".pawch-agent")
}

/// config.toml 位置：exe 同级（打包形态）→ 上三级（开发形态 desktop/target/release）。
fn config_path() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let direct = dir.join("config.toml");
    if direct.is_file() {
        return Some(direct);
    }
    let repo = exe.ancestors().nth(3)?;
    let p = repo.join("config.toml");
    p.is_file().then_some(p)
}

fn read_data_dir(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let doc = text.parse::<toml_edit::DocumentMut>().ok()?;
    let v = doc.get("paths")?.get("data_dir")?.as_str()?.trim().to_string();
    (!v.is_empty()).then_some(v)
}

/// 首次选择的持久化指针（与服务 data_dir_pointer 同路径）：
/// `%LOCALAPPDATA%\Pawch Agent\.data-dir`。不写 config.toml——开发仓库里那是受版本
/// 管理的文件，写进本机绝对路径会弄脏它；安装目录只读时指针也照样能记住。
fn data_dir_pointer() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(default_data_dir);
    base.join("Pawch Agent").join(".data-dir")
}

fn read_data_dir_pointer() -> Option<PathBuf> {
    let text = std::fs::read_to_string(data_dir_pointer()).ok()?;
    let p = text.trim();
    (!p.is_empty()).then(|| PathBuf::from(p))
}

fn write_data_dir_pointer(dir: &Path) -> Result<(), String> {
    let p = data_dir_pointer();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(p, dir.display().to_string()).map_err(|e| e.to_string())
}

/// 数据目录解析：环境变量 > config.toml [paths].data_dir（手工覆盖）> 首次选择指针
/// > 首次启动让用户选（默认建议 `%USERPROFILE%\.pawch-agent`）。
fn resolve_data_dir() -> PathBuf {
    if let Some(p) = std::env::var_os("PAWCH_AGENT_DATA_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let cfg = config_path();
    if let Some(path) = &cfg {
        if let Some(d) = read_data_dir(path) {
            return PathBuf::from(d);
        }
    }
    if let Some(p) = read_data_dir_pointer() {
        return p;
    }
    let default = default_data_dir();
    // 首次启动：让用户选（等价于安装时的目录选择页）。取消 = 用默认建议。
    let picked = rfd::FileDialog::new()
        .set_title("Pawch Agent —— 选择数据目录（设置、对话、日志、插件、缓存都会存在这里）")
        .set_directory(default.parent().unwrap_or(&default))
        .pick_folder()
        .unwrap_or_else(|| default.clone());
    if let Err(e) = write_data_dir_pointer(&picked) {
        slog(&format!("写数据目录指针失败（{e}），本次目录下次启动会再问一次"));
    }
    picked
}

fn main() {
    // 应用数据目录：先于一切（WebView2 缓存与服务都跟它走）。
    // WEBVIEW2_USER_DATA_FOLDER 是 WebView2 官方认的环境变量：壳窗口由 tauri.conf.json
    // 声明创建（没有 builder 钩子），用它把缓存收进 <数据目录>\cache\WebView2。
    let data_dir = resolve_data_dir();
    let _ = std::fs::create_dir_all(&data_dir);
    let webview_cache = data_dir.join("cache").join("WebView2");
    let _ = std::fs::create_dir_all(&webview_cache);
    std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview_cache);
    // 服务从 config.toml 读同一路径；环境变量兜底（config.toml 写不进去时也不分叉）
    std::env::set_var("PAWCH_AGENT_DATA_DIR", &data_dir);

    // tauri/wry 内部错误接进 slog（事件循环里 CreateWebview 失败只 log::error!，无 logger 会蒸发
    // ——2026-10-24 内置浏览器 P0 排查确认了这个盲区，错误码从此有尸检材料）
    install_logger();
    // 单实例：两个壳会抢 9223，第二个直接退出（listener 需保活）
    let _guard = instance_guard();

    // 服务保活先于窗口（窗口加载 5173，服务没起会白屏）
    ensure_service();

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            win_minimize,
            win_toggle_maximize,
            win_hide,
            win_start_resize,
            win_is_maximized
        ])
        .setup(|app| {
            // 先登记句柄：守卫线程收到「第二个壳」的连接时要用它把窗口顶出来
            let _ = APP_HANDLE.set(app.handle().clone());
            app.manage(WebviewHost::default());
            // 关窗 = 藏到托盘（不退出）：壳进程是 child webview 的宿主，窗口一销毁
            // 浏览器宿主连同 CDP 端口一起没——用户习惯点 X，不能一关就没了
            if let Some(win) = app.get_webview_window("main") {
                // 窗口自带图标（WM_SETICON）：不设的话任务栏会去「按 exe 路径查图标缓存」，
                // exe 换了新图标它还显示旧的（2026-11-01 真机实测：任务栏一直是蓝方块，
                // 而 exe 里已经是四指橙图）。图标取编译期嵌进二进制的 app 图标。
                if let Some(icon) = app.default_window_icon() {
                    let _ = win.set_icon(icon.clone());
                }
                let w = win.clone();
                win.on_window_event(move |ev| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = ev {
                        api.prevent_close();
                        let _ = w.hide();
                    }
                });
                // 身份标记补打：壳窗口里的前端页面是启动时加载的，用户可能在浏览器里
                // 还开着一个旧版页面——两者都往服务端报面板矩形，服务端只认带壳标记的那个。
                // 页面刷新会丢掉注入，故按周期重打（脚本幂等，重复执行零副作用）。
                let marker = win.clone();
                std::thread::spawn(move || loop {
                    let _ = marker.eval(SHELL_CLIENT_JS);
                    let _ = marker.eval(WIN_CTRL_JS);
                    std::thread::sleep(std::time::Duration::from_secs(2));
                });
            }
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};

            let open = MenuItem::with_id(app, "open", "打开", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&open, &quit])?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().expect("缺窗口图标").clone())
                .tooltip("Pawch Agent（127.0.0.1:5173）")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, ev| match ev.id.as_ref() {
                    "open" => show_main_window(),
                    "quit" => {
                        cleanup_shell_files();
                        app.exit(0)
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = ev
                    {
                        show_main_window();
                    }
                })
                .build(app)?;

            // 控制通道客户端（连上服务端 9225；服务没起时每秒重试）
            spawn_ctl_client(app.handle().clone());

            // 服务守护：服务进程没了（stop.bat / 强杀 / 崩溃）→ 壳优雅退出。
            // 强杀壳会给 Windows 托盘留幽灵图标—— explorer 一直显示到鼠标划过才清
            // （2026-09-29 真机实测「每次重启服务托盘多出很多个 agent」）。优雅退出
            // 让 Tauri 自己 NIM_DELETE 托盘图标，一个都不留。5s 宽限期：容纳快速
            // stop→start 的重启（服务只是短暂不在）；真没了就退，start.bat 会带新的来。
            {
                let guard_app = app.handle().clone();
                std::thread::spawn(move || {
                    let mut dead_since: Option<std::time::Instant> = None;
                    loop {
                        std::thread::sleep(std::time::Duration::from_secs(1));
                        if service_alive() {
                            dead_since = None;
                            continue;
                        }
                        let since = *dead_since.get_or_insert_with(std::time::Instant::now);
                        if since.elapsed() >= std::time::Duration::from_secs(5) {
                            slog("服务已停止超过 5s，壳优雅退出（清理托盘图标）");
                            cleanup_shell_files();
                            guard_app.exit(0);
                            return;
                        }
                    }
                });
            }
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Pawch Agent 桌面壳启动失败");
    // 正常退出路径（事件循环返回）也收掉 lock/log
    cleanup_shell_files();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "pawch-shell-test-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn service_exe_prefers_sibling_in_install_layout() {
        let base = tmp("installed");
        let shell = base.join("Pawch Agent.exe");
        let svc = base.join("pawch-agent.exe");
        std::fs::write(&shell, b"x").unwrap();
        std::fs::write(&svc, b"x").unwrap();
        assert_eq!(service_exe_for(&shell), svc);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn service_exe_dev_layout_uses_repo_target_release() {
        let base = tmp("dev");
        let shell = base
            .join("desktop")
            .join("target")
            .join("release")
            .join("Pawch Agent.exe");
        std::fs::create_dir_all(shell.parent().unwrap()).unwrap();
        std::fs::write(&shell, b"x").unwrap();
        let want = base.join("target").join("release").join("pawch-agent.exe");
        assert_eq!(service_exe_for(&shell), want);
        let _ = std::fs::remove_dir_all(&base);
    }
}
