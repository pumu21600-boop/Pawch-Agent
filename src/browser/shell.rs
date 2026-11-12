//! 桌面壳 webview 控制通道—— 服务端是指令方，壳是执行方。
//!
//! 内置浏览器搬到 Tauri 壳的 child webview 后，**标签页生命周期的唯一 owner 仍是
//! 本服务**（agent 工具与网页面板共享同一份 tab 状态）。壳通过一条环回 TCP 通道
//! （127.0.0.1:9225，换行分隔 JSON）连进来，服务向下发五类指令：
//!
//! | op | 语义 |
//! |---|---|
//! | `create` | 建 child webview（label/url/矩形/初始显隐） |
//! | `close` | 关 child webview |
//! | `rect` | 移动+缩放（跟随网页面板内容区） |
//! | `visible` | 显示/隐藏（面板切走即隐藏，免得浮层盖住 UI） |
//! | `devtools` | 开/关真 DevTools（child webview 支持，补上无头时代的缺口） |
//! | `menu` | 开「⋯」菜单：建透明置顶的菜单 webview 浮在网页上（页面不动） |
//! | `menu_close` | 关菜单 webview |
//!
//! 设计约束：
//! - **壳 → 服务的唯一上行消息是 `focus`**（窗口「看得见/看不见」变化；2026-11-09 加）：
//!   看不见（被完全遮挡/最小化）时冻结页面动画省渲染，露出来立即恢复——窗口内容
//!   保持最后一帧，切回来无缝，观感对齐 ZCode（它靠 Chromium 自身的遮挡节流）。
//!   其余指令单向，create 的确认靠 /json/list 里出现新 target。
//! - **单连接**：只认第一个连进来的壳（单实例守卫下只有一个）；第二个连接直接关掉，
//!   防止两个壳互相顶替。
//! - 通道断连只意味着"指挥不动壳了"：`available()` 转假，浏览器会话回落到无头模式，
//!   不影响其他任何功能。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use serde_json::{Value, json};

/// 壳 WebView2 环境的 CDP 端口（tauri.conf.json 的 additionalBrowserArguments 同值）。
pub const SHELL_CDP_PORT: u16 = 9223;
/// 控制通道监听端口（壳来连）。
pub const SHELL_CTL_PORT: u16 = 9225;

struct Conn {
    tx: mpsc::UnboundedSender<String>,
}

static CONN: OnceLock<Mutex<Option<Conn>>> = OnceLock::new();

fn slot() -> &'static Mutex<Option<Conn>> {
    CONN.get_or_init(|| Mutex::new(None))
}

/// 壳是否连着（能指挥 webview 的前提）。
pub fn available() -> bool {
    slot().lock().map(|g| g.is_some()).unwrap_or(false)
}

/// 壳主窗口是否「看得见」（缺省 true）：未最小化、未被前台窗口完全盖住。
/// 部分可见（侧并排）时页面照常渲染，跟 ZCode 一致；「看不见」时冻结页面动画
/// （rAF 停住，最后一帧留在屏上，切回来无缝，2026-11-09）。
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

pub fn window_visible() -> bool {
    WINDOW_VISIBLE.load(Ordering::Relaxed)
}

fn set_window_visible(v: bool) {
    WINDOW_VISIBLE.store(v, Ordering::Relaxed);
}

/// 手动禁用 shell 模式（CDP 探测失败等自愈路径；下次壳重连自动恢复）。
pub fn disable() {
    if let Ok(mut g) = slot().lock() {
        *g = None;
    }
}

