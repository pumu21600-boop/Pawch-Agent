//! 窗口枚举与聚焦：EnumWindows 清单 + SetForegroundWindow（带前台锁破解——
//! 后台进程直接 SetForegroundWindow 只会闪任务栏；要挂到**当前前台线程**上再换，
//! 不行再补一次 Alt 解锁，见 focus_window 注释）。
//! 另有 shell_execute：按路径打开应用/文档（相当于双击）——**不在任何沙箱 Job 里**，
//! 进程独立存活（打开应用不该归沙箱管）。

use std::cell::RefCell;

use windows_sys::Win32::Foundation::{BOOL, HWND, LPARAM};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{KEYEVENTF_KEYUP, VK_MENU, keybd_event};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, EnumWindows, GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    IsIconic, IsWindowVisible, ShowWindow, SW_RESTORE,
};
#[cfg(test)]
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, GetClassNameW, PostMessageW, SendMessageTimeoutW,
};

use super::input;

thread_local! {
    /// EnumWindows 回调的收集缓冲（回调是 extern "system" fn，拿不到闭包——
    /// 走线程局部）。枚举同步完成，不会跨线程混。
    static ENUM_BUF: RefCell<Vec<(HWND, String)>> = const { RefCell::new(Vec::new()) };
}

/// 一条可见窗口（句柄 + 标题）。系统壳/不可见/无标题的已滤掉。
#[derive(Debug, Clone)]
pub struct WinInfo {
    pub hwnd: isize,
    pub title: String,
}

/// 枚举当前可见窗口（标题非空）。按 z 序（EnumWindows 序）返回。
pub fn list_windows() -> Vec<WinInfo> {
    ENUM_BUF.with(|b| b.borrow_mut().clear());
    unsafe extern "system" fn cb(hwnd: HWND, _l: LPARAM) -> BOOL {
        unsafe {
            if IsWindowVisible(hwnd) == 0 {
                return 1;
            }
            let mut buf = [0u16; 512];
            let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
            if n <= 0 {
                return 1;
            }
            let title = String::from_utf16_lossy(&buf[..n as usize]);
            if title.trim().is_empty() {
                return 1;
            }
            ENUM_BUF.with(|b| b.borrow_mut().push((hwnd, title)));
            1
        }
    }
    unsafe {
        EnumWindows(Some(cb), 0);
    }
    ENUM_BUF.with(|b| {
        b.borrow()
            .iter()
            .map(|(h, t)| WinInfo {
                hwnd: *h,
                title: t.clone(),
            })
            .collect()
    })
}

/// 按标题子串找窗口（大小写不敏感；多个命中取第一个=最前）。
pub fn find_window(substr: &str) -> Option<WinInfo> {
    let needle = substr.trim().to_lowercase();
    if needle.is_empty() {
        return None;
    }
    list_windows().into_iter().find(|w| w.title.to_lowercase().contains(&needle))
}

/// 把窗口带到前台：最小化先还原 → 挂到**当前前台线程**的输入队列 → 换前台。
///
/// 2026-11-02 修（真机实测 4 次「SetForegroundWindow 被拒」）：原来挂的是**目标**线程，
/// 但真正有换前台资格的是当前前台线程——别人（Pawch Agent）在前台时怎么挂目标都拒。
/// 同机实测「挂前台线程 + 一次 Alt 解锁」一次成功（agent 自己写的 focus_win.ps1 同款）。
/// 失败也返回 Ok(false) 由调用方转告（全屏游戏/安全桌面确实抢不走）。
pub fn focus_window(hwnd: isize) -> Result<bool, String> {
    if hwnd == 0 {
        return Err("窗口句柄为空".into());
    }
    unsafe {
        if IsIconic(hwnd) != 0 {
            ShowWindow(hwnd, SW_RESTORE);
        }
        if GetForegroundWindow() == hwnd {
            return Ok(true);
        }
        use windows_sys::Win32::System::Threading::AttachThreadInput;
        let my_thread = GetCurrentThreadId();
        let fg = GetForegroundWindow();
        let fg_thread = if fg != 0 {
            GetWindowThreadProcessId(fg, std::ptr::null_mut())
        } else {
            0
        };
        // 前台线程的输入队列挂上以后，我们这线程就有「设置前台」的资格
        let attached = fg_thread != 0 && fg_thread != my_thread && AttachThreadInput(my_thread, fg_thread, 1) != 0;
        let mut ok = windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd) != 0;
        if !ok {
            // 前台锁还在：模拟一次 Alt（一条用户输入）再换。孤立 Alt 对绝大多数应用
            // 无副作用，只解锁前台切换；这是 agent 自己在真机上验证过的兜底。
            keybd_event(VK_MENU as u8, 0, 0, 0);
            keybd_event(VK_MENU as u8, 0, KEYEVENTF_KEYUP, 0);
            ok = windows_sys::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd) != 0;
        }
        BringWindowToTop(hwnd);
        if attached {
            AttachThreadInput(my_thread, fg_thread, 0);
        }
        Ok(ok)
    }
}

