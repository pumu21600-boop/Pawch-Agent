//! Pawch Agent 安装器。
//!
//! 安装向导：一个窗口里两行地址（安装目录 / 数据目录）→ 解压安装 → 建快捷方式 → 启动。
//! 目录由用户自己填/建，向导不弹文件夹选择框、也不预建目录。
//! 绿色版兜底：不装安装器直接解压运行，桌面壳首次启动仍会引导选数据目录。
//!
//! 安装内容来自安装器旁边的 `payload.zip`（scripts/build-installer.ps1 生成）；
//! 静默/自动化：`--install-dir <p> --data-dir <p> [--payload <zip>] [--silent] [--no-shortcuts] [--no-launch]`。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Default)]
struct Args {
    install_dir: Option<String>,
    data_dir: Option<String>,
    payload: Option<String>,
    silent: bool,
    no_shortcuts: bool,
    no_launch: bool,
    /// 卸载模式（安装目录里的 Uninstall.exe 用）
    uninstall: bool,
    /// 内部：延迟删除（Uninstall.exe 先拷到 %TEMP% 再删安装目录，避免文件占用）
    purge: bool,
    /// 卸载时同时删除数据目录
    delete_data: bool,
}

fn parse_args() -> Args {
    parse_args_from(std::env::args().skip(1))
}

fn parse_args_from<I: Iterator<Item = String>>(mut it: I) -> Args {
    let mut a = Args::default();
    while let Some(k) = it.next() {
        match k.as_str() {
            "--install-dir" => a.install_dir = it.next(),
            "--data-dir" => a.data_dir = it.next(),
            "--payload" => a.payload = it.next(),
            "--silent" => a.silent = true,
            "--no-shortcuts" => a.no_shortcuts = true,
            "--no-launch" => a.no_launch = true,
            "--uninstall" => a.uninstall = true,
            "--purge" => a.purge = true,
            "--delete-data" => a.delete_data = true,
            _ => {}
        }
    }
    a
}

fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

fn local_appdata() -> PathBuf {
    std::env::var_os("LOCALAPPDATA")
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(exe_dir)
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(exe_dir)
}

fn msg(title: &str, text: impl Into<String>) {
    rfd::MessageDialog::new()
        .set_title(title)
        .set_description(text.into())
        .set_buttons(rfd::MessageButtons::Ok)
        .show();
}

fn ask(title: &str, text: impl Into<String>) -> bool {
    matches!(
        rfd::MessageDialog::new()
            .set_title(title)
            .set_description(text.into())
            .set_buttons(rfd::MessageButtons::YesNo)
            .show(),
        rfd::MessageDialogResult::Yes
    )
}

/// 文件名是 Uninstall.exe（不分大小写）→ 双击（无参数）默认进卸载模式，
/// 否则会掉进安装模式去找 payload（2026-11-03 用户实测踩坑）。
fn looks_like_uninstaller(exe: &Path) -> bool {
    exe.file_name()
        .map(|n| n.to_string_lossy().eq_ignore_ascii_case("uninstall.exe"))
        .unwrap_or(false)
}

/// 内嵌 payload 的识别尾部：`zip 字节 + u64 小端 zip 长度 + "PAWCH1"`。
/// 构建脚本（build-installer.ps1）在 cargo build 之后把 payload.zip 附加到 exe 尾部。
const EMBED_MAGIC: &[u8; 6] = b"PAWCH1";
const EMBED_TRAILER: u64 = 8 + 6; // 长度字段 + 魔数

/// 读自身文件尾部的内嵌 payload；没有（或尾部损坏）返回 None。
/// 单文件分发用：Setup.exe 自带安装内容，不再依赖旁边的 payload.zip。
fn embedded_payload() -> Option<Vec<u8>> {
    let me = std::env::current_exe().ok()?;
    let data = std::fs::read(&me).ok()?;
    let magic_len = EMBED_MAGIC.len();
    if data.len() < (EMBED_TRAILER as usize) + magic_len {
        return None;
    }
    let tail = data.len() - magic_len;
    if &data[tail..] != EMBED_MAGIC {
        return None;
    }
    // 长度字段距文件尾 = 魔数(6) + 长度(8) = EMBED_TRAILER,不再额外减魔数
    let len_off = data.len() - (EMBED_TRAILER as usize);
    let mut len_bytes = [0u8; 8];
    len_bytes.copy_from_slice(&data[len_off..len_off + 8]);
    let zip_len = u64::from_le_bytes(len_bytes) as usize;
    let zip_off = len_off.checked_sub(zip_len)?;
    // 长度必须恰好落在文件头之后（zip 至少要有 EOCD 的 22 字节），防尾部损坏
    if zip_len < 22 || zip_off == 0 {
        return None;
    }
    // Zip 校验：PK\x03\x04 本地文件头魔数
    if &data[zip_off..zip_off + 4] != b"PK\x03\x04" {
        return None;
    }
    Some(data[zip_off..len_off].to_vec())
}

