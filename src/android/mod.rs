//! 安卓模拟器能力（内置插件「安卓」）。
//!
//! 画面源：模拟器自带的 EmulatorController gRPC（proto vendor 在
//! `src/android/proto/`）。这个版本的契约里**没有 H.264 streamVideo**，
//! 帧流是 `streamScreenshot(PNG)`，且**变化驱动**——静止画面不推帧（实测），
//! 静止时面板拿 LATEST 最后一帧即可，CPU 近乎为零；滚动时约 13fps。
//! `ImageFormat.width/height` 缩放参数被模拟器忽略（实测），一律全分辨率帧。
//!
//! 输入：`streamInputEvent`（client-streaming，按序送达）是**唯一可靠的点击
//! 通道**——弃用的 `sendTouch` 单调用只出按压涟漪不触发点击（P0 实测）；
//! down/up 之间要留 hold（80ms），立即连发会被吞。中文文本走剪贴板（`clip`），
//! ASCII 走 `sendKey{text}`，Android keycode 走 adb keyevent。
//!
//! 进程管理：emulator.exe（launcher）+ qemu 子进程，挂 Job Object
//! （jobguard）整树杀；`adb emu kill` 优雅停，不退再强杀。
//!
//! SDK 路径是显式配置（settings android.sdk_path → 环境变量 → 常见路径）：
//! GB 级工具链不许落工作区（2026-11-21 装一次 SDK 刷爆 git/产物面板的教训）。

pub(crate) mod adb;
pub(crate) mod proto;

use std::{
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};

pub(crate) const ANDROID_PLUGIN: &str = "android";
pub(crate) const ANDROID_OFF_MSG: &str = "安卓插件已停用（模拟器/adb 全部不可用）。到 设置→插件 打开「安卓」再试。";

/// 工具方法词表（工具 description 与 core/rules 分级都按它对齐）。
pub(crate) const ALL_METHODS: &[&str] = &[
    "env", "state", "list_avds", "start", "stop", "screenshot", "touch", "key", "text", "clip",
    "ui", "tap_text", "install", "launch", "logcat", "shell",
];

/// method 是否只读（core::rules 审批分级用）。
pub(crate) fn method_is_read_only(method: &str) -> bool {
    matches!(method, "env" | "state" | "list_avds" | "screenshot" | "ui" | "logcat" | "clip")
}

/// 能力总闸：插件停用 = 工具拒 + API 403 + 面板标签消失（照 browser/computer 样板）。
pub(crate) fn capability_on() -> bool {
    !crate::plugin::ui::disabled().iter().any(|d| d == ANDROID_PLUGIN)
}

pub(crate) fn android_enabled() -> bool {
    capability_on()
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// 消费端（dock 轮询/SSE）拿帧时才做 base64（见 Frame 注释：泵侧不预编码）。
pub(crate) fn b64_frame(png: &[u8]) -> String {
    b64(png)
}

// ———————————————————————————————————— 帧广播 ————————————————————————————————————

/// 一帧画面（PNG 全分辨率原始字节）。b64 按需才算：模拟器播视频时泵 13fps，
/// 每帧先 b64 一遍（~1MB 字符串）纯浪费——消费端（dock 轮询/SSE）拿到 LATEST
/// 才编码，且 seq 没变的轮询连编码都跳过（2026-11-23 CPU 实测）。
#[derive(Debug, Clone)]
pub(crate) struct Frame {
    pub seq: u32,
    pub png: Vec<u8>,
    pub w: u32,
    pub h: u32,
}

static FRAME_TX: OnceLock<tokio::sync::broadcast::Sender<Arc<Frame>>> = OnceLock::new();
static LATEST: OnceLock<Mutex<Option<Arc<Frame>>>> = OnceLock::new();
/// agent 最近一次碰安卓的时刻（ms epoch）——前端据此自动唤起面板（agentTouched 同款）。
static AGENT_TOUCHED_MS: AtomicU64 = AtomicU64::new(0);

fn frame_tx() -> &'static tokio::sync::broadcast::Sender<Arc<Frame>> {
    FRAME_TX.get_or_init(|| tokio::sync::broadcast::channel(16).0)
}

fn latest_slot() -> &'static Mutex<Option<Arc<Frame>>> {
    LATEST.get_or_init(|| Mutex::new(None))
}