/// 按路径打开应用/文档（ShellExecute = 双击）：**不在任何沙箱 Job 里**，进程独立
/// 存活，不受 shell 命令结束的清理影响（打开应用不该归沙箱管）。
/// `path` 传 .lnk/.exe 完整路径或系统认识的文件；`.exe` 没给 workdir 时默认用 exe
/// 所在目录（与双击一致，避免应用按 cwd 找资源失败）。
pub fn shell_execute(path: &str, args: &str, workdir: &str) -> Result<(), String> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }
    let path = path.trim();
    let dir = if !workdir.trim().is_empty() {
        workdir.trim().to_string()
    } else {
        let p = std::path::Path::new(path);
        if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("exe")) {
            p.parent().map(|d| d.display().to_string()).unwrap_or_default()
        } else {
            String::new()
        }
    };
    let verb = wide("open");
    let file = wide(path);
    let params = wide(args);
    let dir_w = wide(&dir);
    // SAFETY: 所有字符串都是本地 Vec<u16> 的 NUL 结尾缓冲，调用期间存活；
    // ShellExecuteW 只读这些指针，返回 HINSTANCE 编码的错误码（>32 = 成功）。
    let r = unsafe {
        ShellExecuteW(
            0,
            verb.as_ptr(),
            file.as_ptr(),
            if args.trim().is_empty() { std::ptr::null() } else { params.as_ptr() },
            if dir.is_empty() { std::ptr::null() } else { dir_w.as_ptr() },
            SW_SHOWNORMAL,
        )
    };
    if r as isize > 32 {
        Ok(())
    } else {
        Err(format!("ShellExecute 失败（code {}）：路径或关联程序无效", r as isize))
    }
}

/// 当前前台窗口句柄（UIA 查找默认查它）。
pub fn foreground_hwnd() -> isize {
    unsafe { GetForegroundWindow() }
}

/// 当前前台窗口标题（诊断/验证用）。
pub fn foreground_title() -> Option<String> {
    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd == 0 {
            return None;
        }
        let mut buf = [0u16; 512];
        let n = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        (n > 0).then(|| String::from_utf16_lossy(&buf[..n as usize]))
    }
}

/// 找快捷方式的四个去处：桌面（用户/公共）+ 开始菜单（用户/全局）。
/// 用户 2026-11-02：开应用不该再让模型先 dir 一遍这些目录。
fn app_search_dirs() -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    for key in ["USERPROFILE", "PUBLIC"] {
        if let Ok(p) = std::env::var(key) {
            out.push(std::path::Path::new(&p).join("Desktop"));
        }
    }
    for key in ["APPDATA", "PROGRAMDATA"] {
        if let Ok(p) = std::env::var(key) {
            out.push(std::path::Path::new(&p).join("Microsoft").join("Windows").join("Start Menu").join("Programs"));
        }
    }
    out.into_iter().filter(|p| p.is_dir()).collect()
}

/// 递归收 shortcut（.lnk/.exe/.url），深度限 4 层——开始菜单再深也没意义。
fn collect_shortcuts(dir: &std::path::Path, depth: u8, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if name.starts_with('.') {
            continue;
        }
        let is_dir = e.file_type().map(|t| t.is_dir()).unwrap_or(false);
        if is_dir {
            if depth < 4 {
                collect_shortcuts(&p, depth + 1, out);
            }
            continue;
        }
        let ok = p
            .extension()
            .and_then(|x| x.to_str())
            .is_some_and(|x| x.eq_ignore_ascii_case("lnk") || x.eq_ignore_ascii_case("exe") || x.eq_ignore_ascii_case("url"));
        if ok {
            out.push(p);
        }
    }
}

/// 从候选里挑最像 `want` 的那个：完全同名 > 以它开头 > 包含它；同档优先 .lnk
/// （.lnk 带启动参数/工作目录，比裸 exe 更接近用户双击）。纯函数，好测。
///
/// 比较前把大小写/空格/连字符抹平——用户说 "Clash Verge"，快捷方式叫
/// "clash-verge.lnk"，不归一化就永远匹配不上。
fn pick_best(cands: &[std::path::PathBuf], want: &str) -> Option<std::path::PathBuf> {
    fn norm(s: &str) -> String {
        s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
    }
    let w = norm(want);
    if w.is_empty() {
        return None;
    }
    let mut best: Option<(u8, bool, &std::path::PathBuf)> = None;
    for p in cands {
        let stem = norm(p.file_stem().and_then(|s| s.to_str()).unwrap_or(""));
        if stem.is_empty() {
            continue;
        }
        let rank = if stem == w {
            0
        } else if stem.starts_with(&w) {
            1
        } else if stem.contains(&w) {
            2
        } else {
            continue;
        };
        let is_lnk = p.extension().and_then(|x| x.to_str()).is_some_and(|x| x.eq_ignore_ascii_case("lnk"));
        let better = match best {
            None => true,
            Some((br, bl, _)) => rank < br || (rank == br && is_lnk && !bl),
        };
        if better {
            best = Some((rank, is_lnk, p));
        }
    }
    best.map(|(_, _, p)| p.clone())
}

