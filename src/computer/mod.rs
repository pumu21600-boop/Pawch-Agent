//! computer 工具：操作电脑（2026-10-20 免打扰模式第 4 轮）。
//!
//! 能力面（一个工具按 method 覆盖，与 browser/ui 同 house style）：
//! - screenshot 截虚拟屏 PNG（落盘工作区 + 内联给模型，模型这才「看得见」）
//! - cursor / screen_size / windows / focus 窗口与坐标
//! - click / move / scroll / drag / type / key 键鼠控制（SendInput）
//!
//! 纪律：
//! - **截图内联进工具结果**（`<<COMPUTER_IMG:base64>>` 标记，exec.rs 剥出升级成
//!   多模态 tool 消息）——没有这条，agent 就是瞎子；落盘的那份给人/面板看。
//! - 图片只进请求上下文，**不进会话落盘**（快照剥离，见 summary.rs）——
//!   否则 session 文件被 base64 撑爆。
//! - 只读方法（screenshot/cursor/screen_size/windows）审批归 Safe；键鼠动作归
//!   Normal，按会话审批档走闸（rules.rs）。
//! - 每一步都经 ui_events 总线推 `computer.action`——聊天区实时条 + 主区面板，
//!   用户随时看得到 agent 在动他的电脑（UI 不落侧边栏）。

mod input;
mod png;
mod screen;
mod uia;
mod window;

use std::sync::{Mutex, OnceLock};

use serde_json::{json, Value};

use input::MouseButton;

/// 工具结果里的截图标记（exec.rs 认这个前后缀）。base64 PNG。
pub const IMG_OPEN: &str = "\n<<COMPUTER_IMG:";
pub const IMG_CLOSE: &str = ">>";

/// 内联给模型的截图宽度上限（超过就最近邻降采样——省 token，UI 细节靠落盘那份）。
const INLINE_MAX_WIDTH: u32 = 1440;

/// 截图落盘目录（工作区下）。用户可直接翻看，面板也读这里。
fn shots_dir() -> std::path::PathBuf {
    crate::tools::workspace_root().join("screenshots")
}

/// 最近一次截图（面板/重开后回看用）。
#[derive(Default, Clone)]
pub struct ComputerState {
    pub last_screenshot: Option<String>,
    pub last_size: Option<(u32, u32)>,
    /// 上一次点击的屏幕坐标——回执拿它提醒「你是不是在同一个点反复点」。
    pub last_click: Option<(i32, i32)>,
    pub actions: Vec<String>,
}

fn state_slot() -> &'static Mutex<ComputerState> {
    static S: OnceLock<Mutex<ComputerState>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(ComputerState::default()))
}

/// 记下这次点击，返回上一次的点击位置（没点过 = None）。
fn note_click(x: i32, y: i32) -> Option<(i32, i32)> {
    let mut s = state_slot().lock().unwrap_or_else(|e| e.into_inner());
    s.last_click.replace((x, y))
}

pub fn state() -> ComputerState {
    state_slot().lock().unwrap_or_else(|e| e.into_inner()).clone()
}

fn record_action(line: &str, shot: Option<String>, size: Option<(u32, u32)>) {
    {
        let mut s = state_slot().lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = &shot {
            s.last_screenshot = Some(p.clone());
        }
        if size.is_some() {
            s.last_size = size;
        }
        s.actions.insert(0, line.to_string());
        s.actions.truncate(20);
    }
    crate::server::ui_events::emit(json!({
        "kind": "computer.action",
        "action": line,
        "screenshot": shot,
    }));
}

/// 从工具结果里剥出截图 base64（标记从文本中移除）。没有标记返回 None。
pub fn take_image(content: &mut String) -> Option<String> {
    let start = content.find(IMG_OPEN)?;
    let rest = &content[start + IMG_OPEN.len()..];
    let end = rest.find(IMG_CLOSE)?;
    let b64 = rest[..end].to_string();
    content.replace_range(start..start + IMG_OPEN.len() + end + IMG_CLOSE.len(), "");
    content.push('\n');
    Some(b64)
}

/// 方法清单（工具 schema enum 与 rules 分级共用）。
pub const ALL_METHODS: &[&str] = &[
    "screenshot", "cursor", "screen_size", "windows", "focus", "click", "move", "scroll", "drag", "type", "key",
    "launch", "find",
];

/// 只读方法（审批 Safe）。键鼠动作会真的动用户的电脑，归 Normal。
pub fn method_is_read_only(method: &str) -> bool {
    matches!(method, "screenshot" | "cursor" | "screen_size" | "windows" | "find")
}

/// 工具入口（tools/mod.rs 分发到这里）。全部 Win32 调用包 spawn_blocking，
/// 不占 tokio worker（截图编码 + 逐键输入都是毫秒级同步活）。
pub async fn execute(args: &str, max_chars: usize) -> Result<String, String> {
    let v: Value = serde_json::from_str(args).map_err(|e| format!("参数解析失败: {e}"))?;
    let method = v.get("method").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if !ALL_METHODS.contains(&method.as_str()) {
        return Err(format!(
            "缺少/未知 method，可用: {}",
            ALL_METHODS.join(" / ")
        ));
    }
    tokio::task::spawn_blocking(move || dispatch(&v, &method, max_chars))
        .await
        .map_err(|e| format!("computer 任务panic: {e}"))?
}

