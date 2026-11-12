//! 数据管理：安装目录/数据目录展示、迁移、导出、导入。
//!
//! 定位：用户换机/换盘时的唯一入口。原则与启动迁移一致——**只复制、不删旧数据**；
//! 写完指针后提示重启生效（路径在进程内是 OnceLock，运行中切不了）。

use super::workspace::with_owner;
use super::*;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// 同一时刻只允许一个数据操作（迁移/导出都要整目录扫，并发会互相拖慢/抢盘）。
static DATA_OP: Mutex<()> = Mutex::new(());

/// 导入进度（导入本身排他，单实例足够）：前端轮询 GET /api/data/import/progress。
#[derive(Clone, serde::Serialize)]
pub(super) struct ImportProgress {
    target: String,
    total_bytes: u64,
    total_files: usize,
    done: bool,
    error: Option<String>,
}

static IMPORT_PROGRESS: Mutex<Option<ImportProgress>> = Mutex::new(None);

/// 最近一次迁移/替换导入留下的旧目录（清理接口只认记录里的路径，绝不接受前端任意路径删除）。
///
/// 迁移会重启后端，静态变量跨不过进程重启 → 同时落盘到新数据目录的
/// `.migration-old.json`，清理接口优先读文件。
/// `delete=false` = 只在等用户回答「删不删」；用户点「删除」才置 true 并开始清。
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct OldDirs {
    #[serde(default)]
    install: Option<String>,
    #[serde(default)]
    data: Option<String>,
    #[serde(default)]
    delete: bool,
}

static LAST_OLD_DIRS: Mutex<Option<OldDirs>> = Mutex::new(None);

fn old_dirs_file(data: &Path) -> PathBuf {
    data.join(".migration-old.json")
}

fn save_old_dirs(data: &Path, old: &OldDirs) {
    if let Ok(bytes) = serde_json::to_vec(old) {
        let _ = std::fs::write(old_dirs_file(data), bytes);
    }
}

fn load_old_dirs() -> Option<OldDirs> {
    let p = old_dirs_file(&crate::config::data_root());
    let text = std::fs::read(p).ok()?;
    serde_json::from_slice(&text).ok()
}

#[derive(Debug, Deserialize)]
pub(super) struct MigrateReq {
    #[serde(default)]
    pub install_dir: String,
    #[serde(default)]
    pub data_dir: String,
    #[serde(default)]
    pub include_cache: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct PickReq {
    /// data（选新数据目录）/ export（选导出 zip）/ import（选导入 zip）
    pub kind: String,
    /// import 时选目标目录用
    #[serde(default)]
    pub for_import: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct MoveReq {
    pub target: String,
    #[serde(default)]
    pub include_cache: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct ExportReq {
    #[serde(default)]
    pub dest: String,
    #[serde(default)]
    pub include_cache: bool,
}

#[derive(Debug, Deserialize)]
pub(super) struct ImportReq {
    #[serde(default)]
    pub zip: String,
    #[serde(default)]
    pub target: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct OpenReq {
    /// install / data / cache
    pub which: String,
}

fn ptr_path() -> PathBuf {
    crate::config::data_dir_pointer()
}

fn write_pointer(dir: &Path) -> Result<(), String> {
    let p = ptr_path();
    if let Some(parent) = p.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建指针目录失败: {e}"))?;
    }
    std::fs::write(&p, dir.display().to_string()).map_err(|e| format!("写数据目录指针失败: {e}"))?;
    hide_pointer();
    Ok(())
}

/// 指针目录/文件设隐藏：用户在资源管理器里默认看不到，不会顺手删。
fn hide_pointer() {
    use std::os::windows::process::CommandExt;
    let p = ptr_path();
    if let Some(parent) = p.parent() {
        let _ = std::process::Command::new("attrib")
            .args(["+h"])
            .arg(parent)
            .creation_flags(0x0800_0000)
            .status();
    }
    if p.exists() {
        let _ = std::process::Command::new("attrib")
            .args(["+h"])
            .arg(&p)
            .creation_flags(0x0800_0000)
            .status();
    }
}

fn dir_size(path: &Path) -> u64 {
    fn walk(p: &Path, total: &mut u64) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() {
                walk(&e.path(), total);
            } else if ty.is_file()
                && let Ok(m) = e.metadata()
            {
                *total += m.len();
            }
        }
    }
    let mut total = 0;
    walk(path, &mut total);
    total
}

/// 资源管理器/系统自己往文件夹里塞的隐藏文件：Explorer 看着是空的，别当成「不是空目录」。
/// 用户手动新建的目录常带 desktop.ini（设过图标/视图）/ Thumbs.db，直接拦会让人以为程序不让建。
fn is_os_junk(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "desktop.ini" | "thumbs.db" | ".ds_store"
    )
}

fn count_entries(dir: &Path) -> usize {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter(|e| !is_os_junk(&e.file_name().to_string_lossy()))
                .count()
        })
        .unwrap_or(0)
}

/// 目录当前体积 + 文件数（导入进度用；解压过程中目录在增长）。
fn dir_stat(path: &Path) -> (u64, usize) {
    fn walk(p: &Path, bytes: &mut u64, files: &mut usize) {
        let Ok(rd) = std::fs::read_dir(p) else { return };
        for e in rd.flatten() {
            let Ok(ty) = e.file_type() else { continue };
            if ty.is_dir() {
                walk(&e.path(), bytes, files);
            } else if ty.is_file()
                && let Ok(m) = e.metadata()
            {
                *bytes += m.len();
                *files += 1;
            }
        }
    }
    let mut bytes = 0;
    let mut files = 0;
    walk(path, &mut bytes, &mut files);
    (bytes, files)
}

/// 从 zip 中央目录读总量（进度条分母）。PowerShell 一条命令；失败返回 (0,0)（进度条退化）。
fn zip_stat(zip: &Path) -> (u64, usize) {
    use std::os::windows::process::CommandExt;
    let script = format!(
        "Add-Type -AssemblyName System.IO.Compression.FileSystem; \
         $z = [System.IO.Compression.ZipFile]::OpenRead({}); \
         $b = ($z.Entries | Measure-Object Length -Sum).Sum; \
         $n = @($z.Entries | Where-Object {{ $_.Name -ne '' }}).Count; \
         $z.Dispose(); \
         Write-Output ('{{0}}|{{1}}' -f [int64]$b, [int64]$n)",
        ps_quote(zip)
    );
    let Ok(out) = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .creation_flags(0x0800_0000)
        .output()
    else {
        return (0, 0);
    };
    let s = String::from_utf8_lossy(&out.stdout);
    let mut it = s.trim().split('|');
    let bytes = it.next().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(0);
    let files = it.next().and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(0);
    (bytes, files)
}