/// 把内嵌 payload 写到临时目录供 Expand-Archive 解压（PS 不认内存流）。
fn stage_embedded_payload(bytes: &[u8]) -> Option<PathBuf> {
    let dir = std::env::temp_dir().join(format!("pawch-payload-{}", std::process::id()));
    std::fs::create_dir_all(&dir).ok()?;
    let path = dir.join("payload.zip");
    std::fs::write(&path, bytes).ok()?;
    Some(path)
}

fn wizard(payload: &Path) -> Option<(PathBuf, PathBuf)> {
    if !payload.is_file() {
        msg(
            "缺少安装文件",
            format!(
                "安装器内没有内嵌安装内容，旁边也没有 payload.zip：{}\n\n请重新下载完整的安装包。",
                payload.display()
            ),
        );
        return None;
    }
    // 不再弹「选文件夹」对话框，改成一个窗口里两行地址直接填/粘贴。
    // 向导不创建任何目录（用户自己建）；只有用户点「开始安装」的路径才会被使用。
    let install_default = local_appdata().join("Programs").join("Pawch Agent");
    let data_default = home().join(".pawch-agent");
    ask_dirs_form(&install_default, &data_default)
}

/// 目录输入窗口的 PowerShell 脚本（WinForms，一行一个地址，可粘贴；不弹文件夹选择框）。
///
/// 参数：OutFile（结果写到哪）、DefaultInstall、DefaultData。点「开始安装」→ 写结果并 exit 0；
/// 取消/关窗 → exit 1。脚本以 UTF-8 BOM 落地，避免 PS 5.1 读中文乱码。
const DIRS_PS1: &str = r##"
param([string]$OutFile, [string]$DefaultInstall, [string]$DefaultData)
Add-Type -AssemblyName System.Windows.Forms
Add-Type -AssemblyName System.Drawing
[System.Windows.Forms.Application]::EnableVisualStyles()

$form = New-Object System.Windows.Forms.Form
$form.Text = 'Pawch Agent 安装'
$form.StartPosition = 'CenterScreen'
$form.FormBorderStyle = 'FixedDialog'
$form.MaximizeBox = $false
$form.MinimizeBox = $false
$form.ShowIcon = $false
$form.TopMost = $true
$form.AutoScaleMode = 'Dpi'
$form.AutoScaleDimensions = New-Object System.Drawing.SizeF(96, 96)
$form.ClientSize = New-Object System.Drawing.Size(700, 220)

$l1 = New-Object System.Windows.Forms.Label
$l1.Text = '安装目录'
$l1.Location = New-Object System.Drawing.Point(16, 14)
$l1.AutoSize = $true

$t1 = New-Object System.Windows.Forms.TextBox
$t1.Location = New-Object System.Drawing.Point(16, 38)
$t1.Size = New-Object System.Drawing.Size(668, 26)
$t1.Anchor = 'Top,Left,Right'
$t1.Text = $DefaultInstall

$l2 = New-Object System.Windows.Forms.Label
$l2.Text = '数据目录'
$l2.Location = New-Object System.Drawing.Point(16, 80)
$l2.AutoSize = $true

$t2 = New-Object System.Windows.Forms.TextBox
$t2.Location = New-Object System.Drawing.Point(16, 104)
$t2.Size = New-Object System.Drawing.Size(668, 26)
$t2.Anchor = 'Top,Left,Right'
$t2.Text = $DefaultData

$ok = New-Object System.Windows.Forms.Button
$ok.Text = '开始安装'
$ok.Size = New-Object System.Drawing.Size(110, 32)
$ok.Location = New-Object System.Drawing.Point(458, 170)
$ok.Anchor = 'Bottom,Right'

$cancel = New-Object System.Windows.Forms.Button
$cancel.Text = '取消'
$cancel.Size = New-Object System.Drawing.Size(110, 32)
$cancel.Location = New-Object System.Drawing.Point(574, 170)
$cancel.Anchor = 'Bottom,Right'