fn dispatch(v: &Value, method: &str, max_chars: usize) -> Result<String, String> {
    match method {
        "screenshot" => cmd_screenshot(v, max_chars),
        "cursor" => {
            let (x, y) = screen::cursor_pos().ok_or("取光标位置失败")?;
            record_action(&format!("查光标位置 ({x},{y})"), None, None);
            Ok(format!("光标位置：({x}, {y})"))
        }
        "screen_size" => {
            let (x, y, w, h) = screen::virtual_screen();
            record_action(&format!("查屏幕尺寸 {w}×{h}"), None, None);
            Ok(format!("虚拟屏 {w}×{h}，左上角 ({x}, {y})（多屏拼接；点击/截图坐标都用这套）"))
        }
        "windows" => {
            let wins = window::list_windows();
            let mut out = format!("可见窗口 {} 个（按前后顺序）：\n", wins.len());
            for (i, w) in wins.iter().take(30).enumerate() {
                out.push_str(&format!("{}. [hwnd {}] {}\n", i + 1, w.hwnd, w.title));
            }
            record_action(&format!("列窗口 {} 个", wins.len()), None, None);
            Ok(crate::context::compress(&out, max_chars))
        }
        // 按名字找控件（UIA）：坐标从「估」变「查」——真机日志里模型为了找系统代理
        // 开关截了 3 张局部图、连偏 70 像素，这一下把整串省掉。
        "find" => {
            let text = v.get("text").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if text.is_empty() {
                return Err("find 需要 text（控件名字的一部分，如 \"系统代理\"）".into());
            }
            // 目标窗口：显式 hwnd > 标题 > 当前前台
            let hwnd = if let Some(h) = v.get("hwnd").and_then(Value::as_i64) {
                h as isize
            } else if let Some(t) = v.get("title").and_then(Value::as_str).map(str::trim).filter(|t| !t.is_empty()) {
                window::find_window(t).ok_or_else(|| format!("找不到标题含「{t}」的窗口"))?.hwnd
            } else {
                window::foreground_hwnd()
            };
            let rep = uia::find(&text, Some(hwnd))?;
            record_action(&format!("UIA 查控件「{text}」→ {} 个", rep.matches.len()), None, None);
            let mut out = if rep.matches.is_empty() {
                format!(
                    "在「{}」的 {} 个控件里没找到名字含「{text}」的（整趟 {}ms）。\
                     可能：不在当前页面（先滚动/切页签）、名字叫法不同（试更短的词，如只写「代理」）、\
                     或者这块界面没有暴露 UIA。那就退回 screenshot 配 region 看。",
                    rep.window, rep.count, rep.ms
                )
            } else {
                let mut s = format!(
                    "在「{}」里找到 {} 个名字含「{text}」的控件（共 {} 个元素，整趟 {}ms）：\n",
                    rep.window,
                    rep.matches.len(),
                    rep.count,
                    rep.ms
                );
                for (i, m) in rep.matches.iter().take(10).enumerate() {
                    let (cx, cy) = m.center();
                    s.push_str(&format!(
                        "{}. {}「{}」中心 ({cx},{cy})，rect {},{},{},{}\n",
                        i + 1,
                        m.kind,
                        clip(&m.name, 32),
                        m.rect.0,
                        m.rect.1,
                        m.rect.2,
                        m.rect.3
                    ));
                }
                if !rep.row.is_empty() {
                    s.push_str("它们所在行的可点控件（开关/按钮常常没有名字，靠这一栏认）：\n");
                    for m in rep.row.iter().take(12) {
                        let (cx, cy) = m.center();
                        let name = if m.name.trim().is_empty() { "（无名字）".to_string() } else { format!("「{}」", clip(&m.name, 28)) };
                        s.push_str(&format!(
                            "- {}{name} 中心 ({cx},{cy})，rect {},{},{},{}\n",
                            m.kind, m.rect.0, m.rect.1, m.rect.2, m.rect.3
                        ));
                    }
                }
                s.push_str("直接 click 上面某个中心点即可，不用再截图估坐标；点完看回执确认有没有变化。");
                s
            };
            out = crate::context::compress(&out, max_chars);
            Ok(out)
        }
        "focus" => {
            let title = v.get("title").and_then(Value::as_str).map(str::trim).unwrap_or("");
            let hwnd = v.get("hwnd").and_then(Value::as_i64);
            let (hwnd, label) = if let Some(h) = hwnd {
                (h as isize, format!("hwnd {h}"))
            } else if !title.is_empty() {
                let w = window::find_window(title).ok_or_else(|| format!("找不到标题含「{title}」的窗口"))?;
                (w.hwnd, w.title)
            } else {
                return Err("focus 需要 title 或 hwnd".into());
            };
            let ok = window::focus_window(hwnd)?;
            record_action(&format!("聚焦窗口「{label}」{}", if ok { "" } else { "（系统没让位，可能闪了下任务栏）" }), None, None);
            Ok(if ok {
                format!("已把「{label}」带到前台。{}", window::focus_diagnostics())
            } else {
                format!("SetForegroundWindow 被拒（前台锁）。当前还是：{}", window::focus_diagnostics())
            })
        }
        "click" => {
            let (x, y) = xy(v)?;
            let button = MouseButton::parse(v.get("button").and_then(Value::as_str).unwrap_or("left"))
                .ok_or("button 只能是 left/right/middle")?;
            let clicks = v.get("clicks").and_then(Value::as_u64).unwrap_or(1).clamp(1, 3) as u32;
            // 回执默认给（2026-11-02 真机实测：模型盲点开关，两次修正只差 1 像素；
            // 同日二次：连着三次点偏 70px 不自知）。verify=false 可关，省 token 的场景自己关。
            let verify = v.get("verify").and_then(Value::as_bool).unwrap_or(true);
            // 点之前抓一张：回执里拿它和点完的画面比，直接说「这块有没有反应」
            let before = verify.then(|| screen::capture_bgra().ok()).flatten();
            let prev = note_click(x, y);
            input::click(x, y, button, clicks);
            record_action(&format!("{clicks}连击 {button:?} @({x},{y})"), None, None);
            match verify.then(|| click_receipt(x, y, before, prev)).flatten() {
                Some((text, b64)) => Ok(format!("已在 ({x}, {y}) {button:?} 点击 {clicks} 次。{text}{IMG_OPEN}{b64}{IMG_CLOSE}")),
                None => Ok(format!("已在 ({x}, {y}) {button:?} 点击 {clicks} 次（没取到回执图，需要的话自己 screenshot）")),
            }
        }
        "move" => {
            let (x, y) = xy(v)?;
            input::move_to(x, y);
            record_action(&format!("移动光标 @({x},{y})"), None, None);
            Ok(format!("光标已移到 ({x}, {y})"))
        }
        "scroll" => {
            let (x, y) = xy(v)?;
            let dx = v.get("dx").and_then(Value::as_i64).unwrap_or(0).clamp(-10, 10) as i32;
            let dy = v.get("dy").and_then(Value::as_i64).unwrap_or(0).clamp(-10, 10) as i32;
            input::scroll(x, y, dx, dy);
            record_action(&format!("滚轮 @({x},{y}) dx={dx} dy={dy}"), None, None);
            Ok(format!("已在 ({x}, {y}) 滚动（横 {dx} 竖 {dy} 格）"))
        }
        "drag" => {
            let from = pair(v, "from")?;
            let to = pair(v, "to")?;
            let button = MouseButton::parse(v.get("button").and_then(Value::as_str).unwrap_or("left"))
                .ok_or("button 只能是 left/right/middle")?;
            input::drag(from, to, button);
            record_action(&format!("拖拽 {button:?} {from:?}→{to:?}"), None, None);
            Ok(format!("已从 {from:?} 拖到 {to:?}"))
        }
        "type" => {
            let text = v.get("text").and_then(Value::as_str).unwrap_or("");
            if text.is_empty() {
                return Err("type 需要 text".into());
            }
            input::type_text(text);
            let preview: String = text.chars().take(40).collect();
            record_action(&format!("输入 {} 字：{preview}…", text.chars().count()), None, None);
            Ok(format!("已输入 {} 字符。焦点在：{}", text.chars().count(), window::focus_diagnostics()))
        }
        "key" => {
            let combo = v.get("combo").and_then(Value::as_str).unwrap_or("").trim();
            if combo.is_empty() {
                return Err("key 需要 combo（如 \"ctrl+c\" / \"enter\"）".into());
            }
            input::key_combo(combo)?;
            record_action(&format!("按键 {combo}"), None, None);
            Ok(format!("已按 {combo}。焦点在：{}", window::focus_diagnostics()))
        }
        // 打开应用/文档（2026-11-02）：ShellExecute 等效双击，进程**不进沙箱 Job**，
        // shell 命令结束也不会被清理（打开应用不该归沙箱管）。
        // 同一天二次收口：认应用名（自己找快捷方式）+ 等窗口 + 直接带前台 + 回 hwnd——
        // 这几步原本要模型串 4 次调用，串得不好就白花（真机实测「打开 tabbit 用了 7 步」）。
        "launch" => {
            let want = v.get("path").and_then(Value::as_str).unwrap_or("").trim().to_string();
            if want.is_empty() {
                return Err("launch 需要 path（应用名，如 \"Tabbit\"，或 .lnk/.exe 完整路径）".into());
            }
            let args = v.get("args").and_then(Value::as_str).unwrap_or("");
            let workdir = v.get("workdir").and_then(Value::as_str).unwrap_or("");
            let target = window::resolve_app_target(&want)?;
            let target_str = target.display().to_string();
            let stem = target
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&want)
                .to_string();
            // 启动前的窗口集合：只有新出现的才算「启动成功」
            let before: Vec<isize> = window::list_windows().iter().map(|w| w.hwnd).collect();
            window::shell_execute(&target_str, args, workdir)?;
            record_action(&format!("启动 {target_str}"), None, None);
            let fresh = window::wait_new_window(&before, std::time::Duration::from_millis(4000));
            let (found, launched) = match fresh {
                Some(w) => (Some(w), true),
                // 没新窗口 = 多半本来就在跑（最小化/托盘），按名字找找现有窗口
                None => (window::find_window_loose(&stem), false),
            };
            Ok(match found {
                Some(w) => {
                    let focused = window::focus_window(w.hwnd).unwrap_or(false);
                    let head = if launched {
                        format!("已启动「{stem}」")
                    } else {
                        format!("「{stem}」已经在运行")
                    };
                    if focused {
                        format!(
                            "{head}，窗口「{}」[hwnd {}] 已切到前台。要看画面再 screenshot 一次即可，不用先 windows。",
                            w.title, w.hwnd
                        )
                    } else {
                        format!(
                            "{head}，窗口「{}」[hwnd {}] 出现了但没抢到前台（前台锁）。需要的话 focus hwnd={} 再试。",
                            w.title, w.hwnd, w.hwnd
                        )
                    }
                }
                None => format!(
                    "已启动「{stem}」（{target_str}），但 4 秒内没等到新窗口——可能在托盘、最小化或还在加载。\
                     先 windows 看看，别急着重启一遍。"
                ),
            })
        }
        _ => unreachable!(),
    }
}

