//! adb 命令封装：装 APK / 启动应用 / UI 树 / logcat / 任意 shell。
//!
//! adb 是 SDK 里的独立二进制，自己起 server（5037）；我们只 `Command` 调它，
//! 不长连。所有调用 `CREATE_NO_WINDOW`（服务是后台进程，不许弹控制台）。

use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::Duration;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 跑一条 adb 命令（可带 -s serial），超时杀掉并报错。
pub(crate) fn run(adb: &PathBuf, serial: Option<&str>, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut cmd = Command::new(adb);
    if let Some(s) = serial {
        cmd.arg("-s").arg(s);
    }
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let child = cmd
        .spawn()
        .map_err(|e| format!("adb 起不来（{}）: {e}", adb.display()))?;
    // adb 命令都可能挂死（设备掉线时 install/logcat 会等），统一超时兜底
    match wait_with_timeout(child, timeout) {
        Ok((code, out, err)) => {
            let text = format!("{}{}", out, if err.trim().is_empty() { String::new() } else { format!("\n{}", err.trim()) });
            if code == Some(0) {
                Ok(text)
            } else {
                Err(format!("adb {} 失败(exit={:?}): {}", args.first().unwrap_or(&""), code, text.trim()))
            }
        }
        Err(_) => Err(format!("adb {} 超时（{}s）", args.first().unwrap_or(&""), timeout.as_secs())),
    }
}

fn wait_with_timeout(
    mut child: std::process::Child,
    timeout: Duration,
) -> Result<(Option<i32>, String, String), ()> {
    use std::io::Read;
    let mut out = std::mem::take(&mut child.stdout).unwrap_or_else(|| panic_pipe());
    let mut err = std::mem::take(&mut child.stderr).unwrap_or_else(|| panic_pipe());
    let t = std::thread::spawn(move || {
        let mut ob = Vec::new();
        let _ = out.read_to_end(&mut ob);
        ob
    });
    let te = std::thread::spawn(move || {
        let mut eb = Vec::new();
        let _ = err.read_to_end(&mut eb);
        eb
    });
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let ob = t.join().unwrap_or_default();
                let eb = te.join().unwrap_or_default();
                let text = String::from_utf8_lossy(&ob).into_owned();
                let errs = String::from_utf8_lossy(&eb).into_owned();
                return Ok((status.code(), text, errs));
            }
            Ok(None) => {
                if std::time::Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(());
                }
                std::thread::sleep(Duration::from_millis(40));
            }
            Err(_) => return Err(()),
        }
    }
}

fn panic_pipe() -> ! {
    unreachable!("stdout/stderr 都声明了 piped")
}

/// 装 APK（-r 覆盖安装）。
pub(crate) fn install(adb: &PathBuf, serial: &str, apk: &std::path::Path) -> Result<String, String> {
    let apk_str = apk.display().to_string();
    let out = run(adb, Some(serial), &["install", "-r", &apk_str], Duration::from_secs(180))?;
    if out.contains("Success") {
        Ok(out.trim().to_string())
    } else {
        Err(format!("adb install 未返回 Success: {}", out.trim()))
    }
}

/// 启动应用（package 可带 /activity；不带则解析 launcher activity）。
pub(crate) fn launch(adb: &PathBuf, serial: &str, package: &str, activity: Option<&str>) -> Result<String, String> {
    let target = match activity {
        Some(a) => format!("{package}/{a}"),
        None => package.to_string(),
    };
    let target_ref = target.as_str();
    let args: Vec<&str> = match activity {
        Some(_) => vec!["shell", "am", "start", "-n", target_ref],
        None => vec!["shell", "monkey", "-p", package, "-c", "android.intent.category.LAUNCHER", "1"],
    };
    run(adb, Some(serial), &args, Duration::from_secs(30))
}

/// uiautomator dump：把当前界面控件树写到设备上的固定文件再 cat 回来。
pub(crate) fn ui_dump(adb: &PathBuf, serial: &str) -> Result<String, String> {
    run(adb, Some(serial), &["shell", "rm", "-f", "/sdcard/pawch-ui.xml"], Duration::from_secs(10)).ok();
    let d = run(adb, Some(serial), &["shell", "uiautomator", "dump", "/sdcard/pawch-ui.xml"], Duration::from_secs(20))?;
    if !d.contains("dumped") && !d.contains("true") {
        // 部分系统镜像不打 "dumped"，以文件是否可读为准
    }
    run(adb, Some(serial), &["shell", "cat", "/sdcard/pawch-ui.xml"], Duration::from_secs(10))
}