/// 写指针后**真正重启后端**（数据目录是服务进程启动时定死的，只重开壳没用）。
///
/// 起一个 detached PowerShell：等 HTTP 响应发完 → `stop_exe stop` → 再从
/// `start_exe` 以 `--minimized` 拉起来。壳有 5s 看门狗宽限，重启 ~2s 不会把它带走。
/// 迁移安装目录时 stop_exe=旧目录（停旧服务）、start_exe=新目录（起新服务）。
///
/// 关键：新进程的环境变量里把 `PAWCH_AGENT_DATA_DIR` 设成新目录。
/// 服务由壳拉起时壳会给它设这个变量（优先级高于指针），helper 继承下来就是**旧目录**，
/// 不清/不改的话新服务还会用旧数据目录（2026-11-03 真机实测踩到：导入后重启没生效）。
fn spawn_service_restart(stop_exe: &Path, start_exe: &Path, data: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let start_dir = start_exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let script = format!(
        "$env:PAWCH_AGENT_DATA_DIR = {}; \
         Remove-Item Env:AGENT_DATA_DIR -ErrorAction SilentlyContinue; \
         Start-Sleep -Milliseconds 1200; \
         & {} stop | Out-Null; \
         Start-Sleep -Milliseconds 900; \
         Start-Process -FilePath {} -ArgumentList '--minimized' -WorkingDirectory {}",
        ps_quote(data),
        ps_quote(stop_exe),
        ps_quote(start_exe),
        ps_quote(&start_dir)
    );
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-Command", &script])
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动重启任务失败: {e}"))
}

/// 路径相等（大小写不敏感、忽略末尾分隔符）。
fn same_path(a: &Path, b: &Path) -> bool {
    let norm = |p: &Path| -> String {
        p.display()
            .to_string()
            .trim_end_matches(['\\', '/'])
            .to_lowercase()
    };
    norm(a) == norm(b)
}

/// 两个路径相同或互相包含（复制会递归到自己）。
fn paths_conflict(a: &Path, b: &Path) -> bool {
    is_same_or_within(a, b) || is_same_or_within(b, a)
}

/// 迁移安装目录后更新快捷方式 + 卸载项（都指向新路径）。
fn update_shortcuts_registry(install: &Path) -> Result<(), String> {
    let shell_exe = install.join("Pawch Agent.exe");
    let target = if shell_exe.is_file() {
        shell_exe
    } else {
        install.join("pawch-agent.exe")
    };
    let uninstaller = install.join("Uninstall.exe");
    let script = "$ws = New-Object -ComObject WScript.Shell; $dirs = @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs')); foreach ($d in $dirs) { if ($d) { $s = $ws.CreateShortcut((Join-Path $d 'Pawch Agent.lnk')); $s.TargetPath = __T__; $s.WorkingDirectory = __WD__; $s.IconLocation = __T__ + ',0'; $s.Save() } }; $k = 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PawchAgent'; if (Test-Path $k) { Set-ItemProperty -Path $k -Name InstallLocation -Value __WD__; Set-ItemProperty -Path $k -Name DisplayIcon -Value (__T__ + ',0'); Set-ItemProperty -Path $k -Name UninstallString -Value (__U__ + ' --uninstall'); Set-ItemProperty -Path $k -Name QuietUninstallString -Value (__U__ + ' --uninstall --silent') }"
        .replace("__T__", &ps_quote(&target))
        .replace("__WD__", &ps_quote(install))
        .replace("__U__", &ps_quote(&uninstaller));
    run_powershell(&script)
}

/// 尝试删除迁移留下的旧目录；删不掉（还开着的壳占着 exe/缓存）的留到下一次再试。
/// 返回 (已删, 被占用, 拒绝删除的非法路径)。
fn try_cleanup(old: &OldDirs) -> (Vec<String>, Vec<String>, Vec<String>) {
    let cur_install = crate::config::app_root();
    let cur_data = crate::config::data_root();
    let mut deleted = Vec::new();
    let mut locked = Vec::new();
    let mut refused = Vec::new();
    for p in [old.install.clone(), old.data.clone()].into_iter().flatten() {
        let path = PathBuf::from(&p);
        // 安全闸：只删迁移记录里的旧目录，且绝不能是当前目录本身或其父目录
        if !path.is_absolute()
            || same_path(&path, &cur_install)
            || same_path(&path, &cur_data)
            || is_same_or_within(&cur_install, &path)
            || is_same_or_within(&cur_data, &path)
        {
            refused.push(p);
            continue;
        }
        if !path.exists() {
            deleted.push(p);
            continue;
        }
        match std::fs::remove_dir_all(&path) {
            Ok(()) => deleted.push(p),
            Err(_) => locked.push(p),
        }
    }
    (deleted, locked, refused)
}

/// 启动旧目录清理线程：`.migration-old.json` 里 delete=true 才动手，每 5s 试删一次；
/// 被占用的等壳/进程退出后自动删掉。服务重启后从文件恢复，不依赖外部常驻进程。
pub(crate) fn start_cleanup_worker() {
    // 旧版壁纸在 cache 里（不会被导出/迁移）：启动时先挪到数据目录根
    migrate_wallpaper_from_cache();
    hide_pointer();
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            let Some(old) = load_old_dirs() else { continue };
            if !old.delete {
                continue; // 用户还没点「删除」
            }
            let (_deleted, locked, _refused) = try_cleanup(&old);
            if locked.is_empty() {
                let _ = std::fs::remove_file(old_dirs_file(&crate::config::data_root()));
            }
        }
    });
}

struct FindCabinet {
    title: String,
    found: windows_sys::Win32::Foundation::HWND,
}

unsafe extern "system" fn enum_cabinet(
    hwnd: windows_sys::Win32::Foundation::HWND,
    lparam: isize,
) -> i32 {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let ctx = unsafe { &mut *(lparam as *mut FindCabinet) };
    unsafe {
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut cls = [0u16; 64];
        let n = GetClassNameW(hwnd, cls.as_mut_ptr(), cls.len() as i32);
        if n <= 0 || String::from_utf16_lossy(&cls[..n as usize]) != "CabinetWClass" {
            return 1;
        }
        let mut buf = [0u16; 512];
        let m = GetWindowTextW(hwnd, buf.as_mut_ptr(), buf.len() as i32);
        if m <= 0 {
            return 1;
        }
        if String::from_utf16_lossy(&buf[..m as usize]) == ctx.title {
            ctx.found = hwnd;
            return 0;
        }
    }
    1
}

/// 兜底（没有壳/纯浏览器模式）：后台进程拉起的资源管理器窗口可能落在应用后面，
/// 等它出现后尽力顶到前台（前台权限不够时 SetForegroundWindow 会静默失败，无害）。
fn raise_explorer_window(path: &Path) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let title = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    if title.is_empty() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut ctx = FindCabinet { title: title.clone(), found: 0 };
            unsafe {
                EnumWindows(Some(enum_cabinet), &mut ctx as *mut _ as isize);
                if ctx.found != 0 {
                    SetForegroundWindow(ctx.found);
                    BringWindowToTop(ctx.found);
                    return;
                }
            }
        }
    });
}

/// 规范化路径（不存在时用父目录规范化后拼文件名），用于判断包含关系。
fn normalize_path(p: &Path) -> PathBuf {
    if let Ok(c) = std::fs::canonicalize(p) {
        return c;
    }
    if let (Some(parent), Some(name)) = (p.parent(), p.file_name())
        && let Ok(cp) = std::fs::canonicalize(parent)
    {
        return cp.join(name);
    }
    p.to_path_buf()
}