/// 按字符截断（UIA 名字可能很长，整串塞进回包会挤爆 token）。
fn clip(s: &str, n: usize) -> String {
    let mut out: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        out.push('…');
    }
    out
}

fn xy(v: &Value) -> Result<(i32, i32), String> {
    let x = v.get("x").and_then(Value::as_i64).ok_or("缺少 x（屏幕坐标）")? as i32;
    let y = v.get("y").and_then(Value::as_i64).ok_or("缺少 y（屏幕坐标）")? as i32;
    Ok((x, y))
}

fn pair(v: &Value, key: &str) -> Result<(i32, i32), String> {
    let a = v.get(key).and_then(Value::as_array).ok_or_else(|| format!("缺少 {key}（[x,y] 数组）"))?;
    if a.len() != 2 {
        return Err(format!("{key} 得是 [x, y] 两元素"));
    }
    Ok((a[0].as_i64().unwrap_or(0) as i32, a[1].as_i64().unwrap_or(0) as i32))
}

fn cmd_screenshot(v: &Value, max_chars: usize) -> Result<String, String> {
    let (bgra, w, h, stride) = screen::capture_bgra()?;
    let (vx, vy, _, _) = screen::virtual_screen();
    // region（屏幕坐标）→ 裁一块；不给就是整屏。scale 只用来把局部放大看小控件。
    let (ox, oy, bx, by, rw, rh) = parse_region(v, vx, vy, w, h)?;
    let want_scale = v
        .get("scale")
        .or_else(|| v.get("region").and_then(|r| r.get("scale")))
        .and_then(Value::as_u64)
        .unwrap_or(1)
        .clamp(1, 4) as u32;
    // 局部图只放大到内联宽度上限以内（再大就白烧 token；面板看落盘那份）
    let scale = want_scale.min((INLINE_MAX_WIDTH / rw.max(1)).max(1));
    // 全屏同色 = 锁屏/安全桌面/黑屏——说人话，别让模型对全黑图硬分析
    let solid_hint = if screen::is_solid_color(&bgra) {
        "\n注意：整屏同色（疑似锁屏/安全桌面/黑屏）——先确认用户在场，别对着纯色图做判断。"
    } else {
        ""
    };
    let whole = bx == 0 && by == 0 && rw == w && rh == h && scale == 1;
    let (buf, cw, ch, cstride) = if whole {
        (std::borrow::Cow::Borrowed(&bgra[..]), w, h, stride)
    } else {
        let (cw, ch, buf, cstride) = png::crop_bgra(&bgra, w, h, stride, (bx, by, rw, rh), scale)?;
        (std::borrow::Cow::Owned(buf), cw, ch, cstride)
    };
    // 全尺寸 PNG：落盘给人/面板看
    let png_full = png::encode_png_from_bgra(&buf, cw, ch, cstride)?;
    let dir = shots_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("建截图目录失败: {e}"))?;
    let tag = if whole { "" } else { "_region" };
    let name = format!("computer_{}{tag}.png", chrono::Local::now().format("%Y%m%d_%H%M%S_%3f"));
    let rel = format!("screenshots/{name}");
    std::fs::write(dir.join(&name), &png_full).map_err(|e| format!("写截图失败: {e}"))?;
    // 内联版：超限宽先最近邻降采样再编码（token 账；面板看全尺寸那份）
    let (inline_png, iw, ih) = match png::downscale_bgra(&buf, cw, ch, cstride, INLINE_MAX_WIDTH) {
        Some((nw, nh, raw)) => (png::encode_png_from_bgra(&raw, nw, nh, (nw as usize) * 4)?, nw, nh),
        None => (png_full.clone(), cw, ch),
    };
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &inline_png);
    record_action(&format!("截图 {cw}×{ch} → {rel}"), Some(rel.clone()), Some((cw, ch)));
    // 换算比例写成字面量，模型不用自己推
    let eff = iw as f64 / rw.max(1) as f64;
    let text = if whole {
        format!(
            "已截图 {w}×{h}（内联 {iw}×{ih}，全尺寸存 {rel}，用户可在面板看）。\
             你看到的图是**内联缩略图** {iw}×{ih}，给 click/move/scroll/drag 的坐标必须是**屏幕坐标** \
             =（图上的像素坐标 × {w}/{iw}），直接拿图上的像素值会点偏。\
             小控件（开关/圆点/小按钮）别在这张缩略图上估：用 screenshot 配 region={{\"x\",\"y\",\"w\",\"h\"}} \
             截局部，1:1 时图上像素 + 左上角坐标就是屏幕坐标。{solid_hint}"
        )
    } else {
        // 想放大但区域太宽放不下时明说，别让模型以为拿到了 2×/3×
        let zoom_note = if want_scale > scale {
            format!(
                "（你要 {want_scale}×，但区域宽 {rw} 放不下——想看更大就把 w 缩到 {} 以内再截）",
                INLINE_MAX_WIDTH / want_scale
            )
        } else {
            String::new()
        };
        format!(
            "已截取屏幕区域 ({ox},{oy}) {rw}×{rh}，图 {iw}×{ih}（放大 {eff:.2}×，全尺寸存 {rel}）{zoom_note}。\
             换算：**屏幕坐标 = 图上像素 ÷ {eff:.2} + ({ox},{oy})**；量准了再 click。\
             没看到目标就把 w/h 一次加倍再截（别在原地截同样大的一小块反复试）。{solid_hint}"
        )
    };
    // 只压文本部分——标记+base64 原样拼尾部（压缩会掐中段，绝不动它）
    let text = crate::context::compress(&text, max_chars);
    Ok(format!("{text}{IMG_OPEN}{b64}{IMG_CLOSE}"))
}