[void]$form.Controls.Add($l1)
[void]$form.Controls.Add($t1)
[void]$form.Controls.Add($l2)
[void]$form.Controls.Add($t2)
[void]$form.Controls.Add($ok)
[void]$form.Controls.Add($cancel)
$form.AcceptButton = $ok
$form.CancelButton = $cancel

$script:res1 = ''
$script:res2 = ''

$ok.Add_Click({
    # 粘贴里常带首尾引号（资源管理器「复制为路径」），一并去掉
    $p1 = $t1.Text.Trim().Trim('"').Trim()
    $p2 = $t2.Text.Trim().Trim('"').Trim()
    if (-not $p1 -or -not $p2) {
        [void][System.Windows.Forms.MessageBox]::Show('两个目录都要填，不能为空。', 'Pawch Agent')
        return
    }
    if (-not [System.IO.Path]::IsPathRooted($p1) -or -not [System.IO.Path]::IsPathRooted($p2)) {
        [void][System.Windows.Forms.MessageBox]::Show('请填完整路径，例如：D:\Pawch\PawchAgent', 'Pawch Agent')
        return
    }
    $missing = @()
    foreach ($p in @($p1, $p2)) {
        if (-not [System.IO.Directory]::Exists($p)) { $missing += $p }
    }
    if ($missing.Count -gt 0) {
        $text = "这些目录还不存在：`r`n" + ($missing -join "`r`n") + "`r`n`r`n点「是」让安装器创建；点「否」回去自己建。"
        $ans = [System.Windows.Forms.MessageBox]::Show($text, 'Pawch Agent', 'YesNo', 'Question')
        if ($ans -ne 'Yes') { return }
    }
    $script:res1 = $p1
    $script:res2 = $p2
    $form.DialogResult = 'OK'
    $form.Close()
})

$cancel.Add_Click({
    $form.DialogResult = 'Cancel'
    $form.Close()
})

$r = $form.ShowDialog()
if ($r -eq 'OK' -and $script:res1 -and $script:res2) {
    # 必须写不带 BOM 的 UTF-8：带 BOM 时第一行开头是 U+FEFF，Rust 侧会被当成
    # 路径首字符 → create_dir_all 报 os error 123（2026-11-03 用户实测踩到）。
    $enc = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($OutFile, $script:res1 + "`n" + $script:res2, $enc)
    exit 0
}
exit 1
"##;

/// 弹目录输入窗口，返回 (安装目录, 数据目录)；取消/失败返回 None。
/// 目录是否创建由用户在窗口里明示（不存在时问「要不要创建」），向导自己不建。
fn ask_dirs_form(install_default: &Path, data_default: &Path) -> Option<(PathBuf, PathBuf)> {
    let stamp = std::process::id();
    let script = std::env::temp_dir().join(format!("pawch-setup-dirs-{stamp}.ps1"));
    let out = std::env::temp_dir().join(format!("pawch-setup-dirs-{stamp}.txt"));
    let _ = std::fs::remove_file(&out);
    let mut bytes = vec![0xEF, 0xBB, 0xBF]; // UTF-8 BOM：PS 5.1 读中文不乱码
    bytes.extend_from_slice(DIRS_PS1.as_bytes());
    if let Err(e) = std::fs::write(&script, &bytes) {
        msg("安装向导启动失败", format!("写临时脚本失败：{e}"));
        return None;
    }
    let status = {
        use std::os::windows::process::CommandExt;
        Command::new("powershell.exe")
            .args(["-NoProfile", "-STA", "-ExecutionPolicy", "Bypass", "-File"])
            .arg(&script)
            .arg(&out)
            .arg(install_default)
            .arg(data_default)
            .creation_flags(0x0800_0000)
            .status()
    };
    let _ = std::fs::remove_file(&script);
    let status = match status {
        Ok(s) => s,
        Err(e) => {
            let _ = std::fs::remove_file(&out);
            msg("安装向导启动失败", format!("无法启动 PowerShell：{e}"));
            return None;
        }
    };
    let text = std::fs::read_to_string(&out).ok();
    let _ = std::fs::remove_file(&out);
    if !status.success() {
        return None; // 用户取消/关窗
    }
    let text = match text {
        Some(t) => t,
        None => {
            msg("安装向导没有返回目录", "目录输入窗口没有返回结果，请重试。");
            return None;
        }
    };
    parse_dirs_output(&text)
}