/// `child` 是否等于或位于 `parent` 之内（Windows 大小写不敏感）。
/// 迁移时用来挡住「新目录选在当前数据目录里面」——那样复制会递归到自己。
fn is_same_or_within(child: &Path, parent: &Path) -> bool {
    let norm = |p: &Path| -> Vec<String> {
        normalize_path(p)
            .components()
            .map(|c| c.as_os_str().to_string_lossy().to_lowercase())
            .collect()
    };
    let c = norm(child);
    let p = norm(parent);
    c.len() >= p.len() && c[..p.len()] == p[..]
}

#[derive(Default)]
struct CopyStats {
    files: usize,
    bytes: u64,
    skipped: Vec<String>,
}

/// 递归复制（覆盖同名；`skip` 里的顶层目录跳过）。旧数据不动。
fn copy_tree(src: &Path, dst: &Path, skip: &[&str], st: &mut CopyStats) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| format!("建目录失败（{}）: {e}", dst.display()))?;
    let rd = std::fs::read_dir(src).map_err(|e| format!("读目录失败（{}）: {e}", src.display()))?;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().to_string();
        if skip.contains(&name.as_str()) {
            st.skipped.push(name);
            continue;
        }
        let from = e.path();
        let to = dst.join(&name);
        let Ok(ty) = e.file_type() else { continue };
        if ty.is_dir() {
            copy_tree(&from, &to, &[], st)?;
        } else if ty.is_file() {
            match std::fs::copy(&from, &to) {
                Ok(n) => {
                    st.files += 1;
                    st.bytes += n;
                }
                Err(e) => return Err(format!("复制 {} 失败: {e}", from.display())),
            }
        }
    }
    Ok(())
}

fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "''"))
}

fn run_powershell(script: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
        .output()
        .map_err(|e| format!("启动 powershell 失败: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "powershell 执行失败: {}",
            String::from_utf8_lossy(&out.stderr).chars().take(500).collect::<String>()
        ))
    }
}

fn zip_dir(src: &Path, dest: &Path) -> Result<(), String> {
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("建导出目录失败: {e}"))?;
    }
    // Get-ChildItem -Force：把隐藏文件也打进去（普通 -Path * 会漏点文件）
    let script = format!(
        "Get-ChildItem -LiteralPath {} -Force | Compress-Archive -DestinationPath {} -Force -CompressionLevel Optimal",
        ps_quote(src),
        ps_quote(dest)
    );
    run_powershell(&script)
}

fn unzip_to(zip: &Path, dest: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dest).map_err(|e| format!("建目标目录失败: {e}"))?;
    let script = format!(
        "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
        ps_quote(zip),
        ps_quote(dest)
    );
    run_powershell(&script)
}

/// GET /api/data/info —— 安装目录 / 数据目录 / 缓存 / 指针，以及体积。
static STARTED_MS: std::sync::OnceLock<u64> = std::sync::OnceLock::new();

/// 本进程的「启动标记」：前端用它判断后端是否真的重启过（数据目录路径可能不变）。
fn started_at() -> u64 {
    *STARTED_MS.get_or_init(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    })
}

pub(super) async fn data_info() -> Json<Value> {
    let install = crate::config::app_root();
    let data = crate::config::data_root();
    let cache = crate::config::cache_root();
    let (data_bytes, cache_bytes) = tokio::task::spawn_blocking({
        let d = data.clone();
        let c = cache.clone();
        move || {
            let cache_bytes = dir_size(&c);
            // cache 在数据目录里面：单独展示，不重复计进「当前数据」
            (dir_size(&d).saturating_sub(cache_bytes), cache_bytes)
        }
    })
    .await
    .unwrap_or((0, 0));
    Json(json!({
        "install_dir": install.display().to_string(),
        "data_dir": data.display().to_string(),
        "cache_dir": cache.display().to_string(),
        "pointer": ptr_path().display().to_string(),
        "pointer_exists": ptr_path().is_file(),
        "data_bytes": data_bytes,
        "cache_bytes": cache_bytes,
        "started_at": started_at(),
    }))
}

/// POST /api/data/adopt —— 指针丢了 / 想用已有数据目录：只写指针并重启，不复制、不覆盖。
#[derive(Debug, Deserialize)]
pub(super) struct AdoptReq {
    pub data_dir: String,
}

pub(super) async fn data_adopt(
    Json(req): Json<AdoptReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let target = PathBuf::from(req.data_dir.trim());
    if !target.is_absolute() {
        return Err(err_json("数据目录要填完整路径"));
    }
    if !target.is_dir() {
        return Err(err_json(format!("目录不存在：{}", target.display())));
    }
    let looks_like_data = target.join("settings.json").is_file()
        || target.join("index.json").is_file()
        || target.join("logs").is_dir();
    if !looks_like_data {
        return Err(err_json(format!(
            "这个目录不像 Pawch 数据目录：{}",
            target.display()
        )));
    }
    write_pointer(&target).map_err(err_json)?;
    let exe = std::env::current_exe().map_err(|e| err_json(format!("取服务路径失败: {e}")))?;
    let restarting = spawn_service_restart(&exe, &exe, &target).is_ok();
    Ok(Json(json!({
        "ok": true,
        "data_dir": target.display().to_string(),
        "restarting": restarting,
    })))
}

/// POST /api/data/pick —— 原生目录/文件选择框（服务进程弹窗，与工作区选择同一套）。
pub(super) async fn data_pick(Json(req): Json<PickReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let kind = req.kind.clone();
    let for_import = req.for_import;
    let picked = tokio::task::spawn_blocking(move || -> Option<String> {
        match kind.as_str() {
            "install" => with_owner(
                rfd::FileDialog::new().set_title("选择安装目录（程序本体；选空目录或新建）"),
            )
            .pick_folder()
            .map(|p| p.display().to_string()),
            "data" => with_owner(
                rfd::FileDialog::new()
                    .set_title("选择应用数据目录（设置、对话、日志、插件、缓存都会放这里）"),
            )
            .pick_folder()
            .map(|p| p.display().to_string()),
            "export" => with_owner(
                rfd::FileDialog::new()
                    .set_title("导出数据（保存为 zip）")
                    .set_file_name(format!(
                        "pawch-agent-data-{}.zip",
                        chrono::Local::now().format("%Y%m%d-%H%M%S")
                    ))
                    .add_filter("zip", &["zip"]),
            )
            .save_file()
            .map(|p| p.display().to_string()),
            "import" => {
                if for_import {
                    with_owner(
                        rfd::FileDialog::new().set_title("选择导入后的数据目录（建议新目录）"),
                    )
                    .pick_folder()
                    .map(|p| p.display().to_string())
                } else {
                    with_owner(
                        rfd::FileDialog::new()
                            .set_title("选择要导入的数据 zip")
                            .add_filter("zip", &["zip"]),
                    )
                    .pick_file()
                    .map(|p| p.display().to_string())
                }
            }
            _ => None,
        }
    })
    .await
    .map_err(|e| err_json(format!("选择窗口失败: {e}")))?;
    Ok(Json(json!({ "ok": true, "path": picked })))
}