/// 启动目标解析：给的是存在的文件就直接用；否则当应用名，去桌面/开始菜单找。
pub fn resolve_app_target(input: &str) -> Result<std::path::PathBuf, String> {
    let t = input.trim().trim_matches('"');
    if t.is_empty() {
        return Err("launch 需要应用名或路径".into());
    }
    let p = std::path::Path::new(t);
    if p.is_file() {
        return Ok(p.to_path_buf());
    }
    let dirs = app_search_dirs();
    let mut cands = Vec::new();
    for d in &dirs {
        collect_shortcuts(d, 0, &mut cands);
    }
    if let Some(hit) = pick_best(&cands, t) {
        return Ok(hit);
    }
    Err(format!(
        "没找到「{t}」的快捷方式（搜了桌面和开始菜单，共 {} 个候选）。\
         给 .lnk/.exe 的完整路径，或把名字写全一点（如 \"Tabbit Browser\"）。",
        cands.len()
    ))
}

/// 等一个新窗口出现（启动后轮询；z 序最前的先返回）。
/// 只在 `before` 之外找——启动前已经开着的窗口不算，避免把旧窗口当成启动成功。
pub fn wait_new_window(before: &[isize], timeout: std::time::Duration) -> Option<WinInfo> {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if let Some(w) = list_windows().into_iter().find(|w| !before.contains(&w.hwnd)) {
            return Some(w);
        }
        if std::time::Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(std::time::Duration::from_millis(150));
    }
}

/// 按应用名找窗口：先整体，再退到第一个够长的词（"Tabbit Browser" 的窗口标题
/// 往往只有「新标签页 - Tabbit」）。给「已经在运行」的兜底用。
pub fn find_window_loose(name: &str) -> Option<WinInfo> {
    if let Some(w) = find_window(name) {
        return Some(w);
    }
    name.split_whitespace()
        .filter(|t| t.chars().count() >= 3)
        .find_map(find_window)
}

/// 给指定窗口发 WM_CLOSE（点对点，不碰键盘/前台）。
/// 只给 opt-in 真机 e2e 收尾用——测试自己拉起来的窗口自己关掉，别留残渣。
#[cfg(test)]
pub fn close_window(hwnd: isize) {
    unsafe {
        PostMessageW(hwnd, 0x0010, 0, 0); // WM_CLOSE
    }
}

thread_local! {
    /// EnumChildWindows 的收集缓冲（同 ENUM_BUF 的路数）。
    static CHILD_BUF: RefCell<Vec<isize>> = const { RefCell::new(Vec::new()) };
}

/// 找窗口内第一个编辑控件的 (句柄, 屏幕 rect)——鼠标点击定位用（点它左上角
/// = 文本最前面）。仅 opt-in 真机 e2e（mod.rs 的 real_input_tests）用。
#[cfg(test)]
pub fn edit_control_rect(hwnd: isize) -> Option<(isize, (i32, i32, i32, i32))> {
    CHILD_BUF.with(|b| b.borrow_mut().clear());
    unsafe extern "system" fn cb(child: isize, _l: LPARAM) -> BOOL {
        unsafe {
            let mut cls = [0u16; 64];
            let n = GetClassNameW(child, cls.as_mut_ptr(), cls.len() as i32);
            if n > 0 {
                let name = String::from_utf16_lossy(&cls[..n as usize]);
                if name == "Edit" || name == "RichEditD2DPT" {
                    CHILD_BUF.with(|b| b.borrow_mut().push(child));
                }
            }
            1
        }
    }
    unsafe {
        EnumChildWindows(hwnd, Some(cb), 0);
    }
    let children: Vec<isize> = CHILD_BUF.with(|b| b.borrow().clone());
    let child = children.into_iter().next()?;
    unsafe {
        let mut r = windows_sys::Win32::Foundation::RECT { left: 0, top: 0, right: 0, bottom: 0 };
        (windows_sys::Win32::UI::WindowsAndMessaging::GetWindowRect(child, &mut r) != 0)
            .then_some((child, (r.left, r.top, r.right, r.bottom)))
    }
}