/// 解析 screenshot 的 region（屏幕坐标）→ (屏幕左上角 ox,oy, 缓冲偏移 bx,by, 宽, 高)。
/// 缺省整屏；超出屏幕的部分夹掉（左上角按夹完的真实位置报，模型换算才不飘）。
fn parse_region(v: &Value, vx: i32, vy: i32, w: u32, h: u32) -> Result<(i32, i32, u32, u32, u32, u32), String> {
    let Some(r) = v.get("region").filter(|r| r.is_object()) else {
        return Ok((vx, vy, 0, 0, w, h));
    };
    let num = |k: &str| r.get(k).and_then(Value::as_i64).ok_or_else(|| format!("region 需要 {k}（屏幕坐标整数）"));
    let (x, y, rw, rh) = (num("x")? as i32, num("y")? as i32, num("w")? as i32, num("h")? as i32);
    if rw <= 0 || rh <= 0 {
        return Err("region 的 w/h 必须大于 0".into());
    }
    let left = x.max(vx);
    let top = y.max(vy);
    let right = x.saturating_add(rw).min(vx.saturating_add(w as i32));
    let bottom = y.saturating_add(rh).min(vy.saturating_add(h as i32));
    if right <= left || bottom <= top {
        return Err(format!(
            "region ({x},{y}) {rw}×{rh} 完全在屏幕外（当前屏幕 {vx},{vy} {w}×{h}）"
        ));
    }
    Ok((
        left,
        top,
        (left - vx) as u32,
        (top - vy) as u32,
        (right - left) as u32,
        (bottom - top) as u32,
    ))
}