/// POST /api/data/move —— 全量迁移到新数据目录（只复制、不删旧；写指针，重启生效）。
pub(super) async fn data_move(Json(req): Json<MoveReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let target = PathBuf::from(req.target.trim());
    let current = crate::config::data_root();
    let include_cache = req.include_cache;
    if req.target.trim().is_empty() {
        return Err(err_json("目标目录为空"));
    }
    if target == current {
        return Err(err_json("新目录和当前数据目录相同"));
    }
    if is_same_or_within(&target, &current) {
        return Err(err_json("新目录不能放在当前数据目录里面：复制会递归到自己，请选外面的空目录"));
    }
    if target.exists() && count_entries(&target) > 0 {
        return Err(err_json("目标目录不是空目录：请选一个空目录或新建目录，避免覆盖已有文件"));
    }
    let res = tokio::task::spawn_blocking({
        let target = target.clone();
        move || -> Result<CopyStats, String> {
            let _guard = DATA_OP.lock().unwrap_or_else(|e| e.into_inner());
            let mut st = CopyStats::default();
            let skip: &[&str] = if include_cache { &[] } else { &["cache"] };
            copy_tree(&current, &target, skip, &mut st)?;
            write_pointer(&target)?;
            Ok(st)
        }
    })
    .await
    .map_err(|e| err_json(format!("迁移任务失败: {e}")))?;
    let st = res.map_err(err_json)?;
    observability::event(
        EventType::State,
        json!({ "kind": "data.moved", "to": target.display().to_string(), "files": st.files, "bytes": st.bytes, "cache": include_cache }),
    );
    // 迁移完也要真正重启后端（数据目录在服务进程启动时定死）。
    let restarting = std::env::current_exe()
        .ok()
        .map(|exe| spawn_service_restart(&exe, &exe, &target).is_ok())
        .unwrap_or(false);
    Ok(Json(json!({
        "ok": true,
        "target": target.display().to_string(),
        "files": st.files,
        "bytes": st.bytes,
        "skipped": st.skipped,
        "restart_required": true,
        "restarting": restarting,
    })))
}

/// POST /api/data/migrate —— 按输入框的新路径迁移（安装目录 / 数据目录可单独或同时）。
///
/// 只复制不删旧；复制完自动重启后端（安装目录变了就从新目录起服务）；旧目录记在
/// LAST_OLD_DIRS 里，等前端问「删不删」再走 /api/data/cleanup_old。
pub(super) async fn data_migrate(Json(req): Json<MigrateReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let cur_install = crate::config::app_root();
    let cur_data = crate::config::data_root();
    let want_install = req.install_dir.trim();
    let want_data = req.data_dir.trim();
    let new_install = PathBuf::from(want_install);
    let new_data = PathBuf::from(want_data);
    let install_changed = !want_install.is_empty() && !same_path(&new_install, &cur_install);
    let data_changed = !want_data.is_empty() && !same_path(&new_data, &cur_data);
    if !install_changed && !data_changed {
        return Err(err_json("两个目录都没变：先改成新目录再点迁移"));
    }
    if install_changed {
        if !new_install.is_absolute() {
            return Err(err_json("安装目录要填完整路径，例如 D:\\Pawch Agent"));
        }
        if !cur_install.join("version.txt").is_file() {
            return Err(err_json(
                "当前是开发模式（从仓库目录运行），不支持迁移安装目录；用安装包装的版本才有安装目录可迁",
            ));
        }
        if paths_conflict(&new_install, &cur_install) || same_path(&new_install, &cur_data) {
            return Err(err_json("新安装目录不能是当前安装目录本身/子目录，也不能和当前数据目录重合"));
        }
        if new_install.exists() && count_entries(&new_install) > 0 {
            return Err(err_json(format!(
                "新安装目录不是空目录：{}（已有 {} 项；请清空或换一个不存在的路径）",
                new_install.display(),
                count_entries(&new_install)
            )));
        }
    }
    if data_changed {
        if !new_data.is_absolute() {
            return Err(err_json("数据目录要填完整路径，例如 D:\\pawch-data"));
        }
        if paths_conflict(&new_data, &cur_data) || same_path(&new_data, &cur_install) {
            return Err(err_json("新数据目录不能是当前数据目录本身/子目录，也不能和当前安装目录重合"));
        }
        if new_data.exists() && count_entries(&new_data) > 0 {
            return Err(err_json(format!(
                "新数据目录不是空目录：{}（已有 {} 项；请清空或换一个不存在的路径）",
                new_data.display(),
                count_entries(&new_data)
            )));
        }
    }
    if install_changed && data_changed && paths_conflict(&new_install, &new_data) {
        return Err(err_json("安装目录和数据目录不能相同或互相包含"));
    }
    // 手动填的路径不存在时自动建空目录（用户要的行为）：先建好，复制阶段直接往里写
    if install_changed && !new_install.exists() {
        std::fs::create_dir_all(&new_install).map_err(|e| {
            err_json(format!("创建安装目录失败（{}）: {e}", new_install.display()))
        })?;
    }
    if data_changed && !new_data.exists() {
        std::fs::create_dir_all(&new_data)
            .map_err(|e| err_json(format!("创建数据目录失败（{}）: {e}", new_data.display())))?;
    }

    let install_src = cur_install.clone();
    let install_dst = new_install.clone();
    let data_src = cur_data.clone();
    let data_dst = new_data.clone();
    let include_cache = false; // 迁移固定不复制 cache：可再生，且 WebView2 锁文件会打断迁移
    let _ = req.include_cache; // 字段保留兼容旧前端，实际忽略
    let res = tokio::task::spawn_blocking(move || -> Result<(CopyStats, CopyStats), String> {
        let _guard = DATA_OP.lock().unwrap_or_else(|e| e.into_inner());
        let mut st_install = CopyStats::default();
        let mut st_data = CopyStats::default();
        if data_changed {
            copy_tree(&data_src, &data_dst, &["cache"], &mut st_data)?;
            st_data.skipped.retain(|s| s.as_str() != "cache");
        }
        if install_changed {
            copy_tree(&install_src, &install_dst, &[], &mut st_install)?;
            if !install_dst.join("pawch-agent.exe").is_file() {
                return Err("复制完的新安装目录里没有 pawch-agent.exe（安装目录可能不完整）".to_string());
            }
        }
        // 指针与快捷方式最后写：前面任何一步失败都不让系统指向半成品
        if install_changed {
            update_shortcuts_registry(&install_dst)?;
        }
        if data_changed {
            write_pointer(&data_dst)?;
        }
        Ok((st_install, st_data))
    })
    .await;
    let (st_install, st_data) = match res {
        Ok(Ok(v)) => v,
        Ok(Err(e)) => {
            // 失败时收掉本次复制出来的半成品，否则重试会被「目标不是空目录」拦住
            if install_changed {
                let _ = std::fs::remove_dir_all(&new_install);
            }
            if data_changed {
                let _ = std::fs::remove_dir_all(&new_data);
            }
            return Err(err_json(e));
        }
        Err(e) => return Err(err_json(format!("迁移任务失败: {e}"))),
    };

    let old = OldDirs {
        install: if install_changed { Some(cur_install.display().to_string()) } else { None },
        data: if data_changed { Some(cur_data.display().to_string()) } else { None },
        delete: false,
    };
    if old.install.is_some() || old.data.is_some() {
        let mut g = LAST_OLD_DIRS.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(old.clone());
        // 迁移会重启后端，静态变量跨不过进程 → 落到新数据目录里给新进程读
        let active = if data_changed { &new_data } else { &cur_data };
        let _ = std::fs::create_dir_all(active);
        save_old_dirs(active, &old);
    }
    observability::event(
        EventType::State,
        json!({
            "kind": "data.migrated",
            "install_changed": install_changed,
            "data_changed": data_changed,
            "files_install": st_install.files,
            "files_data": st_data.files,
            "bytes_data": st_data.bytes,
            "skipped": st_data.skipped.len(),
            "include_cache": include_cache,
        }),
    );

    let stop_exe = std::env::current_exe().map_err(|e| err_json(format!("取服务路径失败: {e}")))?;
    let start_exe = if install_changed {
        new_install.join("pawch-agent.exe")
    } else {
        stop_exe.clone()
    };
    let restart_data = if data_changed { new_data.clone() } else { cur_data.clone() };
    let restarting = spawn_service_restart(&stop_exe, &start_exe, &restart_data).is_ok();

    Ok(Json(json!({
        "ok": true,
        "install_changed": install_changed,
        "data_changed": data_changed,
        "install_dir": if install_changed { new_install.display().to_string() } else { cur_install.display().to_string() },
        "data_dir": if data_changed { new_data.display().to_string() } else { cur_data.display().to_string() },
        "old_install": old.install,
        "old_data": old.data,
        "files_install": st_install.files,
        "files_data": st_data.files,
        "bytes_data": st_data.bytes,
        "skipped": st_data.skipped.len(),
        "restarting": restarting,
    })))
}