/// 解析目录窗口的输出（第一行安装目录、第二行数据目录）。
///
/// 必须容错两件事：① UTF-8 BOM（老版本 PS 写文件带 BOM，第一行会以 U+FEFF 开头，
/// 当成路径就是 os error 123）；② 粘贴带首尾引号（资源管理器「复制为路径」）。
fn parse_dirs_output(text: &str) -> Option<(PathBuf, PathBuf)> {
    let text = text.trim_start_matches('\u{feff}');
    let mut lines = text
        .lines()
        .map(|s| s.trim().trim_matches('"').trim())
        .filter(|s| !s.is_empty());
    let install = lines.next()?.to_string();
    let data = lines.next()?.to_string();
    Some((PathBuf::from(install), PathBuf::from(data)))
}

fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "''"))
}

fn ps_str(s: &str) -> String {
    format!("'{}'", s.replace('\'', "''"))
}

fn run_ps(script: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("启动 powershell 失败: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).chars().take(600).collect::<String>())
    }
}

/// 当前数据目录：指针 > 默认（卸载时用来提示保留/删除）。
fn current_data_dir() -> PathBuf {
    let ptr = local_appdata().join("Pawch Agent").join(".data-dir");
    if let Ok(t) = std::fs::read_to_string(&ptr) {
        let p = t.trim();
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    home().join(".pawch-agent")
}

fn stop_app(install_dir: &Path) {
    use std::os::windows::process::CommandExt;
    let stop = install_dir.join("pawch-agent.exe");
    if stop.is_file() {
        let _ = Command::new(&stop).arg("stop").creation_flags(0x0800_0000).status();
    }
    for img in ["Pawch Agent.exe", "pawch-agent.exe"] {
        let _ = Command::new("taskkill")
            .args(["/IM", img, "/F"])
            .creation_flags(0x0800_0000)
            .status();
    }
    std::thread::sleep(std::time::Duration::from_millis(800));
}

fn remove_shortcuts() {
    let _ = run_ps(
        "$ws = New-Object -ComObject WScript.Shell; \
         $dirs = @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs')); \
         foreach ($d in $dirs) { if ($d) { Remove-Item -LiteralPath (Join-Path $d 'Pawch Agent.lnk') -Force -ErrorAction SilentlyContinue } }",
    );
}

fn remove_registry() {
    let _ = run_ps(
        "Remove-Item -LiteralPath 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PawchAgent' -Recurse -Force -ErrorAction SilentlyContinue",
    );
}

/// 卸载入口：停进程 → 删快捷方式/卸载项 → 自己拷到 %TEMP% 后延迟删安装目录。
fn uninstall(args: &Args) {
    let install_dir = args
        .install_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(exe_dir);
    if !install_dir.join("pawch-agent.exe").is_file() {
        if !args.silent {
            msg(
                "找不到安装目录",
                format!("这个目录里没有 pawch-agent.exe：{}", install_dir.display()),
            );
        }
        return;
    }
    let data_dir = current_data_dir();
    let mut delete_data = args.delete_data;
    if !args.silent {
        if !ask(
            "卸载 Pawch Agent",
            format!(
                "将删除程序文件、快捷方式和卸载项。\n数据目录默认保留：{}\n\n继续卸载？",
                data_dir.display()
            ),
        ) {
            return;
        }
        if ask(
            "是否删除数据？",
            format!(
                "是否同时删除数据目录？\n{}\n\n对话、记忆、设置会一起删除且不可恢复。\n选「否」= 只卸载程序，数据保留。",
                data_dir.display()
            ),
        ) {
            delete_data = ask("最后确认", "确定要删除数据目录吗？此操作不可恢复。");
        } else {
            delete_data = false;
        }
    }

    stop_app(&install_dir);
    remove_shortcuts();
    remove_registry();

    // Uninstall.exe 自身被占用，先拷到 %TEMP%，由它延迟删安装目录
    let tmp = std::env::temp_dir().join(format!("pawch-uninstall-{}.exe", std::process::id()));
    let me = std::env::current_exe().unwrap_or_else(|_| install_dir.join("Uninstall.exe"));
    if std::fs::copy(&me, &tmp).is_err() {
        if !args.silent {
            msg("卸载失败", "无法准备卸载程序（复制到临时目录失败）");
        }
        return;
    }
    let mut cmd = Command::new(&tmp);
    cmd.arg("--purge").arg("--silent");
    cmd.arg("--install-dir").arg(&install_dir);
    // 别让 purge 继承安装目录当 cwd（Windows 删不掉进程当前目录，会留空壳）
    cmd.current_dir(std::env::temp_dir());
    if delete_data {
        cmd.arg("--delete-data");
    }
    match cmd.spawn() {
        Ok(_) => std::process::exit(0), // 父进程退出，purge 才能删安装目录
        Err(e) => {
            if !args.silent {
                msg("卸载失败", format!("启动卸载任务失败: {e}"));
            }
        }
    }
}

/// 删目录，被占用（资源管理器开着目录/杀软在扫）时重试几轮再放弃。
fn remove_dir_all_retry(path: &Path, tries: u32, delay: std::time::Duration) -> Result<(), String> {
    let mut last = String::new();
    for _ in 0..tries {
        if !path.exists() {
            return Ok(());
        }
        match std::fs::remove_dir_all(path) {
            Ok(()) => return Ok(()),
            Err(e) => last = format!("{e}"),
        }
        std::thread::sleep(delay);
    }
    if path.exists() {
        Err(last)
    } else {
        Ok(())
    }
}

/// 延迟删除阶段（从 %TEMP% 运行，安装目录里的文件已无占用）。
fn purge(args: &Args) {
    std::thread::sleep(std::time::Duration::from_millis(1500));
    // 关键：purge 进程从 Uninstall.exe 派生，会继承"安装目录"作为工作目录；
    // Windows 不允许删除进程的当前目录（os error 32），会把目录删成空壳。
    // 先把 cwd 挪到 %TEMP%，再删安装目录（2026-11-04 用户实测复现）。
    let _ = std::env::set_current_dir(std::env::temp_dir());
    let install_dir = args
        .install_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(exe_dir);
    let mut err: Option<String> = None;
    if install_dir.is_dir() {
        if let Err(e) = remove_dir_all_retry(&install_dir, 6, std::time::Duration::from_secs(2)) {
            err = Some(format!(
                "删除安装目录失败（{}）：{e}\n多半是资源管理器开着这个目录或杀毒软件在扫描，关掉后手动删一下即可。",
                install_dir.display()
            ));
        }
    }
    if args.delete_data {
        let data = current_data_dir();
        if data.is_dir() {
            if let Err(e) = remove_dir_all_retry(&data, 6, std::time::Duration::from_secs(2)) {
                err = Some(format!("删除数据目录失败（{}）：{e}", data.display()));
            }
        }
        // 指针文件删掉后，把空的指针目录也带走（remove_dir 只删空目录，不会误删）
        let ptr_dir = local_appdata().join("Pawch Agent");
        let _ = std::fs::remove_file(ptr_dir.join(".data-dir"));
        let _ = std::fs::remove_dir(&ptr_dir);
    }
    // %TEMP% 里本应用自己的残留（壳的 lock/log、卸载失败日志、payload 暂存目录）。
    // 服务启动时本来会 sweep 一遍，但卸载后不再启动，这里收尾一次（2026-11-04 实测：
    // 卸载后总剩 shell.lock/shell.log 两个文件）。只认 pawch 前缀的已知名字，
    // 不碰 selfdel 脚本/自身拷贝（正在用）。
    for name in [
        "pawch-agent-shell.lock",
        "pawch-agent-shell.log",
        "pawch-uninstall-failed.log",
    ] {
        let _ = std::fs::remove_file(std::env::temp_dir().join(name));
    }
    if let Ok(rd) = std::fs::read_dir(std::env::temp_dir()) {
        for e in rd.flatten() {
            let name = e.file_name();
            if name.to_string_lossy().starts_with("pawch-payload-") {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    // 自删：本次进程跑的就是 %TEMP% 里的拷贝，退出后由 PowerShell 延迟删掉。
    // 不走 cmd：Rust 会把带引号的参数整体转义成 \"，cmd /C 直接不认（2026-11-03 实测）；
    // 裸 timeout 又会被 PATH 里的 GNU timeout 抢答。临时 .ps1 + ps_quote 转义零歧义。
    if let Ok(me) = std::env::current_exe() {
        if me.starts_with(std::env::temp_dir()) {
            use std::os::windows::process::CommandExt;
            let script = std::env::temp_dir().join(format!(
                "pawch-uninstall-selfdel-{}.ps1",
                std::process::id()
            ));
            let body = format!(
                "Start-Sleep -Seconds 2\r\nRemove-Item -LiteralPath {} -Force\r\nRemove-Item -LiteralPath {} -Force\r\n",
                ps_quote(&me),
                ps_quote(&script),
            );
            let mut bytes = vec![0xEF, 0xBB, 0xBF]; // BOM：PS 5.1 读中文路径要 UTF-8 BOM
            bytes.extend_from_slice(body.as_bytes());
            if std::fs::write(&script, bytes).is_ok() {
                let _ = Command::new("powershell")
                    .args([
                        "-NoProfile",
                        "-NoLogo",
                        "-NonInteractive",
                        "-ExecutionPolicy",
                        "Bypass",
                        "-WindowStyle",
                        "Hidden",
                        "-File",
                    ])
                    .arg(&script)
                    .creation_flags(0x0800_0000) // CREATE_NO_WINDOW，别闪黑框
                    .spawn();
            }
        }
    }
    if !args.silent {
        match err {
            None => msg("卸载完成", "Pawch Agent 已卸载。"),
            Some(e) => msg("卸载未完成", e),
        }
    } else if let Some(e) = err {
        // 静默模式没人看弹窗：失败留个尸检日志
        let _ = std::fs::write(std::env::temp_dir().join("pawch-uninstall-failed.log"), e);
    }
}

fn install(payload: &Path, install_dir: &Path, data_dir: &Path, shortcuts: bool) -> Result<(), String> {
    if let Some(parent) = install_dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            format!("建安装目录的上级失败（{}）：{e}", parent.display())
        })?;
    }
    std::fs::create_dir_all(install_dir).map_err(|e| {
        format!("建安装目录失败（{}）：{e}", install_dir.display())
    })?;
    // 先把目录可写性试出来，别解压到一半才发现 Program Files 需要管理员
    let probe = install_dir.join(".pawch-write-test");
    std::fs::write(&probe, b"x").map_err(|e| {
        format!(
            "安装目录不可写（{}）：{e}\n请换一个普通用户可写的目录（如 %LOCALAPPDATA%\\Programs\\Pawch Agent）。",
            install_dir.display()
        )
    })?;
    let _ = std::fs::remove_file(&probe);

    run_ps(&format!(
        "Expand-Archive -LiteralPath {} -DestinationPath {} -Force",
        ps_quote(payload),
        ps_quote(install_dir)
    ))?;
    if !install_dir.join("pawch-agent.exe").is_file() {
        return Err("解压后没有找到 pawch-agent.exe，安装包可能不完整".into());
    }

    std::fs::create_dir_all(data_dir).map_err(|e| {
        format!("建数据目录失败（{}）：{e}", data_dir.display())
    })?;
    let ptr_dir = local_appdata().join("Pawch Agent");
    std::fs::create_dir_all(&ptr_dir).map_err(|e| format!("建指针目录失败（{}）：{e}", ptr_dir.display()))?;
    std::fs::write(ptr_dir.join(".data-dir"), data_dir.display().to_string())
        .map_err(|e| format!("写数据目录指针失败: {e}"))?;

    if shortcuts {
        let target = if install_dir.join("Pawch Agent.exe").is_file() {
            install_dir.join("Pawch Agent.exe")
        } else {
            install_dir.join("pawch-agent.exe")
        };
        let script = format!(
            "$ws = New-Object -ComObject WScript.Shell; \
             $dirs = @([Environment]::GetFolderPath('Desktop'), [Environment]::GetFolderPath('Programs')); \
             foreach ($d in $dirs) {{ if ($d) {{ $s = $ws.CreateShortcut((Join-Path $d 'Pawch Agent.lnk')); \
               $s.TargetPath = {}; $s.WorkingDirectory = {}; $s.IconLocation = {}+',0'; $s.Save() }} }}",
            ps_quote(&target),
            ps_quote(install_dir),
            ps_quote(&target)
        );
        // 快捷方式失败不阻断安装（权限/安全软件拦 WScript 时仍可用 start.bat）
        let _ = run_ps(&script);
    }

    // 卸载器 = 安装器自己的一份拷贝（带 --uninstall 模式，不依赖额外二进制）
    if let Ok(me) = std::env::current_exe() {
        let _ = std::fs::copy(me, install_dir.join("Uninstall.exe"));
    }

    // 控制面板「程序和功能」卸载项
    let version = std::fs::read_to_string(install_dir.join("version.txt"))
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| "0.0.0".into());
    let icon_target = if install_dir.join("Pawch Agent.exe").is_file() {
        install_dir.join("Pawch Agent.exe")
    } else {
        install_dir.join("pawch-agent.exe")
    };
    let uninstaller = install_dir.join("Uninstall.exe");
    let reg = format!(
        "$k='HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\PawchAgent'; \
         New-Item -Path $k -Force | Out-Null; \
         Set-ItemProperty -Path $k -Name DisplayName -Value 'Pawch Agent'; \
         Set-ItemProperty -Path $k -Name DisplayVersion -Value {}; \
         Set-ItemProperty -Path $k -Name Publisher -Value 'Pawch Agent'; \
         Set-ItemProperty -Path $k -Name DisplayIcon -Value {}; \
         Set-ItemProperty -Path $k -Name InstallLocation -Value {}; \
         Set-ItemProperty -Path $k -Name UninstallString -Value {}; \
         Set-ItemProperty -Path $k -Name QuietUninstallString -Value {}; \
         New-ItemProperty -Path $k -Name NoModify -Value 1 -PropertyType DWord -Force | Out-Null; \
         New-ItemProperty -Path $k -Name NoRepair -Value 1 -PropertyType DWord -Force | Out-Null",
        ps_str(&version),
        ps_str(&format!("{},0", icon_target.display())),
        ps_str(&install_dir.display().to_string()),
        ps_str(&format!("\"{}\" --uninstall", uninstaller.display())),
        ps_str(&format!("\"{}\" --uninstall --silent", uninstaller.display())),
    );
    let _ = run_ps(&reg); // 注册表被策略拦住时仍可用安装目录里的 Uninstall.exe
    Ok(())
}