/// 点击回执的取景尺寸（屏幕像素）。**2 倍放大**回给模型：开关/圆点这类小控件
/// 在 1:1 下只有十几像素，模型量不准（2026-11-02 二次实锤：连着三次点偏 70px）。
/// 取景 480×270 屏幕像素 → 图上 960×540，放大后仍在 1440 内联上限以内。
const RECEIPT_W: u32 = 480;
const RECEIPT_H: u32 = 270;
const RECEIPT_SCALE: u32 = 2;

/// 以点击点为中心取一块缓冲坐标 (bx,by,bw,bh)，夹在屏幕内。
/// 参数：point=点击点，origin=虚拟屏左上角，screen=屏幕尺寸，size=取景尺寸。
/// 贴边时中心会偏移——所以回执文案报的是**实际左上角**，模型按它换算。
fn centered_rect(
    point: (i32, i32),
    origin: (i32, i32),
    screen: (u32, u32),
    size: (u32, u32),
) -> (u32, u32, u32, u32) {
    let ((x, y), (vx, vy), (sw, sh), (cw, ch)) = (point, origin, screen, size);
    let bw = cw.min(sw);
    let bh = ch.min(sh);
    let want_x = x - bw as i32 / 2;
    let want_y = y - bh as i32 / 2;
    let left = want_x.clamp(vx, vx + (sw - bw) as i32);
    let top = want_y.clamp(vy, vy + (sh - bh) as i32);
    ((left - vx) as u32, (top - vy) as u32, bw, bh)
}

/// 「原地连点」判定：离上一次点击不到 20 屏幕像素就直说。
/// 不拦（项目铁律：不做变相自动终止的护栏），只把事实摆给模型看。
fn repeat_note(prev: Option<(i32, i32)>, now: (i32, i32)) -> String {
    let Some((px, py)) = prev else { return String::new() };
    let d = (((now.0 - px).pow(2) + (now.1 - py).pow(2)) as f64).sqrt();
    if d < 20.0 {
        format!("⚠️ 这次离上一次点击 ({px},{py}) 只有 {d:.0} 像素——同一个点再点一次不会改变结果。")
    } else {
        String::new()
    }
}

/// 点击回执：点完稍等界面反应，截一块以点击点为中心的 2 倍放大局部图回给模型，
/// 图上画红色十字标出「点在哪」，并报告这块区域点完有没有变化。
///
/// 三件事对应三种失败（2026-11-02 真机日志 99995f34）：模型点偏 70px 不自知 →
/// 十字给出可见的「我点的」；1:1 太小看不清 → 放大 2 倍；点完没反应还在原地微调 →
/// 前后像素对比直接说「这块没变化」。
fn click_receipt(
    x: i32,
    y: i32,
    before: Option<(Vec<u8>, u32, u32, usize)>,
    prev_click: Option<(i32, i32)>,
) -> Option<(String, String)> {
    // 切换动画/弹层要一点时间才落定，太早截会拿到过渡帧
    std::thread::sleep(std::time::Duration::from_millis(280));
    let (after, w, h, stride) = screen::capture_bgra().ok()?;
    let (vx, vy, _, _) = screen::virtual_screen();
    let (bx, by, bw, bh) = centered_rect((x, y), (vx, vy), (w, h), (RECEIPT_W, RECEIPT_H));
    let (cw, ch, mut buf, cstride) =
        png::crop_bgra(&after, w, h, stride, (bx, by, bw, bh), RECEIPT_SCALE).ok()?;
    let (ox, oy) = (bx as i32 + vx, by as i32 + vy);
    // 十字画在点击点（图上像素 = 屏幕差 × 放大倍数），中间留空不糊住目标
    let cx = ((x - ox).max(0) as u32) * RECEIPT_SCALE;
    let cy = ((y - oy).max(0) as u32) * RECEIPT_SCALE;
    png::draw_crosshair(&mut buf, cw, ch, cstride, (cx, cy), 26, 5);
    let png_bytes = png::encode_png_from_bgra(&buf, cw, ch, cstride).ok()?;
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &png_bytes);
    // 前后对比：只比 1:1 的同块区域（便宜，且不受十字影响）
    let change = before.and_then(|(prev, pw, ph, pstride)| {
        if (pw, ph) != (w, h) {
            return None; // 分辨率中途变了（切显示器/锁屏），这轮不给结论
        }
        let (_, _, prev_c, _) = png::crop_bgra(&prev, pw, ph, pstride, (bx, by, bw, bh), 1).ok()?;
        let (aw, ah, cur_c, cur_s) = png::crop_bgra(&after, w, h, stride, (bx, by, bw, bh), 1).ok()?;
        Some(png::diff_ratio(&prev_c, &cur_c, aw, ah, cur_s))
    });
    let change_text = match change {
        Some(r) if r < 0.002 => "这块区域点完**没有可见变化**——开关/按钮类说明没点中，别在原地微调，按图中目标的位置整段改坐标。".to_string(),
        Some(r) => format!("这块区域点完有变化（{:.1}% 像素变了）。", r * 100.0),
        None => String::new(),
    };
    // 原地连点是最常见的空转（真机日志 99995f34：1013→1020→1006，三下没挪窝）——
    // 不拦（不设自动终止），但把事实摆出来
    let repeat_text = repeat_note(prev_click, (x, y));
    let text = format!(
        "回执：点击点周边 {bw}×{bh} 屏幕像素，放大 {RECEIPT_SCALE}×（图 {cw}×{ch}，屏幕 ({ox},{oy}) 起）。\
         **红色十字就是你的点击点**，中心留空处才是那一点本身。\
         换算：屏幕坐标 = 图上像素 ÷ {RECEIPT_SCALE} + ({ox},{oy})。\
         {change_text}{repeat_text}看十字压在目标上没有：没压住就量「十字→目标」的图上距离，÷{RECEIPT_SCALE} 就是屏幕上的偏差，改坐标再点一次。"
    );
    Some((text, b64))
}