/// POST /api/data/cleanup_old —— 删除最近一次迁移的旧目录（只认 LAST_OLD_DIRS，
/// 绝不接受前端传任意路径）。删不掉（壳还开着旧 exe/缓存）的排队，退出后自动删。
pub(super) async fn data_cleanup_old() -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mut old = load_old_dirs().or_else(|| {
        let g = LAST_OLD_DIRS.lock().unwrap_or_else(|e| e.into_inner());
        g.clone()
    });
    let Some(mut old) = old.take() else {
        return Err(err_json("没有待清理的旧目录（先做一次迁移或导入）"));
    };
    // 用户点「删除」：置位并落盘，后台线程会把删不掉的继续重试
    old.delete = true;
    save_old_dirs(&crate::config::data_root(), &old);
    {
        let mut g = LAST_OLD_DIRS.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(old.clone());
    }
    let (deleted, locked, refused) = tokio::task::spawn_blocking(move || try_cleanup(&old))
        .await
        .map_err(|e| err_json(format!("清理任务失败: {e}")))?;
    // 都处理完（删掉或判定非法）就清记录；还有被占用的留着，后台线程继续等
    if locked.is_empty() {
        let _ = std::fs::remove_file(old_dirs_file(&crate::config::data_root()));
        let mut g = LAST_OLD_DIRS.lock().unwrap_or_else(|e| e.into_inner());
        *g = None;
    }
    observability::event(
        EventType::State,
        json!({ "kind": "data.cleanup_old", "deleted": deleted, "scheduled": locked, "failed": refused }),
    );
    Ok(Json(json!({
        "ok": true,
        "deleted": deleted,
        "scheduled": locked,
        "failed": refused,
    })))
}

/// POST /api/data/forget_old —— 用户选「我自己删」：丢掉旧目录记录，不删任何东西。
pub(super) async fn data_forget_old() -> Json<Value> {
    let _ = std::fs::remove_file(old_dirs_file(&crate::config::data_root()));
    {
        let mut g = LAST_OLD_DIRS.lock().unwrap_or_else(|e| e.into_inner());
        *g = None;
    }
    Json(json!({ "ok": true }))
}

/// POST /api/data/restart_app —— 重启整个应用（服务 + 壳）。
/// 迁移安装目录/换数据目录后，旧壳还占着旧目录里的 exe 和缓存，必须退出才能删。
pub(super) async fn data_restart_app() -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let install = crate::config::app_root();
    let data = crate::config::data_root();
    spawn_app_restart(&install, &data).map_err(err_json)?;
    Ok(Json(json!({ "ok": true })))
}

/// POST /api/data/export —— 导出数据目录为 zip（默认不含 cache；旧目录不动）。
pub(super) async fn data_export(Json(req): Json<ExportReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let current = crate::config::data_root();
    let include_cache = req.include_cache;
    let dest = if req.dest.trim().is_empty() {
        let picked = tokio::task::spawn_blocking(|| {
            with_owner(
                rfd::FileDialog::new()
                    .set_title("导出数据（保存为 zip）")
                    .set_file_name(format!(
                        "pawch-agent-data-{}.zip",
                        chrono::Local::now().format("%Y%m%d-%H%M%S")
                    ))
                    .add_filter("zip", &["zip"]),
            )
            .save_file()
        })
        .await
        .map_err(|e| err_json(format!("选择窗口失败: {e}")))?;
        match picked {
            Some(p) => p,
            None => return Ok(Json(json!({ "ok": false, "cancelled": true }))),
        }
    } else {
        PathBuf::from(req.dest.trim())
    };
    let res = tokio::task::spawn_blocking({
        let dest = dest.clone();
        move || -> Result<(CopyStats, PathBuf), String> {
            let _guard = DATA_OP.lock().unwrap_or_else(|e| e.into_inner());
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis())
                .unwrap_or(0);
            let stage = std::env::temp_dir().join(format!("pawch-export-{}-{stamp}", std::process::id()));
            let mut st = CopyStats::default();
            // 访问令牌不进导出包：导出常被拿去备份/换机，别把本机凭证一起带走
            // （数据目录搬迁/migrate 仍照常复制 token——那是同一台机换位置，保持凭证连续）
            let skip: &[&str] = if include_cache {
                &["server.token"]
            } else {
                &["cache", "server.token"]
            };
            let r = (|| {
                copy_tree(&current, &stage, skip, &mut st)?;
                zip_dir(&stage, &dest)?;
                Ok(())
            })();
            let _ = std::fs::remove_dir_all(&stage);
            r.map(|_| (st, dest))
        }
    })
    .await
    .map_err(|e| err_json(format!("导出任务失败: {e}")))?;
    let (st, dest) = res.map_err(err_json)?;
    let bytes = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    observability::event(
        EventType::State,
        json!({ "kind": "data.exported", "path": dest.display().to_string(), "files": st.files, "zip_bytes": bytes }),
    );
    Ok(Json(json!({
        "ok": true,
        "path": dest.display().to_string(),
        "files": st.files,
        "zip_bytes": bytes,
        "skipped": st.skipped,
    })))
}

/// 数据目录旁边的临时目录（staging / 旧数据备份）：同卷兄弟目录，移动是秒级 rename。
fn sibling_dir(current: &Path, suffix: &str) -> PathBuf {
    let parent = current.parent().unwrap_or_else(|| Path::new("."));
    let stem = current
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "pawch-data".to_string());
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    let mut p = parent.join(format!("{stem}-{suffix}-{stamp}"));
    let mut n = 1;
    while p.exists() {
        p = parent.join(format!("{stem}-{suffix}-{stamp}-{n}"));
        n += 1;
    }
    p
}