/// 起控制通道监听（服务启动时调一次；端口被占只记日志，不影响主服务）。
///
/// 单连接语义：`slot()` 的 compare-and-set 在锁内完成——已有时新连接直接放掉，
/// 不会出现两个壳互相顶替；读任务断开时注销，壳重连自然恢复。
pub fn start_listener() {
    tokio::spawn(async move {
        let listener = match TcpListener::bind(("127.0.0.1", SHELL_CTL_PORT)).await {
            Ok(l) => l,
            Err(e) => {
                crate::observability::debug(&format!(
                    "[shell] 控制通道监听失败 {SHELL_CTL_PORT}: {e}（内置浏览器回退无头模式）"
                ));
                return;
            }
        };
        crate::observability::debug(&format!("[shell] 控制通道监听 127.0.0.1:{SHELL_CTL_PORT}"));
        loop {
            let Ok((sock, _)) = listener.accept().await else { continue };
            let (tx, mut rx) = mpsc::unbounded_channel::<String>();
            let ctl_tx = tx.clone();
            {
                let mut g = slot().lock().unwrap_or_else(|e| e.into_inner());
                if g.is_some() {
                    continue; // 已有一个壳连着，放掉新的（防互相顶替）
                }
                *g = Some(Conn { tx });
            }
            crate::observability::debug("[shell] 壳已接入控制通道");
            // 服务（重）启动后壳第一次接入：本进程还没有浏览器会话，壳里挂着的 child
            // webview 全是上一任服务留下的孤儿（服务重启/停止不会关它们）——立即清掉。
            // 不清的话一条旧页面会一直悬在面板/聊天列上，谁也不管它（重启后
            // chrome-error 页会浮在错误位置）。会话活着时不清（只是通道闪断重连，
            // webview 还归当前会话管，清了反而闪没）。
            if super::SESSION.lock().await.is_none() {
                let _ = ctl_tx.send(format!("{}\n", json!({ "op": "reset" })));
            }
            let (rd, mut wr) = tokio::io::split(sock);
            // 写任务：指令队列 → 套接字
            tokio::spawn(async move {
                while let Some(line) = rx.recv().await {
                    if wr.write_all(line.as_bytes()).await.is_err() {
                        break;
                    }
                }
            });
            // 读任务：只消费 hello/探活，断开即注销
            tokio::spawn(async move {
                let mut r = BufReader::new(rd);
                let mut line = String::new();
                loop {
                    line.clear();
                    match r.read_line(&mut line).await {
                        Ok(0) => break,
                        Ok(_) => {
                            if let Ok(v) = serde_json::from_str::<Value>(line.trim()) {
                                match v.get("op").and_then(Value::as_str) {
                                    Some("hello") => {
                                        crate::observability::debug(&format!("[shell] 壳 hello: {}", line.trim()));
                                    }
                                    // 壳窗口可见性变化（被完全遮挡/最小化 ↔ 露出来了）：
                                    // 立即冻结/恢复页面动画（不等 5s 巡检），语义见 WINDOW_VISIBLE。
                                    Some("focus") => {
                                        let visible = v.get("focused").and_then(Value::as_bool).unwrap_or(true);
                                        set_window_visible(visible);
                                        crate::observability::debug(&format!("[shell] 窗口可见 visible={visible}"));
                                        let session = {
                                            let g = super::SESSION.lock().await;
                                            g.clone()
                                        };
                                        if let Some(s) = session {
                                            super::sweep_session(&s).await;
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                        Err(_) => break,
                    }
                }
                disable();
                crate::observability::debug("[shell] 壳断开控制通道");
            });
        }
    });
}

/// 发一条指令（壳没连时静默丢弃——调用方按无头模式走）。
fn send(op: &Value) {
    let _ = send_checked(op);
}

/// 发指令并返回是否真的发出去了（壳没连 = false）。
fn send_checked(op: &Value) -> bool {
    let tx = {
        let g = slot().lock().unwrap_or_else(|e| e.into_inner());
        g.as_ref().map(|c| c.tx.clone())
    };
    match tx {
        Some(tx) => tx.send(format!("{op}\n")).is_ok(),
        None => false,
    }
}

/// 让**壳**（前台进程）打开一个目录：资源管理器窗口由前台进程拉起才能到前台。
/// 返回 false = 壳没连（调用方回退自己起 explorer）。
pub fn open_path(path: &std::path::Path) -> bool {
    send_checked(&json!({
        "op": "open",
        "label": "open",
        "path": path.display().to_string(),
    }))
}

/// 建 child webview。label 由服务端生成（tab 身份的一部分）；rect = (x, y, w, h)。
/// bg = 画布底色 [r,g,b]（跟应用主题）：不设的话 WebView2 画布跟系统深浅色走，
/// 系统深色时 about:blank 等空白页在浅色主题下就是一块黑（2026-11-07 实锤）。
pub fn create(label: &str, url: &str, rect: (f64, f64, f64, f64), visible: bool, bg: Option<[u8; 3]>) {
    let (x, y, w, h) = rect;
    let mut op = json!({
        "op": "create", "label": label, "url": url,
        "x": x, "y": y, "w": w, "h": h, "visible": visible
    });
    if let Some([r, g, b]) = bg {
        op["bg"] = json!([r, g, b]);
    }
    send(&op);
}

/// 应用主题（"light"/"dark"，来自前端上报的 theme_actual；没报过按默认深色）。
pub fn app_theme_actual() -> &'static str {
    let theme = crate::server::ui_events::last_state()
        .and_then(|s| s.get("theme_actual").and_then(Value::as_str).map(str::to_string))
        .unwrap_or_else(|| "dark".into());
    if theme == "light" { "light" } else { "dark" }
}

/// 应用主题对应的 webview 画布色（浅色 = 面板底色 g1（#fffefc）；深色 = g1（#1c1a18））。
pub fn themed_webview_bg() -> Option<[u8; 3]> {
    Some(if app_theme_actual() == "light" { [255, 254, 252] } else { [28, 26, 24] })
}

/// 清空壳里全部 child webview（服务端会话重建时调用——壳不重启时上一个会话的
/// webview 还活着，不清会一直泄漏常驻）。
pub fn reset() {
    send(&json!({ "op": "reset" }));
}

pub fn close(label: &str) {
    send(&json!({ "op": "close", "label": label }));
}

/// 开「⋯」菜单：壳建一个**透明置顶**的 child webview（label 固定 wv-menu，页面由服务
/// 同源提供 http://127.0.0.1:5173/menu.html），铺在面板矩形上——它压在下层内容 webview
/// 之上，菜单因此能浮在网页上面（页面纹丝不动）。anchor 是「⋯」按钮相对面板
/// 左上角的矩形，tab 是当前可见标签（菜单项要操作的那一页）。按需创建=最新的子窗口=
/// 天然置顶。URL 由壳用自己的 SERVICE_URL 拼（不依赖服务的端口配置）。
pub fn menu(x: f64, y: f64, w: f64, h: f64, anchor: (f64, f64, f64, f64), tab: &str) {
    send(&json!({
        "op": "menu", "label": MENU_LABEL,
        "x": x, "y": y, "w": w, "h": h,
        "ax": anchor.0, "ay": anchor.1, "aw": anchor.2, "ah": anchor.3,
        "tab": tab,
        // 当前应用主题：壳用它校正窗口主题（防止浅色时代建过网页把窗口主题
        // 卡在 Light，之后切深色菜单/DevTools 全跟浅色），并随 URL 传给菜单页
        "theme": app_theme_actual()
    }));
}

/// 关菜单 webview（点了菜单项/backdrop/切走面板）。
pub fn menu_close() {
    send(&json!({ "op": "menu_close", "label": MENU_LABEL }));
}

/// 菜单浮层的 webview label（壳侧同名常量）。
pub const MENU_LABEL: &str = "wv-menu";

pub fn rect(label: &str, x: f64, y: f64, w: f64, h: f64) {
    send(&json!({ "op": "rect", "label": label, "x": x, "y": y, "w": w, "h": h }));
}

pub fn visible(label: &str, visible: bool) {
    send(&json!({ "op": "visible", "label": label, "visible": visible }));
}

pub fn devtools(label: &str, open: bool) {
    send(&json!({ "op": "devtools", "label": label, "open": open }));
}