fn launch(install_dir: &Path) {
    let shell = install_dir.join("Pawch Agent.exe");
    if shell.is_file() {
        let _ = Command::new(&shell).current_dir(install_dir).spawn();
        return;
    }
    let mut cmd = Command::new("cmd.exe");
    cmd.args(["/c", "start", "", "pawch-agent.exe"]);
    let _ = cmd.current_dir(install_dir).spawn();
}

fn main() {
    let args = parse_args();
    // 卸载模式：安装目录里的 Uninstall.exe --uninstall（或 --purge 延迟删除阶段）
    if args.purge {
        purge(&args);
        return;
    }
    if args.uninstall
        || std::env::current_exe()
            .ok()
            .is_some_and(|exe| looks_like_uninstaller(&exe))
    {
        uninstall(&args);
        return;
    }

    // payload 三层来源：--payload 显式 > 自身内嵌（单文件分发的主路径） >
    // 旁边 payload.zip（旧包/手工布局兜底）
    let mut staged: Option<PathBuf> = None;
    let payload: PathBuf = match args.payload.as_ref() {
        Some(p) => PathBuf::from(p),
        None => match embedded_payload() {
            Some(bytes) => match stage_embedded_payload(&bytes) {
                Some(p) => {
                    staged = Some(p.clone());
                    p
                }
                None => {
                    msg("安装失败", "内嵌安装内容读取失败，请重新下载安装包。");
                    return;
                }
            },
            None => exe_dir().join("payload.zip"),
        },
    };

    let (install_dir, data_dir) = match (&args.install_dir, &args.data_dir) {
        (Some(i), Some(d)) => (PathBuf::from(i), PathBuf::from(d)),
        _ => match wizard(&payload) {
            Some(v) => v,
            None => return,
        },
    };

    match install(&payload, &install_dir, &data_dir, !args.no_shortcuts) {
        Ok(()) => {}
        Err(e) => {
            msg("安装失败", e);
            // 内嵌 payload 的临时文件留着无害（%TEMP% 会自己清），失败路径不再折腾
            return;
        }
    }

    // 内嵌 payload 落地的临时 zip 用完即删
    if let Some(p) = &staged {
        if let Some(dir) = p.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    if !args.no_launch {
        launch(&install_dir);
    }
    // 不弹「安装完成」：应用窗口拉起来就是完成信号，
    // 失败才有弹窗（上面的「安装失败」）。
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(v: &[&str]) -> Args {
        parse_args_from(v.iter().map(|s| s.to_string()))
    }

    #[test]
    fn parses_install_flags() {
        let a = args(&[
            "--install-dir",
            "C:\\Apps\\Pawch Agent",
            "--data-dir",
            "D:\\data",
            "--silent",
            "--no-shortcuts",
            "--no-launch",
        ]);
        assert_eq!(a.install_dir.as_deref(), Some("C:\\Apps\\Pawch Agent"));
        assert_eq!(a.data_dir.as_deref(), Some("D:\\data"));
        assert!(a.silent && a.no_shortcuts && a.no_launch);
        assert!(!a.uninstall && !a.purge && !a.delete_data);
    }

    #[test]
    fn parses_uninstall_mode_and_ignores_unknown() {
        let a = args(&["--uninstall", "--silent", "--delete-data", "--wat", "x"]);
        assert!(a.uninstall && a.silent && a.delete_data);
        assert!(a.install_dir.is_none());
    }

    #[test]
    fn missing_option_value_stays_none() {
        let a = args(&["--install-dir"]);
        assert!(a.install_dir.is_none());
    }

    #[test]
    fn remove_dir_retry_eventually_reports() {
        // 不存在的目录直接算成功
        assert!(remove_dir_all_retry(Path::new("Z:\\definitely-not-here\\pawch-test"), 2, std::time::Duration::from_millis(1)).is_ok());
    }

    #[test]
    fn uninstaller_filename_defaults_to_uninstall() {
        assert!(looks_like_uninstaller(Path::new("D:\\Pawch Agent\\Uninstall.exe")));
        assert!(looks_like_uninstaller(Path::new("d:\\x\\UNINSTALL.EXE")));
        assert!(!looks_like_uninstaller(Path::new("D:\\dist\\Pawch Agent Setup.exe")));
        assert!(!looks_like_uninstaller(Path::new("payload.zip")));
    }

    #[test]
    fn powershell_quotes_escape_single_quotes() {
        assert_eq!(ps_quote(Path::new("C:/a'b")), "'C:/a''b'");
        assert_eq!(ps_str("it's"), "'it''s'");
    }

    #[test]
    fn parses_dirs_output_with_bom_and_quotes() {
        // 普通两行
        let (a, b) = parse_dirs_output("D:\\Pawch Agent\nD:\\data\n").unwrap();
        assert_eq!(a, PathBuf::from("D:\\Pawch Agent"));
        assert_eq!(b, PathBuf::from("D:\\data"));
        // 带 BOM + CRLF + 引号（粘贴/老 PS 写文件）
        let (a, b) = parse_dirs_output("\u{feff}D:\\Pawch Agent\r\n\"D:\\data\"\r\n").unwrap();
        assert_eq!(a, PathBuf::from("D:\\Pawch Agent"));
        assert_eq!(b, PathBuf::from("D:\\data"));
        // 空/只有 BOM
        assert!(parse_dirs_output("").is_none());
        assert!(parse_dirs_output("\u{feff}\r\n").is_none());
    }

    /// 造一段带内嵌 payload 尾部的可执行镜像字节（zip 数据 + 长度 + 魔数）
    fn embed_image(zip: &[u8]) -> Vec<u8> {
        let mut v = vec![0xC3, 0xB0, 0x01]; // 假 exe 正文
        v.extend_from_slice(zip);
        v.extend_from_slice(&(zip.len() as u64).to_le_bytes());
        v.extend_from_slice(EMBED_MAGIC);
        v
    }

    #[test]
    fn embedded_payload_layout_roundtrip() {
        // 最小合法 zip 头：PK\x03\x04 + 18 字节凑满 EOCD 下限
        let zip = [b'P', b'K', 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let img = embed_image(&zip);
        let tail = img.len() - EMBED_MAGIC.len();
        assert_eq!(&img[tail..], EMBED_MAGIC);
        let len_off = img.len() - (EMBED_TRAILER as usize);
        let zip_len = u64::from_le_bytes(img[len_off..len_off + 8].try_into().unwrap()) as usize;
        assert_eq!(zip_len, zip.len());
        assert_eq!(&img[len_off - zip_len..len_off], &zip);
    }

    #[test]
    fn embedded_payload_rejects_broken_tails() {
        // 没有尾部（裸 exe）→ 长度不足的判定条件
        let short_len = (EMBED_TRAILER as usize) + EMBED_MAGIC.len() - 1;
        let short = vec![0u8; short_len];
        assert!(short.len() < (EMBED_TRAILER as usize) + EMBED_MAGIC.len());
        // 魔数不对
        let zip = [b'P', b'K', 3, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        let mut bad = embed_image(&zip);
        let n = bad.len();
        bad[n - 1] = b'X';
        let tail = &bad[bad.len() - EMBED_MAGIC.len()..];
        assert_ne!(tail, EMBED_MAGIC);
        // zip 长度字段超过 len_off 可用范围 → 损坏
        let mut len_bad = embed_image(&zip);
        let lo = len_bad.len() - (EMBED_TRAILER as usize) - EMBED_MAGIC.len();
        len_bad[lo..lo + 8].copy_from_slice(&999999u64.to_le_bytes());
        let len = u64::from_le_bytes(len_bad[lo..lo + 8].try_into().unwrap()) as usize;
        assert!(lo < len); // checked_sub 会返回 None
    }
}