/// 替换式导入的切换 helper：等响应发完 → 停服务 → 旧数据（除 cache）挪到备份 →
/// staging 挪进数据目录 → 写旧目录记录（等用户回答删不删）→ 重新拉起服务。
///
/// 为什么不直接覆盖：服务正开着日志/设置文件，Windows 不让覆盖；cache 被壳的
/// WebView2 占着也不能动。所以先解压到同卷 staging，停服务后整目录 rename 交换，
/// cache 原地不动（壳不用重启也能继续用）。
fn spawn_swap_helper(staging: &Path, data: &Path, backup: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("取服务路径失败: {e}"))?;
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let script = format!(
        "$env:PAWCH_AGENT_DATA_DIR = {data_q}; \
         Remove-Item Env:AGENT_DATA_DIR -ErrorAction SilentlyContinue; \
         Start-Sleep -Milliseconds 1200; \
         & {exe_q} stop | Out-Null; \
         Start-Sleep -Milliseconds 900; \
         New-Item -ItemType Directory -Path {backup_q} -Force | Out-Null; \
         Get-ChildItem -LiteralPath {data_q} -Force | Where-Object {{ $_.Name -ne 'cache' }} | ForEach-Object {{ Move-Item -LiteralPath $_.FullName -Destination {backup_q} -Force -ErrorAction SilentlyContinue }}; \
         Get-ChildItem -LiteralPath {staging_q} -Force | ForEach-Object {{ Move-Item -LiteralPath $_.FullName -Destination {data_q} -Force -ErrorAction SilentlyContinue }}; \
         if (Test-Path (Join-Path {backup_q} 'server.token')) {{ Copy-Item -LiteralPath (Join-Path {backup_q} 'server.token') -Destination (Join-Path {data_q} 'server.token') -Force }}; \
         Remove-Item -LiteralPath {staging_q} -Recurse -Force -ErrorAction SilentlyContinue; \
         $rec = @{{ install = $null; data = {backup_q}; delete = $false }} | ConvertTo-Json -Compress; \
         [System.IO.File]::WriteAllText((Join-Path {data_q} '.migration-old.json'), $rec, (New-Object System.Text.UTF8Encoding($false))); \
         Start-Process -FilePath {exe_q} -ArgumentList '--minimized' -WorkingDirectory {dir_q}",
        data_q = ps_quote(data),
        exe_q = ps_quote(&exe),
        backup_q = ps_quote(backup),
        staging_q = ps_quote(staging),
        dir_q = ps_quote(&dir),
    );
    spawn_ps_file(&script, "swap")
}

/// 把长 PowerShell 脚本写成带 BOM 的临时 .ps1 再 `-File` 执行。
///
/// 长脚本走 `-Command` 容易踩引号/解析坑（2026-11-03 实测：替换导入的 helper 静默失败，
/// 服务没停、staging 没换）。改成文件 + stdout/stderr 落日志，失败有尸检材料。
/// 用完即收（2026-11-03 待修项：迁移/导入 helper 每次留一对 ps1+log）：脚本尾部
/// 自删 .ps1；日志句柄要等 PowerShell 退出才释放，另起一个延迟 30s 的小清理。
fn spawn_ps_file(script: &str, tag: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let stamp = std::process::id();
    let path = std::env::temp_dir().join(format!("pawch-{tag}-{stamp}.ps1"));
    let log = std::env::temp_dir().join(format!("pawch-{tag}-{stamp}.log"));
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(script.as_bytes());
    bytes.extend_from_slice(
        format!(
            "\nRemove-Item -LiteralPath {} -ErrorAction SilentlyContinue\n",
            ps_quote(&path)
        )
        .as_bytes(),
    );
    std::fs::write(&path, bytes).map_err(|e| format!("写临时脚本失败: {e}"))?;
    let out = std::fs::File::create(&log).map_err(|e| format!("建日志失败: {e}"))?;
    let err = out.try_clone().map_err(|e| format!("复制日志句柄失败: {e}"))?;
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&path)
        .stdout(out)
        .stderr(err)
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 PowerShell 失败: {e}"))?;
    // 日志延迟清理：单条语句走 -Command（长组合命令仍必须走 .ps1，见上）
    let _ = std::process::Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-Command",
            &format!(
                "Start-Sleep -Seconds 30; Remove-Item -LiteralPath {} -ErrorAction SilentlyContinue",
                ps_quote(&log)
            ),
        ])
        .creation_flags(0x0800_0000)
        .spawn();
    Ok(())
}

/// 启动清扫（2026-11-03 待修项的兜底）：%TEMP% 里 pawch 自己的残留——
/// helper 的 ps1/log（>2 小时）与壳的 lock/log（>24 小时，壳崩溃时的兜底）。只删旧文件，
/// 正在跑的实例不受影响。失败静默（清扫是卫生，不是业务）。
pub(crate) fn sweep_stale_temp() {
    std::thread::spawn(|| {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let tmp = std::env::temp_dir();
        let Ok(rd) = std::fs::read_dir(&tmp) else { return };
        for entry in rd.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let min_age: u64 = if name.starts_with("pawch-agent-shell.") {
                24 * 3600
            } else if name.starts_with("pawch-") && (name.ends_with(".ps1") || name.ends_with(".log")) {
                2 * 3600
            } else {
                continue;
            };
            // 修改时间早于 (now - min_age) 才算陈旧
            let stale = entry
                .metadata()
                .ok()
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() + min_age < now)
                .unwrap_or(false);
            if stale {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    });
}

/// 重启整个应用（服务 + 壳）：迁移安装目录/换数据目录后，旧壳占着旧 exe 和缓存，
/// 不退出就删不掉旧目录；从新安装目录重新打开壳，环境变量指到当前数据目录。
fn spawn_app_restart(install: &Path, data: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("取服务路径失败: {e}"))?;
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
    let shell = install.join("Pawch Agent.exe");
    let start = if shell.is_file() {
        format!(
            "Start-Process -FilePath {} -WorkingDirectory {}",
            ps_quote(&shell),
            ps_quote(install)
        )
    } else {
        // 没有壳（纯服务/开发模式）：只把服务重新拉起来
        format!(
            "Start-Process -FilePath {} -ArgumentList '--minimized' -WorkingDirectory {}",
            ps_quote(&exe),
            ps_quote(&dir)
        )
    };
    let script = format!(
        "$env:PAWCH_AGENT_DATA_DIR = {data_q}; \
         Remove-Item Env:AGENT_DATA_DIR -ErrorAction SilentlyContinue; \
         Start-Sleep -Milliseconds 1200; \
         & {exe_q} stop | Out-Null; \
         taskkill /IM 'Pawch Agent.exe' /F | Out-Null; \
         Start-Sleep -Milliseconds 900; \
         {start}",
        data_q = ps_quote(data),
        exe_q = ps_quote(&exe),
    );
    spawn_ps_file(&script, "restart")
}