#[cfg(test)]
mod tests {
    use super::*;

    /// launch 的入参守卫：没有 path 直接报错（绝不拿空串去 ShellExecute）。
    #[test]
    fn launch_requires_path() {
        let err = dispatch(&serde_json::json!({ "method": "launch" }), "launch", 4096)
            .expect_err("空 path 应该报错");
        assert!(err.contains("path"), "错误要说清缺 path：{err}");
    }

    /// launch 不是只读方法（会拉起进程，要按 Normal 走审批闸）。
    #[test]
    fn launch_is_not_read_only() {
        assert!(!method_is_read_only("launch"));
        assert!(ALL_METHODS.contains(&"launch"));
    }

    /// region 解析：夹屏、缺省整屏、完全在外报错（换算的底数，错了坐标全飘）。
    #[test]
    fn region_parses_and_clamps() {
        let (ox, oy, bx, by, rw, rh) =
            parse_region(&serde_json::json!({"region": {"x": -50, "y": 10, "w": 100, "h": 50}}), 0, 0, 1920, 1080)
                .unwrap();
        assert_eq!((ox, oy), (0, 10), "超出左边的部分夹掉，左上角报真实位置");
        assert_eq!((bx, by, rw, rh), (0, 10, 50, 50), "宽度按夹完的算");
        // 缺省 region = 整屏
        let (ox, oy, bx, by, rw, rh) = parse_region(&serde_json::json!({}), 0, 0, 1920, 1080).unwrap();
        assert_eq!((ox, oy, bx, by, rw, rh), (0, 0, 0, 0, 1920, 1080));
        // 完全在屏幕外 → 报错（别给一张全黑图让模型瞎猜）
        assert!(parse_region(&serde_json::json!({"region": {"x": 5000, "y": 0, "w": 10, "h": 10}}), 0, 0, 1920, 1080).is_err());
        // 缺字段 / 非正尺寸 → 报错
        assert!(parse_region(&serde_json::json!({"region": {"x": 1, "y": 1, "w": 10}}), 0, 0, 1920, 1080).is_err());
        assert!(parse_region(&serde_json::json!({"region": {"x": 1, "y": 1, "w": 0, "h": 10}}), 0, 0, 1920, 1080).is_err());
    }

    /// 原地连点提示：<20px 要说，远了不说，没点过也不说。
    #[test]
    fn repeat_note_warns_only_when_stuck() {
        assert_eq!(repeat_note(None, (100, 100)), "");
        assert!(repeat_note(Some((1013, 962)), (1020, 962)).contains("只有 7 像素"));
        assert_eq!(repeat_note(Some((1013, 962)), (1013, 1031)), "", "差 69px 是有效修正，别啰嗦");
    }

    /// 点击回执取景：居中；贴边夹住（这时中心偏移，靠回执里的左上角换算）。
    #[test]
    fn receipt_rect_centers_and_clamps() {
        let size = (RECEIPT_W, RECEIPT_H);
        let (bx, by, bw, bh) = centered_rect((1000, 700), (0, 0), (2560, 1440), size);
        assert_eq!((bw, bh), (RECEIPT_W, RECEIPT_H));
        assert_eq!((bx, by), (1000 - 240, 700 - 135));
        assert_eq!(centered_rect((10, 10), (0, 0), (2560, 1440), size), (0, 0, RECEIPT_W, RECEIPT_H));
        assert_eq!(
            centered_rect((2559, 1439), (0, 0), (2560, 1440), size),
            (2560 - RECEIPT_W, 1440 - RECEIPT_H, RECEIPT_W, RECEIPT_H)
        );
        // 副屏在左（虚拟屏起点为负）时按缓冲坐标走
        assert_eq!(centered_rect((-1920, 100), (-1920, 0), (4480, 1440), size), (0, 0, RECEIPT_W, RECEIPT_H));
        // 屏幕比取景还小：整屏，不外扩
        assert_eq!(centered_rect((100, 100), (0, 0), (320, 200), size), (0, 0, 320, 200));
    }
}

#[cfg(test)]
mod real_input_tests {
    //! 真实键鼠 e2e（默认忽略，PAWCH_AGENT_COMPUTER_E2E=1 才跑；旧名 AGENT_COMPUTER_E2E 兼容一个版本）。
    //! 靶子是记事本：自己拉起来、自己聚焦、自己输入、自己收。**绝不碰用户前台**——
    //! 没有 PAWCH_AGENT_COMPUTER_E2E 环境变量时整个测试跳过（CI/别人机器上不误触）。
    use super::*;