/// 读窗口内**编辑控件的文本**（记事本/输入框类：找 class="Edit" 的子窗口）。
/// 验证键鼠输入落点用——记事本标题只显示文件名，内容在 Edit 子窗口里。
/// 仅 opt-in 真机 e2e 用。
#[cfg(test)]
pub fn edit_control_text(hwnd: isize) -> Option<String> {
    CHILD_BUF.with(|b| b.borrow_mut().clear());
    unsafe extern "system" fn cb(child: isize, _l: LPARAM) -> BOOL {
        unsafe {
            let mut cls = [0u16; 64];
            let n = GetClassNameW(child, cls.as_mut_ptr(), cls.len() as i32);
            if n > 0 {
                let name = String::from_utf16_lossy(&cls[..n as usize]);
                if name == "Edit" || name == "RichEditD2DPT" {
                    CHILD_BUF.with(|b| b.borrow_mut().push(child));
                }
            }
            1
        }
    }
    unsafe {
        EnumChildWindows(hwnd, Some(cb), 0);
    }
    let children: Vec<isize> = CHILD_BUF.with(|b| b.borrow().clone());
    for child in children {
        // 跨进程读文本走 SendMessageTimeoutW + WM_GETTEXT——GetWindowTextW 对别的
        // 进程的窗口经常返空（UIPI/ marshalling 限制，实测记事本就读不到）。
        unsafe {
            let mut buf = [0u16; 4096];
            let mut got: usize = 0;
            let r = SendMessageTimeoutW(
                child,
                0x000D, // WM_GETTEXT
                buf.len(),
                buf.as_mut_ptr() as isize,
                2u32, // SMTO_ABORTIFHUNG（常量被特性门控，用字面量）
                200,
                &mut got,
            );
            if r != 0 && got > 0 {
                let n = got.min(buf.len() - 1);
                return Some(String::from_utf16_lossy(&buf[..n]));
            }
        }
    }
    None
}

/// 键盘输入落点诊断：当前焦点窗口 + 大写锁定/插入等切换键状态（人话一行）。
/// 让模型在「敲了键盘没反应」时先看这儿，而不是盲目重敲。
pub fn focus_diagnostics() -> String {
    let fg = foreground_title().unwrap_or_else(|| "(取不到前台窗口)".into());
    let caps = input::is_key_down(0x14); // VK_CAPITAL
    format!("当前前台窗口：{fg}；大写锁定：{}；光标位置：{:?}", if caps { "开" } else { "关" }, super::screen::cursor_pos())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_windows_returns_titled_windows() {
        // CI/无头环境可能一个都没有——不断言非空，只断言不崩且形状正确
        let wins = list_windows();
        for w in &wins {
            assert!(!w.title.trim().is_empty(), "无标题窗口不该进清单");
        }
    }

    #[test]
    fn find_window_missing_returns_none() {
        assert!(find_window("绝不可能存在的窗口标题 xyz123").is_none());
        assert!(find_window("  ").is_none(), "空子串不瞎找");
    }

    #[test]
    fn focus_null_handle_errors() {
        assert!(focus_window(0).is_err());
    }

    #[test]
    fn pick_best_ranks_and_normalizes() {
        use std::path::PathBuf;
        let cands = vec![
            PathBuf::from(r"C:\x\Tabbit Browser.lnk"),
            PathBuf::from(r"C:\x\Unrelated.exe"),
            PathBuf::from(r"C:\x\Tabbit.exe"),
            PathBuf::from(r"C:\x\clash-verge.lnk"),
        ];
        // 完全同名优先（哪怕另一个是以它开头的 .lnk）
        assert_eq!(pick_best(&cands, "tabbit"), Some(PathBuf::from(r"C:\x\Tabbit.exe")));
        // 前缀匹配
        assert_eq!(pick_best(&cands, "tabbit browser"), Some(PathBuf::from(r"C:\x\Tabbit Browser.lnk")));
        // 包含匹配
        assert_eq!(pick_best(&cands, "browser"), Some(PathBuf::from(r"C:\x\Tabbit Browser.lnk")));
        // 空格/连字符抹平：用户嘴上说的 "Clash Verge" 对得上 clash-verge.lnk
        assert_eq!(pick_best(&cands, "Clash Verge"), Some(PathBuf::from(r"C:\x\clash-verge.lnk")));
        assert!(pick_best(&cands, "nope").is_none());
        assert!(pick_best(&cands, "   ").is_none(), "空名字不瞎猜");
    }

    #[test]
    fn resolve_app_target_rejects_empty() {
        assert!(resolve_app_target("  ").is_err());
        // 不存在的名字：报错要带「给完整路径」的出路，别让模型自己瞎猜
        let err = resolve_app_target("绝不可能存在的应用名 xyz123").unwrap_err();
        assert!(err.contains("没找到"), "{err}");
    }
}