/// POST /api/data/import —— 用 zip 里的数据**替换当前数据目录**（同路径原地换）。
///
/// 流程：解压到同卷 staging → helper 停服务 → 旧数据（除 cache）挪到备份目录 →
/// staging 挪进数据目录 → 写旧目录记录（delete=false，等用户回答）→ 拉服务。
/// 指定 `target` 时保留旧行为（解到该空目录 + 切指针），给自动化/兼容用。
pub(super) async fn data_import(Json(req): Json<ImportReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let zip = if req.zip.trim().is_empty() {
        let picked = tokio::task::spawn_blocking(|| {
            with_owner(
                rfd::FileDialog::new()
                    .set_title("选择要导入的数据 zip")
                    .add_filter("zip", &["zip"]),
            )
            .pick_file()
        })
        .await
        .map_err(|e| err_json(format!("选择窗口失败: {e}")))?;
        match picked {
            Some(p) => p,
            None => return Ok(Json(json!({ "ok": false, "cancelled": true }))),
        }
    } else {
        PathBuf::from(req.zip.trim())
    };
    if !zip.is_file() {
        return Err(err_json(format!("找不到导入文件: {}", zip.display())));
    }
    let replace = req.target.trim().is_empty();
    let target = if replace {
        sibling_dir(&crate::config::data_root(), "import-staging")
    } else {
        PathBuf::from(req.target.trim())
    };
    if !replace && target.exists() && count_entries(&target) > 0 {
        return Err(err_json(format!(
            "导入目标不是空目录：{}（不传目标时用替换模式，不需要目标目录）",
            target.display()
        )));
    }
    // 总量（进度条分母）：读 zip 中央目录，失败就 0（前端退化成不确定进度）。
    let target_for_stat = target.clone();
    let (total_bytes, total_files) = {
        let z = zip.clone();
        tokio::task::spawn_blocking(move || zip_stat(&z)).await.unwrap_or((0, 0))
    };
    {
        let mut g = IMPORT_PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        *g = Some(ImportProgress {
            target: target_for_stat.display().to_string(),
            total_bytes,
            total_files,
            done: false,
            error: None,
        });
    }
    let res = tokio::task::spawn_blocking({
        let target = target.clone();
        let zip = zip.clone();
        move || -> Result<(), String> {
            let _guard = DATA_OP.lock().unwrap_or_else(|e| e.into_inner());
            unzip_to(&zip, &target)?;
            if !replace {
                write_pointer(&target)?;
            }
            Ok(())
        }
    })
    .await;
    let outcome = match res {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e),
        Err(e) => Err(format!("导入任务失败: {e}")),
    };
    {
        let mut g = IMPORT_PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(p) = g.as_mut() {
            p.done = true;
            if let Err(e) = &outcome {
                p.error = Some(e.clone());
            }
        }
    }
    if let Err(e) = outcome {
        if replace {
            let _ = std::fs::remove_dir_all(&target);
        }
        return Err(err_json(e));
    }
    if replace {
        let data = crate::config::data_root();
        let backup = sibling_dir(&data, "old");
        let restarting = spawn_swap_helper(&target, &data, &backup).is_ok();
        if !restarting {
            let _ = std::fs::remove_dir_all(&target);
        }
        observability::event(
            EventType::State,
            json!({ "kind": "data.imported.replace", "zip": zip.display().to_string(), "backup": backup.display().to_string(), "restarting": restarting }),
        );
        return Ok(Json(json!({
            "ok": true,
            "replaced": true,
            "target": data.display().to_string(),
            "backup": backup.display().to_string(),
            "restart_required": true,
            "restarting": restarting,
        })));
    }
    observability::event(
        EventType::State,
        json!({ "kind": "data.imported", "zip": zip.display().to_string(), "target": target.display().to_string() }),
    );
    // 兼容模式：真正重启后端让新数据目录生效。
    let restarting = std::env::current_exe()
        .ok()
        .map(|exe| spawn_service_restart(&exe, &exe, &target).is_ok())
        .unwrap_or(false);
    Ok(Json(json!({
        "ok": true,
        "target": target.display().to_string(),
        "restart_required": true,
        "restarting": restarting,
    })))
}

/// GET /api/data/import/progress —— 导入进度（前端 400ms 轮询）。
pub(super) async fn data_import_progress() -> Json<Value> {
    let snap = {
        let g = IMPORT_PROGRESS.lock().unwrap_or_else(|e| e.into_inner());
        g.clone()
    };
    match snap {
        None => Json(json!({ "running": false })),
        Some(p) => {
            let (bytes, files) = if p.done { (0, 0) } else { dir_stat(Path::new(&p.target)) };
            Json(json!({
                "running": !p.done,
                "done": p.done,
                "target": p.target,
                "total_bytes": p.total_bytes,
                "total_files": p.total_files,
                "bytes": bytes,
                "files": files,
                "error": p.error,
            }))
        }
    }
}

/// POST /api/data/open —— 打开某个目录。
///
/// 优先让**壳**（前台进程）来拉起资源管理器：后台服务进程起的窗口容易落在应用后面；
/// 壳没连（纯浏览器/服务模式）才自己起 explorer，并尽力把窗口顶到前台。
pub(super) async fn data_open(Json(req): Json<OpenReq>) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let path = match req.which.as_str() {
        "install" => crate::config::app_root(),
        "cache" => crate::config::cache_root(),
        _ => crate::config::data_root(),
    };
    std::fs::create_dir_all(&path).map_err(|e| err_json(format!("目录不存在: {e}")))?;
    if crate::browser::shell::open_path(&path) {
        return Ok(Json(json!({ "ok": true, "path": path.display().to_string(), "via": "shell" })));
    }
    std::process::Command::new("explorer.exe")
        .arg(&path)
        .spawn()
        .map_err(|e| err_json(format!("打开目录失败: {e}")))?;
    raise_explorer_window(&path);
    Ok(Json(json!({ "ok": true, "path": path.display().to_string() })))
}

// —— 界面微调规则导出（2026-11-12）——
// 原来走浏览器下载：WebView2 的下载目录由系统定，前端拿不到路径，用户问「导出到哪了」。
// 改成服务端写 <数据目录>/exports/<名>.json，真实路径回给界面显示；同名自动加序号。
#[derive(Debug, Deserialize)]
pub(super) struct MicroExportReq {
    pub json: String,
    #[serde(default)]
    pub filename: String,
}

pub(super) async fn micro_export(
    Json(req): Json<MicroExportReq>,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let dir = crate::config::data_root().join("exports");
    std::fs::create_dir_all(&dir).map_err(|e| err_json(format!("创建导出目录失败: {e}")))?;
    // 只取文件名部分，防路径穿越
    let raw = req.filename.trim();
    let base = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();
    let stem = if base.is_empty() {
        format!("ui-css-rules-{}", chrono::Local::now().format("%Y%m%d-%H%M%S"))
    } else {
        base.trim_end_matches(".json").to_string()
    };
    let mut path = dir.join(format!("{stem}.json"));
    let mut i = 1u32;
    while path.exists() && i <= 999 {
        path = dir.join(format!("{stem}-{i}.json"));
        i += 1;
    }
    std::fs::write(&path, req.json.as_bytes())
        .map_err(|e| err_json(format!("写入导出文件失败: {e}")))?;
    Ok(Json(json!({ "ok": true, "path": path.display().to_string() })))
}