    #[test]
    #[ignore = "真实键鼠：PAWCH_AGENT_COMPUTER_E2E=1 才跑（会动你的桌面）"]
    fn type_into_notepad_lands() {
        if crate::config::env_pawch("COMPUTER_E2E").is_none() {
            return;
        }
        // 靶子记事本：自己拉、自己收（Child 句柄 kill，不留进程）
        let mut child = std::process::Command::new("notepad.exe")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .stdin(std::process::Stdio::null())
            .spawn()
            .expect("拉起记事本");
        std::thread::sleep(std::time::Duration::from_millis(2000));
        let w = window::find_window("记事本")
            .or_else(|| window::find_window("Notepad"))
            .expect("找到记事本窗口");
        assert!(window::focus_window(w.hwnd).is_ok(), "聚焦调用不报错");
        std::thread::sleep(std::time::Duration::from_millis(600));
        assert!(
            window::foreground_title().is_some_and(|t| t.contains("记事本") || t.contains("Notepad")),
            "前台应是记事本"
        );
        // 1) unicode 输入落地（中文/符号同路径，UTF-16 码元直灌）
        let marker = format!("PAWCH_AGENT_E2E_{}", std::process::id());
        input::type_text(&marker);
        std::thread::sleep(std::time::Duration::from_millis(400));
        let c1 = window::edit_control_text(w.hwnd).unwrap_or_default();
        assert!(c1.contains(&marker), "unicode 输入应落进编辑框，实际：{c1:?}");
        // 2) 鼠标点击定位：点编辑控件左上角内侧，光标离开行尾
        let Some((_eh, (el, et, _, _))) = window::edit_control_rect(w.hwnd) else {
            child.kill().ok();
            panic!("找不到记事本的 Edit 子控件");
        };
        input::click(el + 12, et + 8, input::MouseButton::Left, 1);
        std::thread::sleep(std::time::Duration::from_millis(300));
        input::type_text("X");
        std::thread::sleep(std::time::Duration::from_millis(400));
        let c2 = window::edit_control_text(w.hwnd).unwrap_or_default();
        assert_ne!(c2, format!("{marker}X"), "鼠标点击应把光标从行尾移走（实际：{c2:?}）");
        // 点击把光标挪到了文本中段（marker 被 X 断开=没落在行尾，正是鼠标定位的证据）
        assert!(c2.contains('X') && c2.len() == marker.len() + 1, "X 应插在文本中：{c2:?}");
        // 3) 组合键 end：光标到行尾，Y 落在最后
        input::key_combo("end").expect("end 键");
        std::thread::sleep(std::time::Duration::from_millis(200));
        input::type_text("Y");
        std::thread::sleep(std::time::Duration::from_millis(400));
        let c3 = window::edit_control_text(w.hwnd).unwrap_or_default();
        assert!(c3.ends_with('Y'), "end 组合键应把光标移行尾（实际：{c3:?}");
        // 4) 截图通路（顺路验真实桌面能出图）
        let (bgra, sw, sh, stride) = screen::capture_bgra().expect("截图成功");
        assert!(sw > 0 && sh > 0);
        let png = png::encode_png_from_bgra(&bgra, sw, sh, stride).expect("PNG 编码成功");
        assert!(png.starts_with(&[0x89u8, b'P', b'N', b'G']), "PNG 签名");
        assert!(png.len() > 10_000, "截图应有实际内容（{} 字节）", png.len());
        // 5) 收拾：关窗不保存（alt+f4 → 对话框按 N）；句柄 kill 兜底。
        // 发键前先确认前台仍是我们拉起的记事本——否则 alt+f4 会打到桌面/别的窗口
        // （桌面上无焦点窗口时 Windows 会弹「关闭 Windows」确认框，2026-10-21 实锤）。
        let still_ours = window::foreground_title()
            .is_some_and(|t| t.contains("记事本") || t.contains("Notepad"));
        if still_ours {
            let _ = input::key_combo("alt+f4");
            std::thread::sleep(std::time::Duration::from_millis(400));
            let _ = input::key_combo("n");
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        child.kill().ok();
        child.wait().ok();
    }
}

#[cfg(test)]
mod desktop_e2e {
    //! 真桌面 e2e（默认忽略，PAWCH_AGENT_COMPUTER_E2E=1 才跑）。
    //! 只做三件低风险的事：读开始菜单解析名字、开一个自己的临时目录窗口再关掉、
    //! 截局部图核尺寸。**不点鼠标、不发键盘、不碰用户已开的窗口**。
    use super::*;

    fn enabled() -> bool {
        crate::config::env_pawch("COMPUTER_E2E").is_some()
    }

    #[test]
    #[ignore = "真实机器：PAWCH_AGENT_COMPUTER_E2E=1 才跑"]
    fn resolve_finds_real_shortcuts() {
        if !enabled() {
            return;
        }
        // 机器无关的一条：任何 Windows 的开始菜单里都有 PowerShell 快捷方式
        let p = window::resolve_app_target("powershell").expect("应能找到 powershell 快捷方式");
        assert!(p.is_file(), "解析结果必须是真实文件：{}", p.display());
        // 本机目标只打印不断言（换台机器没有也不该红）
        for want in ["clash verge", "tabbit"] {
            match window::resolve_app_target(want) {
                Ok(p) => println!("[e2e] {want} → {}", p.display()),
                Err(e) => println!("[e2e] {want} → 未找到（{e}）"),
            }
        }
    }

