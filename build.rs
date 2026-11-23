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
    compile_android_proto();
    #[cfg(windows)]
    embed_icon();
}

/// 安卓模拟器 gRPC 契约：proto vendor 在 `src/android/proto/`（Apache-2.0，
/// 取自 Android Emulator 37.3.10）。protoc 用 protoc-bin-vendored 自带的二进制，
/// 构建机不需要另装 protoc。
fn compile_android_proto() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let proto_dir = std::path::PathBuf::from(manifest).join("src").join("android").join("proto");
    let proto = proto_dir.join("emulator_controller.proto");
    println!("cargo:rerun-if-changed={}", proto.display());
    println!(
        "cargo:rerun-if-changed={}",
        proto_dir.join("google").join("protobuf").join("empty.proto").display()
    );
    if let Ok(protoc) = protoc_bin_vendored::protoc_bin_path() {
        // SAFETY: build.rs 单线程执行，只在本进程内给 prost-build 指定 protoc 路径。
        unsafe { std::env::set_var("PROTOC", protoc) };
    }
    // tonic 0.14 起 prost 代码生成拆到 tonic-prost-build（configure 也搬了过去）
    tonic_prost_build::configure()
        .build_server(false)
        .build_client(true)
        .compile_protos(&[&proto], &[&proto_dir])
        .expect("编译 emulator_controller.proto 失败");
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