fn err_json(msg: impl Into<String>) -> (StatusCode, Json<Value>) {
    (StatusCode::BAD_REQUEST, Json(json!({ "error": msg.into() })))
}

// —— 自定义背景媒体（视频也能当壁纸）——
// 文件落 <数据目录>\cache\wallpaper.<ext>，localStorage 只存 URL：视频几十上百 MB，
// localStorage 装不下。cache 可再生，清缓存的代价只是重选一次背景；导出/迁移默认跳过
// cache，几百 MB 的视频不会跟着备份走。
const WALLPAPER_EXTS: [&str; 7] = ["mp4", "webm", "mov", "png", "jpg", "gif", "webp"];
const WALLPAPER_MAX: usize = 300 * 1024 * 1024;

fn wallpaper_mime(ext: &str) -> &'static str {
    match ext {
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        "png" => "image/png",
        "jpg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
}

fn wallpaper_ext_of(mime: &str) -> Option<&'static str> {
    Some(match mime {
        "video/mp4" | "video/mpeg4" | "application/mp4" => "mp4",
        "video/webm" => "webm",
        "video/quicktime" => "mov",
        "image/png" => "png",
        "image/jpeg" | "image/jpg" => "jpg",
        "image/gif" => "gif",
        "image/webp" => "webp",
        _ => return None,
    })
}

fn wallpaper_clear_files(dir: &Path) {
    for ext in WALLPAPER_EXTS {
        let _ = std::fs::remove_file(dir.join(format!("wallpaper.{ext}")));
    }
}

/// 背景媒体落数据目录根（不走 cache）：导出/迁移/换机都带上。
fn wallpaper_dir() -> PathBuf {
    crate::config::data_root()
}

/// 旧版本把壁纸放在 cache（导出默认不含 cache，备份会丢）：启动时挪出来。
fn migrate_wallpaper_from_cache() {
    let data = wallpaper_dir();
    let legacy = data.join("cache");
    for ext in WALLPAPER_EXTS {
        let from = legacy.join(format!("wallpaper.{ext}"));
        let to = data.join(format!("wallpaper.{ext}"));
        if from.is_file() && !to.exists() {
            let _ = std::fs::rename(&from, &to).or_else(|_| {
                std::fs::copy(&from, &to).map(|_| {
                    let _ = std::fs::remove_file(&from);
                })
            });
        }
    }
}

/// POST /api/wallpaper —— 原始字节上传，Content-Type 定扩展名，上限 300MB。
pub(super) async fn wallpaper_upload(
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let mime = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let Some(ext) = wallpaper_ext_of(mime) else {
        return Err(err_json("只支持图片（png/jpg/gif/webp）或视频（mp4/webm/mov）"));
    };
    if body.is_empty() {
        return Err(err_json("收到的是空文件"));
    }
    if body.len() > WALLPAPER_MAX {
        return Err(err_json("背景媒体太大（上限 300MB），剪短一点或换一张"));
    }
    let dir = wallpaper_dir();
    std::fs::create_dir_all(&dir).map_err(|e| err_json(format!("建数据目录失败: {e}")))?;
    wallpaper_clear_files(&dir); // 扩展名可能变化，旧的先清掉
    wallpaper_clear_files(&dir.join("cache")); // 顺手清旧位置，别让旧图盖住新图
    std::fs::write(dir.join(format!("wallpaper.{ext}")), &body)
        .map_err(|e| err_json(format!("写背景文件失败: {e}")))?;
    let kind = if mime.starts_with("video/") { "video" } else { "image" };
    observability::event(
        EventType::State,
        json!({ "kind": "wallpaper.set", "media": kind, "bytes": body.len() }),
    );
    Ok(Json(json!({ "ok": true, "kind": kind, "url": "/api/wallpaper/file" })))
}

/// GET /api/wallpaper/file —— 背景媒体本体（按扩展名给 MIME）。
pub(super) async fn wallpaper_file() -> impl IntoResponse {
    for dir in [wallpaper_dir(), wallpaper_dir().join("cache")] {
        for ext in WALLPAPER_EXTS {
            let p = dir.join(format!("wallpaper.{ext}"));
            if let Ok(bytes) = std::fs::read(&p) {
                return ([(CONTENT_TYPE, wallpaper_mime(ext).to_string())], bytes).into_response();
            }
        }
    }
    (StatusCode::NOT_FOUND, "no wallpaper").into_response()
}

/// DELETE /api/wallpaper —— 清掉背景文件（前端清壁纸时调用；文件不存在也算成功）。
pub(super) async fn wallpaper_delete() -> Json<Value> {
    let dir = wallpaper_dir();
    wallpaper_clear_files(&dir);
    wallpaper_clear_files(&dir.join("cache"));
    Json(json!({ "ok": true }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "pawch-data-admin-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn count_entries_ignores_os_junk() {
        let d = tmp("count-junk");
        std::fs::write(d.join("desktop.ini"), "x").unwrap();
        std::fs::write(d.join("Thumbs.db"), "x").unwrap();
        assert_eq!(count_entries(&d), 0, "系统隐藏文件不算内容");
        std::fs::write(d.join("real.txt"), "x").unwrap();
        assert_eq!(count_entries(&d), 1);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn copy_tree_copies_and_skips_cache() {
        let src = tmp("src");
        let dst = tmp("dst-out");
        std::fs::write(src.join("a.txt"), "abc").unwrap();
        std::fs::create_dir_all(src.join("sub")).unwrap();
        std::fs::write(src.join("sub").join("c.txt"), "de").unwrap();
        std::fs::create_dir_all(src.join("cache")).unwrap();
        std::fs::write(src.join("cache").join("big.bin"), vec![0u8; 1024]).unwrap();

        let mut st = CopyStats::default();
        copy_tree(&src, &dst, &["cache"], &mut st).unwrap();
        assert_eq!(st.files, 2, "只应复制 2 个普通文件");
        assert_eq!(st.bytes, 5);
        assert!(st.skipped.contains(&"cache".to_string()));
        assert_eq!(std::fs::read_to_string(dst.join("a.txt")).unwrap(), "abc");
        assert!(dst.join("sub").join("c.txt").is_file());
        assert!(!dst.join("cache").exists(), "cache 应被跳过");

        let _ = std::fs::remove_dir_all(&src);
        let _ = std::fs::remove_dir_all(&dst);
    }

    #[test]
    fn ps_quote_doubles_single_quotes() {
        assert_eq!(ps_quote(Path::new("C:/a'b")), "'C:/a''b'");
    }

    #[test]
    fn sibling_dir_sits_next_to_current() {
        let cur = Path::new("C:\\data\\.pawch-agent");
        let p = sibling_dir(cur, "old");
        assert_eq!(p.parent().unwrap(), Path::new("C:\\data"));
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        assert!(name.starts_with(".pawch-agent-old-"), "意外目录名: {name}");
    }

    #[test]
    fn move_target_inside_current_is_rejected() {
        let root = tmp("within");
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let sibling = tmp("within-sibling");
        assert!(is_same_or_within(&root, &root));
        assert!(is_same_or_within(&sub, &root));
        assert!(is_same_or_within(&root.join("not-yet"), &root), "不存在的子路径也要判出来");
        assert!(!is_same_or_within(&sibling, &root));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&sibling);
    }
}