    #[test]
    #[ignore = "真实桌面：PAWCH_AGENT_COMPUTER_E2E=1 才跑（会短暂弹一个资源管理器窗口，自己关）"]
    fn launch_opens_window_and_closes() {
        if !enabled() {
            return;
        }
        // 靶子是自己的临时目录：窗口标题就是目录名，唯一可认，不会撞上用户的应用
        let dir = std::env::temp_dir().join(format!("pawch-computer-e2e-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("建临时目录");
        let before: Vec<isize> = window::list_windows().iter().map(|w| w.hwnd).collect();
        window::shell_execute(&dir.display().to_string(), "", "").expect("打开临时目录");
        let win = window::wait_new_window(&before, std::time::Duration::from_millis(4000))
            .expect("4 秒内应等到新窗口");
        assert!(win.title.contains("pawch-computer-e2e"), "标题要能认出是它：{}", win.title);
        assert!(window::focus_window(win.hwnd).is_ok(), "聚焦调用不报错");
        window::close_window(win.hwnd);
        let mut gone = false;
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(150));
            if !window::list_windows().iter().any(|w| w.hwnd == win.hwnd) {
                gone = true;
                break;
            }
        }
        std::fs::remove_dir_all(&dir).ok();
        assert!(gone, "关窗后不该留残渣（hwnd {}）", win.hwnd);
    }

    #[test]
    #[ignore = "真实桌面：PAWCH_AGENT_COMPUTER_E2E=1 才跑（只读屏幕，不点不敲）"]
    fn region_shot_and_click_receipt_sizes() {
        if !enabled() {
            return;
        }
        let (vx, vy, sw, sh) = screen::virtual_screen();
        assert!(sw > 400 && sh > 300, "屏幕太小，跳过尺寸断言");
        let (x, y) = (vx + 10, vy + 10);
        let dir = shots_dir();
        let before: Vec<std::path::PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| rd.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        // region 局部截图：必须是 1:1（否则模型换算全错）
        let mut out = dispatch(
            &serde_json::json!({"method": "screenshot", "region": {"x": x, "y": y, "w": 400, "h": 300}}),
            "screenshot",
            8192,
        )
        .expect("局部截图");
        let b64 = take_image(&mut out).expect("结果里要带内联图");
        let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &b64).expect("base64");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "PNG 签名");
        let iw = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let ih = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((iw, ih), (400, 300), "region 局部图必须 1:1");
        assert!(out.contains(&format!("({x},{y})")), "文案要报左上角真实坐标：{out}");
        // 点击回执只取图不点击：验证放大倍数与文案（十字像素本身在 png 单测里验）
        let (cx, cy) = (vx + sw as i32 / 2, vy + sh as i32 / 2);
        let (text, b64) = click_receipt(cx, cy, None, None).expect("回执");
        let png = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, &b64).expect("base64");
        assert!(png.starts_with(&[0x89, b'P', b'N', b'G']), "回执 PNG 签名");
        let rw = u32::from_be_bytes(png[16..20].try_into().unwrap());
        let rh = u32::from_be_bytes(png[20..24].try_into().unwrap());
        assert_eq!((rw, rh), (RECEIPT_W * RECEIPT_SCALE, RECEIPT_H * RECEIPT_SCALE), "回执必须 2 倍放大");
        assert!(text.contains("红色十字") && text.contains("÷ 2"), "回执文案要讲十字与换算：{text}");
        // 想亲眼看回执长什么样：PAWCH_AGENT_COMPUTER_E2E_KEEP=1，落到临时目录自己删
        if crate::config::env_pawch("COMPUTER_E2E_KEEP").is_some() {
            let keep = std::env::temp_dir().join("pawch-receipt-e2e.png");
            std::fs::write(&keep, &png).ok();
            println!("[e2e] 回执图 → {}", keep.display());
        }
        // 收尾：删掉这次新落的截图，别在工作区留残渣
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let p = e.path();
                if !before.contains(&p) {
                    std::fs::remove_file(&p).ok();
                }
            }
        }
    }

    /// UIA 真机：打开 Clash Verge，找「系统代理」，行列里必须有一个能点到的开关。
    /// 这是拿真机日志里点偏 70 像素的那一行当靶子——它找得对，模型就不用瞎估了。
    #[test]
    #[ignore = "真实桌面：PAWCH_AGENT_COMPUTER_E2E=1 才跑（会短暂开 Clash Verge 窗口，探完关回托盘）"]
    fn uia_finds_system_proxy_toggle() {
        if !enabled() {
            return;
        }
        let Ok(lnk) = window::resolve_app_target("clash verge") else {
            println!("[e2e] 本机没有 Clash Verge，跳过");
            return;
        };
        // 已经在托盘里的话这次调用只是把窗口唤出来
        let before: Vec<isize> = window::list_windows().iter().map(|w| w.hwnd).collect();
        window::shell_execute(&lnk.display().to_string(), "", "").ok();
        let win = window::wait_new_window(&before, std::time::Duration::from_millis(6000))
            .or_else(|| window::find_window("Clash Verge"));
        let Some(win) = win else {
            println!("[e2e] 等不到 Clash Verge 窗口，跳过");
            return;
        };
        std::thread::sleep(std::time::Duration::from_millis(1200)); // 让界面渲染完
        let out = dispatch(&serde_json::json!({"method":"find","text":"系统代理","hwnd":win.hwnd}), "find", 8192)
            .expect("find 不该失败");
        println!("[e2e] find 回包：\n{out}");
        assert!(out.contains("系统代理"), "必须认出系统代理那一行：{out}");
        assert!(out.contains("中心 ("), "必须给出中心坐标：{out}");
        assert!(out.contains("Button") || out.contains("CheckBox"), "同行要给出可点的开关/按钮：{out}");
        window::close_window(win.hwnd);
    }
}
