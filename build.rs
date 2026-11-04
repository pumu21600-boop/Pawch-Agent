//! 构建脚本：把应用图标编进**服务**二进制。
//!
//! 壳（desktop/）的 exe 图标由 tauri-build 按 tauri.conf.json 的 bundle.icon 嵌入；
//! 服务是自己 cargo build 出来的裸 bin，一直是默认的空白窗口图标——2026-11-01 用户
//! 实锤「二进制好像还不是新图标」。这里用 Windows SDK 的 rc.exe 编译一个只含图标的
//! .rc，再通过 rustc-link-arg 把 .res 交给链接器。
//!
//! 找不到 rc.exe / 非 Windows / 图标文件缺失 → 静默跳过（不能让构建挂在这儿）。

fn main() {
    println!("cargo:rerun-if-changed=desktop/icons/icon.ico");
    println!("cargo:rerun-if-changed=build.rs");
    #[cfg(windows)]
    embed_icon();
}

#[cfg(windows)]
fn embed_icon() {
    use std::path::PathBuf;
    use std::process::Command;

    let Ok(manifest) = std::env::var("CARGO_MANIFEST_DIR") else { return };
    let Ok(out_dir) = std::env::var("OUT_DIR") else { return };
    let ico = PathBuf::from(&manifest).join("desktop").join("icons").join("icon.ico");
    if !ico.exists() {
        return;
    }
    let Some(rc_exe) = find_rc() else { return };

    let out = PathBuf::from(out_dir);
    let rc_file = out.join("pawch-icon.rc");
    let res_file = out.join("pawch-icon.res");
    // 资源脚本只需一行图标声明；路径里的反斜杠要转义成 \\
    let icon_lit = ico.display().to_string().replace('\\', "\\\\");
    if std::fs::write(&rc_file, format!("1 ICON \"{icon_lit}\"\n")).is_err() {
        return;
    }
    let ok = Command::new(&rc_exe)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res_file)
        .arg(&rc_file)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if ok && res_file.exists() {
        println!("cargo:rustc-link-arg={}", res_file.display());
    }
}

/// Windows SDK 里的 rc.exe：取 `%ProgramFiles(x86)%\Windows Kits\10\bin\<最高版本>\x64\rc.exe`，
/// 退而求其次用 PATH 上的 rc。
#[cfg(windows)]
fn find_rc() -> Option<std::path::PathBuf> {
    let kits = std::env::var("ProgramFiles(x86)")
        .ok()
        .map(|p| std::path::PathBuf::from(p).join("Windows Kits").join("10").join("bin"));
    if let Some(dir) = kits
        && let Ok(entries) = std::fs::read_dir(&dir)
    {
        let mut versions: Vec<_> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.join("x64").join("rc.exe").exists())
            .collect();
        versions.sort();
        if let Some(v) = versions.pop() {
            return Some(v.join("x64").join("rc.exe"));
        }
    }
    Some(std::path::PathBuf::from("rc.exe"))
}