/// logcat 最近 N 行。
pub(crate) fn logcat(adb: &PathBuf, serial: &str, lines: usize) -> Result<String, String> {
    let n = lines.clamp(1, 5000).to_string();
    let n_ref = n.as_str();
    run(adb, Some(serial), &["logcat", "-d", "-t", n_ref], Duration::from_secs(20))
}

/// 任意 shell（高危，按审批档位走）。
pub(crate) fn shell(adb: &PathBuf, serial: &str, cmd: &str) -> Result<String, String> {
    run(adb, Some(serial), &["shell", cmd], Duration::from_secs(60))
}

/// 单个 `uiautomator` node 的紧凑视图。
#[derive(Debug, Clone)]
pub(crate) struct UiNode {
    pub text: String,
    pub desc: String,
    pub resource: String,
    pub clickable: bool,
    pub bounds: (i64, i64, i64, i64),
}

/// 从 uiautomator XML 提取节点。手写属性扫描（不引 XML crate）：
/// 逐个 `<node ...>` 标签抽关键属性，`bounds="[x1,y1][x2,y2]"` 单独解析。
pub(crate) fn parse_ui_nodes(xml: &str) -> Vec<UiNode> {
    let mut out = Vec::new();
    for seg in xml.split("<node ").skip(1) {
        let seg = match seg.split('>').next() {
            Some(s) => s,
            None => continue,
        };
        let attr = |name: &str| -> String {
            let pat = format!("{name}=\"");
            seg.find(&pat)
                .map(|i| {
                    let rest = &seg[i + pat.len()..];
                    rest.find('"').map(|j| rest[..j].to_string()).unwrap_or_default()
                })
                .unwrap_or_default()
        };
        let bounds = attr("bounds");
        let (x1, y1, x2, y2) = parse_bounds(&bounds);
        if x2 <= x1 || y2 <= y1 {
            continue;
        }
        out.push(UiNode {
            text: unescape(&attr("text")),
            desc: unescape(&attr("content-desc")),
            resource: attr("resource-id"),
            clickable: attr("clickable") == "true",
            bounds: (x1, y1, x2, y2),
        });
    }
    out
}

fn parse_bounds(s: &str) -> (i64, i64, i64, i64) {
    let nums: Vec<i64> = s
        .split(|c: char| !c.is_ascii_digit() && c != '-')
        .filter(|p| !p.is_empty())
        .filter_map(|p| p.parse().ok())
        .collect();
    if nums.len() >= 4 {
        (nums[0], nums[1], nums[2], nums[3])
    } else {
        (0, 0, 0, 0)
    }
}

fn unescape(s: &str) -> String {
    s.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_node_parse_extracts_text_and_center() {
        // 第一个节点带全屏 bounds（真机 uiautomator 的根节点都有 bounds）；
        // 无 bounds 的节点（空串）应被过滤——单独一条测试覆盖
        let xml = r#"<hierarchy><node index="0" text="" class="android.widget.FrameLayout" clickable="false" bounds="[0,0][1080,2400]"/>
<node index="1" text="从教务系统导入" content-desc="" resource-id="com.sgu.timetable:id/import_btn" class="android.widget.Button" clickable="true" bounds="[64,480][1016,560]"/></hierarchy>"#;
        let nodes = parse_ui_nodes(xml);
        assert_eq!(nodes.len(), 2);
        let b = &nodes[1];
        assert_eq!(b.text, "从教务系统导入");
        assert!(b.clickable);
        assert_eq!(b.bounds, (64, 480, 1016, 560));
        assert_eq!((b.bounds.0 + b.bounds.2) / 2, 540);
    }

    #[test]
    fn ui_node_without_bounds_is_dropped() {
        let xml = r#"<node index="0" text="无bounds" clickable="true"/>"#;
        assert!(parse_ui_nodes(xml).is_empty());
    }

    #[test]
    fn bounds_parse_tolerates_garbage() {
        assert_eq!(parse_bounds("[0,0][0,0]"), (0, 0, 0, 0));
        assert_eq!(parse_bounds(""), (0, 0, 0, 0));
        assert_eq!(parse_bounds("[10,20][30,40]"), (10, 20, 30, 40));
    }

    #[test]
    fn unescape_xml_entities() {
        assert_eq!(unescape("a&amp;b&lt;c&gt;d&quot;e&quot;"), "a&b<c>d\"e\"");
    }
}