pub(crate) fn latest_frame() -> Option<Arc<Frame>> {
    latest_slot().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

pub(crate) fn subscribe_frames() -> tokio::sync::broadcast::Receiver<Arc<Frame>> {
    frame_tx().subscribe()
}

pub(crate) fn agent_touched_ms() -> u64 {
    AGENT_TOUCHED_MS.load(Ordering::Relaxed)
}

fn mark_agent_touched() {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    AGENT_TOUCHED_MS.store(ms, Ordering::Relaxed);
    // agent 碰安卓 → 前端自动唤起「安卓」面板（agentTouched 同款，SSE 直推）
    crate::server::ui_events::emit(serde_json::json!({ "kind": "android_touched", "ms": ms }));
}

// ———————————————————————————————————— 配置 ————————————————————————————————————

/// 解析后的启动配置（路径已验证存在）。
#[derive(Debug, Clone)]
pub(crate) struct Resolved {
    pub sdk: PathBuf,
    pub adb: PathBuf,
    pub emulator: PathBuf,
    pub avd: Option<String>,
    pub grpc_port: u16,
    pub no_window: bool,
    pub memory_mb: u32,
    pub cores: u32,
    pub gpu: &'static str,
}

fn android_settings() -> crate::config::settings::AndroidSettings {
    crate::config::settings::shared_settings()
        .and_then(|h| h.read().ok().map(|s| s.android.clone()))
        .unwrap_or_default()
}

/// SDK 路径解析：settings 显式配置 → 环境变量 → 常见安装位。
/// 都找不到时给人话报错（引导安装/设置，不自动下载）。
pub(crate) fn resolve() -> Result<Resolved, String> {
    let cfg = android_settings();
    let sdk = find_sdk(&cfg)?;
    let adb = sdk.join("platform-tools").join("adb.exe");
    let emulator = sdk.join("emulator").join("emulator.exe");
    for p in [&adb, &emulator] {
        if !p.exists() {
            return Err(format!(
                "SDK 目录不完整：缺 {}（确认 android.sdk_path 指向 SDK 根目录）",
                p.display()
            ));
        }
    }
    Ok(Resolved {
        sdk,
        adb,
        emulator,
        avd: cfg.avd.clone(),
        grpc_port: cfg.port(),
        no_window: cfg.no_window,
        memory_mb: cfg.mem(),
        cores: cfg.cpu(),
        gpu: cfg.gpu_mode(),
    })
}

fn find_sdk(cfg: &crate::config::settings::AndroidSettings) -> Result<PathBuf, String> {
    // 显式配置优先且**不静默回落**：用户指定的路径不存在/不完整要明说，
    // 悄悄换兜底路径会让「配了没生效」变成悬案
    if let Some(p) = cfg.sdk_path.as_deref().filter(|s| !s.trim().is_empty()) {
        let c = PathBuf::from(p.trim());
        return if c.join("emulator").join("emulator.exe").exists() {
            Ok(c)
        } else {
            Err(format!(
                "android.sdk_path 指向的目录不可用：{}（缺 emulator\\emulator.exe）。确认这是 SDK 根目录，或清空该项改走自动探测",
                c.display()
            ))
        };
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    for key in ["PAWCH_AGENT_ANDROID_SDK", "ANDROID_SDK_ROOT", "ANDROID_HOME"] {
        if let Ok(v) = std::env::var(key)
            && !v.trim().is_empty()
        {
            candidates.push(PathBuf::from(v));
        }
    }
    if let Ok(l) = std::env::var("LOCALAPPDATA") {
        candidates.push(PathBuf::from(l).join("Android").join("Sdk"));
    }
    for p in ["D:\\Android\\sdk", "C:\\Android\\sdk"] {
        candidates.push(PathBuf::from(p));
    }
    for c in &candidates {
        if c.join("emulator").join("emulator.exe").exists() {
            return Ok(c.clone());
        }
    }
    // 只说原因；怎么装/怎么配的引导在面板空态的引导行（单一出处，别两头重复）
    Err("找不到 Android SDK".to_string())
}

/// 列出可用 AVD（emulator -list-avds）。
pub(crate) fn list_avds(r: &Resolved) -> Result<Vec<String>, String> {
    let out = adb::run(&r.emulator, None, &["-list-avds"], Duration::from_secs(20))?;
    Ok(out.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
}

// ———————————————————————————————————— 进程状态 ————————————————————————————————————

struct Runtime {
    child: Child,
    job: Option<crate::jobguard::ChildJob>,
    pid: u32,
    avd: String,
    serial: String,
    grpc_port: u16,
    booted: bool,
    pump: Option<tokio::task::AbortHandle>,
}

static RUNTIME: OnceLock<Mutex<Option<Runtime>>> = OnceLock::new();

fn runtime_slot() -> &'static Mutex<Option<Runtime>> {
    RUNTIME.get_or_init(|| Mutex::new(None))
}

/// 模拟器是否在跑（进程活着才算；死了顺手清状态）。
pub(crate) fn running() -> bool {
    let mut g = runtime_slot().lock().unwrap_or_else(|e| e.into_inner());
    match g.as_mut() {
        Some(rt) => match rt.child.try_wait() {
            Ok(Some(_)) => {
                if let Some(h) = rt.pump.take() {
                    h.abort();
                }
                *g = None;
                false
            }
            Ok(None) => true,
            Err(_) => false,
        },
        None => false,
    }
}

fn take_runtime() -> Option<Runtime> {
    runtime_slot().lock().unwrap_or_else(|e| e.into_inner()).take()
}

/// 当前状态 JSON（HTTP state 与工具 state 共用）。
/// 注意：这里**不**调 emulator -list-avds（子进程，前端 2s 轮询拉不起）；
/// AVD 清单走工具 env/list_avds 或 start 返回。
pub(crate) fn status_json() -> serde_json::Value {
    let r = resolve();
    let run = running();
    let g = runtime_slot().lock().unwrap_or_else(|e| e.into_inner());
    let (avd, serial, pid, port, booted) = match g.as_ref() {
        Some(rt) => (rt.avd.clone(), rt.serial.clone(), rt.pid, rt.grpc_port, rt.booted),
        None => (String::new(), String::new(), 0, 0, false),
    };
    let frame = latest_frame();
    serde_json::json!({
        "enabled": capability_on(),
        "running": run,
        "booted": run && booted,
        "avd": avd,
        "serial": serial,
        "pid": pid,
        "grpc_port": port,
        "sdk": r.as_ref().map(|x| x.sdk.display().to_string()).unwrap_or_default(),
        "sdk_ok": r.is_ok(),
        "sdk_error": r.as_ref().err().cloned(),
        "screen": frame.as_ref().map(|f| vec![f.w, f.h]).unwrap_or_default(),
        "frame_seq": frame.as_ref().map(|f| f.seq).unwrap_or(0),
        "agent_touched_ms": agent_touched_ms(),
    })
}

/// 启动模拟器（wait_boot=true 时等 boot 完成，最多 180s；超时报错但进程继续跑）。
pub(crate) async fn start(avd_override: Option<&str>, wait_boot: bool) -> Result<serde_json::Value, String> {
    if !capability_on() {
        return Err(ANDROID_OFF_MSG.to_string());
    }
    let r = resolve()?;
    if running() {
        return Ok(status_json());
    }
    let avds = list_avds(&r)?;
    if avds.is_empty() {
        return Err("没有任何 AVD。先建一个：avdmanager create avd -n <名字> -k \"system-images;android-30;default;x86_64\"（system image 缺就用 sdkmanager 装）".to_string());
    }
    let avd = avd_override
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| r.avd.clone())
        .unwrap_or_else(|| avds.last().cloned().unwrap_or_default());
    if !avds.contains(&avd) {
        return Err(format!("AVD「{avd}」不存在。可用：{}", avds.join("、")));
    }
    // 挑空闲 gRPC 端口（从配置端口起顺延，最多试 8 个）
    let mut grpc_port = 0u16;
    for p in r.grpc_port..r.grpc_port.saturating_add(8) {
        if std::net::TcpListener::bind(("127.0.0.1", p)).is_ok() {
            grpc_port = p;
            break;
        }
    }
    if grpc_port == 0 {
        return Err(format!("gRPC 端口 {}–{} 都被占用，腾一个再启动", r.grpc_port, r.grpc_port + 7));
    }

    let mut cmd = Command::new(&r.emulator);
    cmd.arg("-avd").arg(&avd)
        .arg("-port").arg("5554")
        .arg("-no-audio")
        .arg("-no-boot-anim")
        .arg("-no-snapshot")
        .arg("-no-metrics")
        // GPU 渲染后端：host = 真机 GPU 卸载（无窗口模式实测可用，2026-11-23：
        // 滑动负载 CPU 15s→3.8s/8s，视频类持续画面的差距更大）；显卡驱动异常
        // 可在设置 android.gpu 切 swiftshader_indirect（纯软件渲染，CPU 打满）。
        .arg("-gpu").arg(r.gpu)
        .arg("-cores").arg(r.cores.to_string())
        .arg("-memory").arg(r.memory_mb.to_string())
        .arg("-grpc").arg(grpc_port.to_string());
    if r.no_window {
        cmd.arg("-no-window");
    }
    cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW：服务后台进程不弹控制台
    }
    let child = cmd.spawn().map_err(|e| format!("emulator.exe 起不来: {e}"))?;
    let job = crate::jobguard::ChildJob::new();
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle as _;
        // jobguard 收的是 windows_sys HANDLE（isize）；std 的 RawHandle 是指针，转一下
        let handle = child.as_raw_handle() as isize;
        if let Some(j) = job.as_ref() {
            j.assign(handle);
        }
    }
    let rt = Runtime {
        pid: child.id(),
        child,
        job,
        avd: avd.clone(),
        serial: "emulator-5554".to_string(),
        grpc_port,
        booted: false,
        pump: Some(spawn_pump(grpc_port).abort_handle()),
    };
    *runtime_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(rt);

    if wait_boot {
        let deadline = std::time::Instant::now() + Duration::from_secs(180);
        loop {
            if !running() {
                return Err("模拟器进程退出了。常见原因：控制台端口 5554 被占、镜像损坏、AEHD 加速没装（emulator -accel-check 看）、GPU host 模式显卡驱动不支持（设置 android.gpu 改 swiftshader_indirect 重试）".to_string());
            }
            if boot_poll(grpc_port).await {
                set_booted(true);
                break;
            }
            if std::time::Instant::now() > deadline {
                return Err("等模拟器 boot 超时（180s）。进程还活着，稍后用 method=state 再查；或 stop 后重试".to_string());
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    }
    mark_agent_touched();
    Ok(status_json())
}

fn set_booted(v: bool) {
    let mut g = runtime_slot().lock().unwrap_or_else(|e| e.into_inner());
    if let Some(rt) = g.as_mut() {
        rt.booted = v;
    }
}

/// 停模拟器：adb emu kill 优雅停 → 不退再 Job 整树强杀。
pub(crate) async fn stop() -> Result<serde_json::Value, String> {
    let mut rt = take_runtime().ok_or_else(|| "模拟器没在跑".to_string())?;
    if let Some(h) = rt.pump.take() {
        h.abort();
    }
    if let Ok(r) = resolve() {
        let adb = r.adb.clone();
        let serial = rt.serial.clone();
        tokio::task::spawn_blocking(move || adb::run(&adb, Some(&serial), &["emu", "kill"], Duration::from_secs(8)).ok())
            .await
            .ok();
    }
    for _ in 0..50 {
        match rt.child.try_wait() {
            Ok(Some(_)) => return Ok(status_json()),
            Ok(None) => tokio::time::sleep(Duration::from_millis(100)).await,
            Err(_) => break,
        }
    }
    drop(rt.job.take()); // Drop = TerminateJobObject：qemu 整树跟着死
    let _ = rt.child.kill();
    let _ = rt.child.wait();
    Ok(status_json())
}

// ———————————————————————————————————— gRPC 客户端 ————————————————————————————————————

type Ctrl = proto::control::emulator_controller_client::EmulatorControllerClient<tonic::transport::Channel>;

async fn connect_ctrl(port: u16, wait: Duration) -> Result<Ctrl, String> {
    let addr = format!("http://127.0.0.1:{port}");
    let deadline = std::time::Instant::now() + wait;
    loop {
        match Ctrl::connect(addr.clone()).await {
            Ok(c) => return Ok(c),
            Err(e) => {
                if std::time::Instant::now() > deadline {
                    return Err(format!("连不上模拟器 gRPC（{addr}）: {e}"));
                }
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
        }
    }
}

/// gRPC getStatus().booted（boot 检测不依赖 adb server）。
async fn boot_poll(port: u16) -> bool {
    let Ok(mut c) = connect_ctrl(port, Duration::from_secs(2)).await else { return false };
    match c.get_status(tonic::Request::new(())).await {
        Ok(resp) => resp.into_inner().booted,
        Err(_) => false,
    }
}

/// 按需单帧（全分辨率 PNG）。
async fn screenshot_grpc(port: u16) -> Result<proto::control::Image, String> {
    let mut c = connect_ctrl(port, Duration::from_secs(5)).await?;
    let req = proto::control::ImageFormat {
        format: proto::control::image_format::ImgFormat::Png as i32,
        ..Default::default()
    };
    c.get_screenshot(tonic::Request::new(req))
        .await
        .map(|r| r.into_inner())
        .map_err(|e| format!("getScreenshot 失败: {e}"))
}

/// gRPC streamInputEvent 送触点序列。`items = (触点事件, 其后延时 ms)`——
/// down/up 之间必须留 hold（实测立即连发会被模拟器吞掉）。
async fn send_touch_stream(port: u16, items: Vec<(proto::control::TouchEvent, u64)>) -> Result<(), String> {
    use tokio_stream::wrappers::ReceiverStream;
    let mut c = connect_ctrl(port, Duration::from_secs(5)).await?;
    let (tx, rx) = tokio::sync::mpsc::channel::<proto::control::InputEvent>(16);
    tokio::spawn(async move {
        for (ev, delay_ms) in items {
            // InputEvent 是 oneof（prost 生成 r#type + input_event::Type 枚举）
            let input = proto::control::InputEvent {
                r#type: Some(proto::control::input_event::Type::TouchEvent(ev)),
            };
            if tx.send(input).await.is_err() {
                break;
            }
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }
        }
    });
    c.stream_input_event(tonic::Request::new(ReceiverStream::new(rx)))
        .await
        .map(|_| ())
        .map_err(|e| format!("streamInputEvent 失败: {e}"))
}

fn touch(x: i64, y: i64, pressure: i64) -> proto::control::Touch {
    proto::control::Touch {
        x: x.clamp(0, i32::MAX as i64) as i32,
        y: y.clamp(0, i32::MAX as i64) as i32,
        identifier: 1,
        pressure: pressure.clamp(0, 1024) as i32,
        touch_major: 0,
        touch_minor: 0,
        expiration: 0,
        orientation: 0,
    }
}

fn touch_event(x: i64, y: i64, pressure: i64) -> proto::control::TouchEvent {
    proto::control::TouchEvent { touches: vec![touch(x, y, pressure)], display: 0 }
}

/// 点击（down → 80ms hold → up）。
pub(crate) async fn tap(port: u16, x: i64, y: i64) -> Result<(), String> {
    send_touch_stream(port, vec![(touch_event(x, y, 100), 80), (touch_event(x, y, 0), 0)]).await
}

/// 滑动（直线，默认 240ms）。
pub(crate) async fn swipe(port: u16, x1: i64, y1: i64, x2: i64, y2: i64, duration_ms: u64) -> Result<(), String> {
    let steps: i64 = ((duration_ms.max(60) as f64 / 16.0).round() as i64).clamp(4, 40);
    let mut items = vec![(touch_event(x1, y1, 100), 0)];
    for i in 1..steps {
        let t = i as f64 / steps as f64;
        let x = x1 + ((x2 - x1) as f64 * t).round() as i64;
        let y = y1 + ((y2 - y1) as f64 * t).round() as i64;
        items.push((touch_event(x, y, 100), 16));
    }
    items.push((touch_event(x2, y2, 0), 0));
    send_touch_stream(port, items).await
}

/// ASCII 文本直送（gRPC sendKey{text}，按键级 press-and-release；非 ASCII 走 clip）。
pub(crate) async fn text(port: u16, text: &str) -> Result<(), String> {
    if !text.is_ascii() {
        return Err("text 只收 ASCII（中文请用 method=clip 设置剪贴板，再在输入框长按粘贴）".to_string());
    }
    let mut c = connect_ctrl(port, Duration::from_secs(5)).await?;
    let ev = proto::control::KeyboardEvent { text: text.to_string(), ..Default::default() };
    c.send_key(tonic::Request::new(ev))
        .await
        .map(|_| ())
        .map_err(|e| format!("sendKey 失败: {e}"))
}

/// 剪贴板 set/get（中文路径）。
pub(crate) async fn clip(port: u16, set_text: Option<&str>) -> Result<String, String> {
    let mut c = connect_ctrl(port, Duration::from_secs(5)).await?;
    if let Some(t) = set_text {
        c.set_clipboard(tonic::Request::new(proto::control::ClipData { text: t.to_string() }))
            .await
            .map_err(|e| format!("setClipboard 失败: {e}"))?;
    }
    c.get_clipboard(tonic::Request::new(()))
        .await
        .map(|r| r.into_inner().text)
        .map_err(|e| format!("getClipboard 失败: {e}"))
}

/// Android keycode（4=BACK 3=HOME 187=RECENTS…）走 adb input keyevent。
pub(crate) async fn key(r: &Resolved, serial: &str, keycode: i32) -> Result<(), String> {
    let code = keycode.to_string();
    let adb = r.adb.clone();
    let serial = serial.to_string();
    tokio::task::spawn_blocking(move || {
        adb::run(&adb, Some(&serial), &["shell", "input", "keyevent", &code], Duration::from_secs(15))
    })
    .await
    .map_err(|e| format!("keyevent 任务失败: {e}"))?
    .map(|_| ())
}

// ———————————————————————————————————— 帧泵 ————————————————————————————————————

/// 常驻帧泵：等 gRPC → 等 boot → 循环 streamScreenshot（变化驱动）。
/// 流断开自动重连；模拟器退出（running() 假）即停。
fn spawn_pump(port: u16) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let Ok(mut ctrl) = connect_ctrl(port, Duration::from_secs(90)).await else { return };
        // 等 boot（start 的 wait_boot 是一层，这里是泵自己的双保险）
        for _ in 0..400 {
            let booted = match ctrl.get_status(tonic::Request::new(())).await {
                Ok(resp) => resp.into_inner().booted,
                Err(_) => false,
            };
            if booted {
                set_booted(true);
                break;
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
            if !running() {
                return;
            }
        }
        loop {
            if !running() {
                return;
            }
            let req = proto::control::ImageFormat {
                format: proto::control::image_format::ImgFormat::Png as i32,
                ..Default::default()
            };
            let mut s = match ctrl.stream_screenshot(tonic::Request::new(req)).await {
                Ok(resp) => resp.into_inner(),
                Err(_) => {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    continue;
                }
            };
            loop {
                match s.message().await {
                    Ok(Some(img)) => {
                        let (w, h) = img
                            .format
                            .as_ref()
                            .map(|f| (f.width, f.height))
                            .unwrap_or((0, 0));
                        let frame = Arc::new(Frame {
                            seq: img.seq,
                            png: img.image,
                            w,
                            h,
                        });
                        *latest_slot().lock().unwrap_or_else(|e| e.into_inner()) = Some(frame.clone());
                        let _ = frame_tx().send(frame);
                    }
                    Ok(None) => break, // 流结束（模拟器退了/被取消）→ 外层重连
                    Err(_) => break,
                }
                if !running() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    })
}

// ———————————————————————————————————— 工具入口 ————————————————————————————————————

fn active_runtime() -> Result<(u16, String), String> {
    if !running() {
        return Err("模拟器没在跑。先 method=start 启动（首次冷启动约 40s）".to_string());
    }
    let g = runtime_slot().lock().unwrap_or_else(|e| e.into_inner());
    let rt = g.as_ref().ok_or_else(|| "模拟器状态丢失".to_string())?;
    Ok((rt.grpc_port, rt.serial.clone()))
}

/// 工具 path 参数解析：相对工作区、绝对路径直用（装 APK 允许机器上任意位置——
/// 模拟器是隔离设备，不构成工作区逃逸面）。
fn resolve_apk_path(workspace: &Path, p: &str) -> Result<PathBuf, String> {
    let t = p.trim();
    if t.is_empty() {
        return Err("path 为空".to_string());
    }
    let path = PathBuf::from(t);
    if path.is_absolute() {
        Ok(path)
    } else {
        Ok(workspace.join(path))
    }
}

/// 面板取图：LATEST 帧直出（不落盘），没有才按需抓一帧。
pub(crate) async fn current_png() -> Result<Vec<u8>, String> {
    if let Some(f) = latest_frame() {
        return Ok(f.png.clone());
    }
    let (port, _) = active_runtime()?;
    Ok(screenshot_grpc(port).await?.image)
}

/// 截图：落盘工作区 screenshots/ + inline marker（computer 同款）。
async fn screenshot_impl(workspace: &Path) -> Result<String, String> {
    let (port, _) = active_runtime()?;
    let img = screenshot_grpc(port).await?;
    let (w, h) = img.format.as_ref().map(|f| (f.width, f.height)).unwrap_or((0, 0));
    let dir = workspace.join("screenshots");
    std::fs::create_dir_all(&dir).map_err(|e| format!("建 screenshots/ 失败: {e}"))?;
    let path = dir.join(format!("android_{}.png", chrono::Local::now().format("%Y%m%d_%H%M%S")));
    std::fs::write(&path, &img.image).map_err(|e| format!("写截图失败: {e}"))?;
    let body = format!(
        "已截图 {}×{}（{} KB），全图存 {}；画面内容看下面这张图",
        w,
        h,
        img.image.len() / 1024,
        path.display()
    );
    Ok(format!("{body}{}{}{}", crate::computer::IMG_OPEN, b64(&img.image), crate::computer::IMG_CLOSE))
}

/// android 工具入口（tools::execute_one 分发到这里）。args 是原始 JSON 串
/// （与 browser/computer 同口径：字符串进来自己 parse）。
pub(crate) async fn execute(args: &str, max_chars: usize, workspace: &Path) -> Result<String, String> {
    let args: serde_json::Value = serde_json::from_str(args).unwrap_or_else(|_| serde_json::json!({}));
    let method = args
        .get("method")
        .and_then(|m| m.as_str())
        .ok_or_else(|| "method 缺失".to_string())?;
    if !capability_on() {
        return Err(ANDROID_OFF_MSG.to_string());
    }
    mark_agent_touched();
    let cap = |s: String| crate::context::compress(&s, max_chars);
    match method {
        "env" => {
            let r = resolve()?;
            let avds = list_avds(&r)?;
            Ok(cap(format!(
                "SDK: {}\nadb: {}\nemulator: {}\nAVD: {}\n配置: grpc_port={} no_window={} memory={}MB cores={} gpu={}\n\
                 口径：模拟器没跑先 method=start 启动（首次冷启动约 40s）；找控件先 method=ui 或 tap_text；\
                 中文输入走 method=clip 设剪贴板；装 system image/SDK 这类大件前先问用户位置。",
                r.sdk.display(),
                r.adb.display(),
                r.emulator.display(),
                if avds.is_empty() { "（无，先 avdmanager create avd）".to_string() } else { avds.join("、") },
                r.grpc_port,
                r.no_window,
                r.memory_mb,
                r.cores,
                r.gpu,
            )))
        }
        "state" => Ok(cap(status_json().to_string())),
        "list_avds" => {
            let r = resolve()?;
            let avds = list_avds(&r)?;
            Ok(cap(if avds.is_empty() { "（无 AVD）".to_string() } else { avds.join("\n") }))
        }
        "start" => {
            let avd = args.get("avd").and_then(|v| v.as_str());
            start(avd, true).await.map(|v| cap(v.to_string()))
        }
        "stop" => stop().await.map(|v| cap(v.to_string())),
        "screenshot" => screenshot_impl(workspace).await,
        "touch" => {
            let (port, _) = active_runtime()?;
            let x = args.get("x").and_then(|v| v.as_i64()).ok_or("x 缺失")?;
            let y = args.get("y").and_then(|v| v.as_i64()).ok_or("y 缺失")?;
            match (args.get("x2").and_then(|v| v.as_i64()), args.get("y2").and_then(|v| v.as_i64())) {
                (Some(x2), Some(y2)) => {
                    let d = args.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(240);
                    swipe(port, x, y, x2, y2, d).await?;
                    Ok(cap(format!("已滑动 ({x},{y})→({x2},{y2})")))
                }
                _ => {
                    tap(port, x, y).await?;
                    Ok(cap(format!("已点击 ({x},{y})")))
                }
            }
        }
        "key" => {
            let (r, serial) = (resolve()?, active_runtime()?.1);
            let code = args
                .get("keycode")
                .and_then(|v| v.as_i64())
                .ok_or("keycode 缺失（Android keycode：4=返回 3=HOME 187=最近任务）")? as i32;
            key(&r, &serial, code).await?;
            Ok(cap(format!("已发送 keyevent {code}")))
        }
        "text" => {
            let (port, _) = active_runtime()?;
            let t = args.get("text").and_then(|v| v.as_str()).ok_or("text 缺失")?;
            text(port, t).await?;
            Ok(cap(format!("已输入 {} 个字符", t.len())))
        }
        "clip" => {
            let (port, _) = active_runtime()?;
            let t = args.get("text").and_then(|v| v.as_str());
            let got = clip(port, t).await?;
            Ok(cap(match t {
                Some(_) => format!("剪贴板已设置，当前内容: {got}"),
                None => format!("剪贴板内容: {got}"),
            }))
        }
        "ui" => {
            let (r, serial) = (resolve()?, active_runtime()?.1);
            let xml = adb::ui_dump(&r.adb, &serial)?;
            let nodes = adb::parse_ui_nodes(&xml);
            if nodes.is_empty() {
                return Err("ui dump 为空（界面可能正在动画，重试一次）".to_string());
            }
            let mut lines: Vec<String> = nodes
                .iter()
                .map(|n| {
                    let (x1, y1, x2, y2) = n.bounds;
                    let label = if !n.text.is_empty() {
                        n.text.clone()
                    } else if !n.desc.is_empty() {
                        format!("[desc]{}", n.desc)
                    } else {
                        String::new()
                    };
                    format!(
                        "({},{}) {}{} {}",
                        (x1 + x2) / 2,
                        (y1 + y2) / 2,
                        if n.clickable { "[可点] " } else { "" },
                        if n.resource.is_empty() { String::new() } else { format!("{} ", n.resource) },
                        label
                    )
                })
                .collect();
            lines.sort();
            Ok(cap(format!("界面控件（中心坐标 可点标记 资源id 文本）：\n{}", lines.join("\n"))))
        }
        "tap_text" => {
            let (r, port, serial) = {
                let rr = resolve()?;
                let (port, serial) = active_runtime()?;
                (rr, port, serial)
            };
            let want = args.get("text").and_then(|v| v.as_str()).ok_or("text 缺失（要点的控件文本）")?;
            let xml = adb::ui_dump(&r.adb, &serial)?;
            let nodes = adb::parse_ui_nodes(&xml);
            let hit = nodes
                .iter()
                .find(|n| n.text.contains(want) || n.desc.contains(want))
                .ok_or_else(|| format!("界面上没找到文本含「{want}」的控件（先 method=ui 看有哪些）"))?;
            let (x1, y1, x2, y2) = hit.bounds;
            let (cx, cy) = ((x1 + x2) / 2, (y1 + y2) / 2);
            tap(port, cx, cy).await?;
            Ok(cap(format!("已点击「{}」({cx},{cy})", hit.text)))
        }
        "install" => {
            let (r, serial) = (resolve()?, active_runtime()?.1);
            let p = args.get("path").and_then(|v| v.as_str()).ok_or("path 缺失（APK 路径）")?;
            let apk = resolve_apk_path(workspace, p)?;
            if !apk.exists() {
                return Err(format!("APK 不存在: {}", apk.display()));
            }
            let adb = r.adb.clone();
            let serial2 = serial.clone();
            let out = tokio::task::spawn_blocking(move || adb::install(&adb, &serial2, &apk))
                .await
                .map_err(|e| format!("install 任务失败: {e}"))??;
            Ok(cap(format!("已安装: {out}")))
        }
        "launch" => {
            let (r, serial) = (resolve()?, active_runtime()?.1);
            let package = args.get("package").and_then(|v| v.as_str()).ok_or("package 缺失")?;
            let activity = args.get("activity").and_then(|v| v.as_str()).map(str::to_string);
            let adb = r.adb.clone();
            let serial2 = serial.clone();
            let package = package.to_string();
            let out = tokio::task::spawn_blocking(move || adb::launch(&adb, &serial2, &package, activity.as_deref()))
                .await
                .map_err(|e| format!("launch 任务失败: {e}"))??;
            Ok(cap(out.trim().to_string()))
        }
        "logcat" => {
            let (r, serial) = (resolve()?, active_runtime()?.1);
            let n = args.get("lines").and_then(|v| v.as_u64()).unwrap_or(200) as usize;
            let adb = r.adb.clone();
            let serial2 = serial.clone();
            let out = tokio::task::spawn_blocking(move || adb::logcat(&adb, &serial2, n))
                .await
                .map_err(|e| format!("logcat 任务失败: {e}"))??;
            Ok(cap(out))
        }
        "shell" => {
            let (r, serial) = (resolve()?, active_runtime()?.1);
            let cmd = args.get("cmd").and_then(|v| v.as_str()).ok_or("cmd 缺失")?;
            let adb = r.adb.clone();
            let serial2 = serial.clone();
            let cmd = cmd.to_string();
            let out = tokio::task::spawn_blocking(move || adb::shell(&adb, &serial2, &cmd))
                .await
                .map_err(|e| format!("shell 任务失败: {e}"))??;
            Ok(cap(out))
        }
        other => Err(format!("未知 method: {other}（可选：{}）", ALL_METHODS.join("/"))),
    }
}
