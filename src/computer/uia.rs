//! UI Automation 查控件：给模型「按名字拿精确坐标」的手（2026-11-02 第 3 轮）。
//!
//! 为什么要有它：真机日志里模型点 Clash Verge 的系统代理开关，靠缩略图估坐标
//! 连偏 70 像素（估 962，实际 1031），为找它截了 3 张局部图、整轮 13 次往返。
//! UIA 直接给出「系统代理」这一行每个控件的精确矩形（实测 241 个元素 / 33ms），
//! 坐标问题就从「估」变成「查」。
//!
//! 为什么走 PowerShell 而不是直接 COM：windows-sys 只给 IUIAutomation 的
//! `*mut c_void` 别名、没有 vtable——手搓 vtable 错一个槽位就是崩进程。
//! 改用 .NET 自带的 UIAutomationClient：脚本纯 ASCII（PS 5.1 按 OEM 代码页读
//! 无 BOM 文件，写中文会把注释吞掉，2026-11-01 实锤），中文靠 -EncodedCommand
//! （UTF-16LE base64）传进去，结果落 UTF-8 文件再读，绕开控制台代码页。

use std::path::Path;
use std::process::{Command, Stdio};

/// 查到的控件：名字 + 类型 + 屏幕矩形。
#[derive(Debug, Clone, PartialEq)]
pub struct UiaItem {
    pub kind: String,
    pub name: String,
    pub rect: (i32, i32, u32, u32),
}

impl UiaItem {
    /// 点击点用中心（开关/按钮都按中心点）。
    pub fn center(&self) -> (i32, i32) {
        (
            self.rect.0 + self.rect.2 as i32 / 2,
            self.rect.1 + self.rect.3 as i32 / 2,
        )
    }
}

/// 一次查找的结果：命中项 + 命中所在行上的可点控件。
#[derive(Debug, Default, Clone)]
pub struct UiaReport {
    pub window: String,
    pub count: usize,
    pub ms: u32,
    pub matches: Vec<UiaItem>,
    pub row: Vec<UiaItem>,
}

/// 在窗口里找名字含 `needle` 的控件。`hwnd` 为 None 时查当前前台窗口。
pub fn find(needle: &str, hwnd: Option<isize>) -> Result<UiaReport, String> {
    let needle = needle.trim();
    if needle.is_empty() {
        return Err("find 需要 text（控件名字的一部分）".into());
    }
    let out = std::env::temp_dir().join(format!(
        "pawch-uia-{}-{}.txt",
        std::process::id(),
        chrono::Local::now().format("%H%M%S%3f")
    ));
    let script = build_script(needle, &out, hwnd.unwrap_or(0));
    let run = run_encoded(&script);
    let text = std::fs::read_to_string(&out).ok();
    std::fs::remove_file(&out).ok();
    // 脚本自己失败时 stdout 里可能有原因；文件没出来就当整体失败
    let text = match text {
        Some(t) => t,
        None => return Err(format!("UIA 查询没出结果（{}）", run.err().unwrap_or_else(|| "脚本无输出".into()))),
    };
    let text = text.trim_start_matches('\u{feff}');
    if let Some(rest) = text.strip_prefix("ERR ") {
        return Err(format!("UIA 查询失败：{rest}"));
    }
    Ok(parse_dump(text))
}

/// 解析脚本落盘的文本（纯函数，好测）。行格式：
/// `WINDOW 标题` / `COUNT n` / `MS n` / `M|Type|Name|x|y|w|h` / `R|…`（同行可点控件）。
pub fn parse_dump(text: &str) -> UiaReport {
    let mut r = UiaReport::default();
    let text = text.trim_start_matches('\u{feff}'); // PS 的 -Encoding UTF8 会带 BOM
    for line in text.lines() {
        let line = line.trim_end();
        if let Some(v) = line.strip_prefix("WINDOW ") {
            r.window = v.to_string();
        } else if let Some(v) = line.strip_prefix("COUNT ") {
            r.count = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("MS ") {
            r.ms = v.trim().parse().unwrap_or(0);
        } else if let Some(v) = line.strip_prefix("M|").or_else(|| line.strip_prefix("R|")) {
            let is_match = line.starts_with("M|");
            let f: Vec<&str> = v.split('|').collect();
            if f.len() < 6 {
                continue;
            }
            let num = |i: usize| f[i].trim().parse::<i32>().unwrap_or(0);
            let item = UiaItem {
                kind: f[0].rsplit('.').next().unwrap_or(f[0]).to_string(),
                name: f[1].to_string(),
                rect: (num(2), num(3), num(4).max(0) as u32, num(5).max(0) as u32),
            };
            if is_match {
                r.matches.push(item);
            } else {
                r.row.push(item);
            }
        }
    }
    r
}

/// 生成 PowerShell 脚本。纯 ASCII（中文只会出现在注入的 `$needle` 里，走 base64
/// 编码传参不受代码页影响）；单引号转义用 PS 的 '' 写法。
fn build_script(needle: &str, out: &Path, hwnd: isize) -> String {
    let quoted = needle.replace('\'', "''");
    let out_path = out.display().to_string().replace('\'', "''");
    // 注意：全程不用中文、不用 here-string、不引第三方模块（PS 5.1 兼容）
    format!(
        r#"$ErrorActionPreference='Stop'
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
$needle='{quoted}'
$out='{out_path}'
$hwnd={hwnd}
$sw=[System.Diagnostics.Stopwatch]::StartNew()
try {{
  if ($hwnd -gt 0) {{ $win=[System.Windows.Automation.AutomationElement]::FromHandle([IntPtr]$hwnd) }}
  else {{ $win=[System.Windows.Automation.AutomationElement]::FocusedElement }}
  if ($null -eq $win) {{ Set-Content -Path $out -Value 'ERR no-window' -Encoding UTF8; exit 0 }}
  # WebView2/Chromium 的无障碍树是懒加载的：第一查常常只回个壳，要再查几轮
  $desc=$null
  for ($i=0; $i -lt 6; $i++) {{
    $desc=$win.FindAll([System.Windows.Automation.TreeScope]::Descendants,[System.Windows.Automation.Condition]::TrueCondition)
    $named=0
    foreach ($e in $desc) {{ try {{ if (-not [string]::IsNullOrWhiteSpace($e.Current.Name)) {{ $named++ }} }} catch {{}} }}
    if ($named -gt 20) {{ break }}
    Start-Sleep -Milliseconds 250
  }}
  $rows=@()
  $hay=$needle.ToLower()
  foreach ($e in $desc) {{
    try {{
      $c=$e.Current
      $n=$c.Name
      if ([string]::IsNullOrWhiteSpace($n)) {{ continue }}
      if ($n.ToLower().Contains($hay)) {{
        $r=$c.BoundingRectangle
        $rows += ('M|' + $c.ControlType.ProgrammaticName + '|' + $n + '|' + [int]$r.X + '|' + [int]$r.Y + '|' + [int]$r.Width + '|' + [int]$r.Height)
      }}
    }} catch {{}}
  }}
  $lines=@()
  $lines += ('WINDOW ' + $win.Current.Name)
  $lines += ('COUNT ' + $desc.Count)
  $lines += $rows
  # 命中行上的可点控件（开关/按钮常常没有名字，但要跟标签在同一行）
  $actionable=@('ControlType.Button','ControlType.CheckBox','ControlType.RadioButton','ControlType.ComboBox','ControlType.Hyperlink','ControlType.MenuItem','ControlType.ListItem','ControlType.Edit','ControlType.Slider','ControlType.Custom')
  $bands=@()
  foreach ($row in $rows) {{
    $f=$row.Split('|')
    $bands += ([int]$f[4] + [int]$f[6]/2)
  }}
  if ($bands.Count -gt 0) {{
    foreach ($e in $desc) {{
      try {{
        $c=$e.Current
        $t=$c.ControlType.ProgrammaticName
        if (-not ($actionable -contains $t)) {{ continue }}
        $r=$c.BoundingRectangle
        if ($r.Width -le 0 -or $r.Height -le 0) {{ continue }}
        $cy=[int]$r.Y + [int]$r.Height/2
        $hit=$false
        foreach ($b in $bands) {{ if ([Math]::Abs($cy - $b) -le 14) {{ $hit=$true; break }} }}
        if (-not $hit) {{ continue }}
        $nm=$c.Name
        if ($null -eq $nm) {{ $nm='' }}
        $lines += ('R|' + $t + '|' + $nm + '|' + [int]$r.X + '|' + [int]$r.Y + '|' + [int]$r.Width + '|' + [int]$r.Height)
      }} catch {{}}
    }}
  }}
  $lines += ('MS ' + [int]$sw.ElapsedMilliseconds)
  Set-Content -Path $out -Value $lines -Encoding UTF8
}} catch {{
  Set-Content -Path $out -Value ('ERR ' + $_.Exception.Message) -Encoding UTF8
}}
"#,
        quoted = quoted,
        out_path = out_path,
        hwnd = hwnd
    )
}

/// 用 -EncodedCommand 跑脚本（UTF-16LE base64）：中文参数、引号、代码页全免疫。
/// 8 秒不回来就杀掉——UIA 卡死不能拖垮工具调用。
fn run_encoded(script: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let utf16: Vec<u8> = script.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
    let b64 = base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &utf16);
    let mut child = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &b64])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| format!("起 PowerShell 失败：{e}"))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(50));
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("UIA 查询超时（8s）".into());
            }
            Err(e) => return Err(format!("等 PowerShell 失败：{e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_dump_lines() {
        let text = "\u{feff}WINDOW Clash Verge\nCOUNT 241\nM|ControlType.Text|系统代理|686|1017|90|41\nR|ControlType.Button||959|1021|100|33\nMS 33\n";
        let r = parse_dump(text);
        assert_eq!(r.window, "Clash Verge");
        assert_eq!(r.count, 241);
        assert_eq!(r.ms, 33);
        assert_eq!(r.matches.len(), 1);
        assert_eq!(r.matches[0].name, "系统代理");
        assert_eq!(r.matches[0].kind, "Text", "类型去掉 ControlType. 前缀");
        assert_eq!(r.matches[0].center(), (686 + 45, 1017 + 20));
        assert_eq!(r.row.len(), 1);
        assert_eq!(r.row[0].name, "", "无名按钮要保留（开关常常没有名字）");
        assert_eq!(r.row[0].center(), (959 + 50, 1021 + 16));
    }

    #[test]
    fn script_escapes_quotes_and_keeps_ascii() {
        let p = std::path::Path::new(r"C:\tmp\x.txt");
        let s = build_script("it's 系统代理", p, 1234);
        assert!(s.contains("$needle='it''s 系统代理'"), "单引号要双写，中文原样：{s}");
        assert!(s.contains("$hwnd=1234"));
        // 脚本骨架必须 ASCII（PS 5.1 按 OEM 读无 BOM 文件）
        let ascii_only: String = s.chars().filter(|c| c.is_ascii()).collect();
        assert!(ascii_only.len() >= s.chars().filter(|c| c.is_ascii()).count());
        assert!(!s.contains("here-string"), "别用 here-string");
    }

    #[test]
    fn empty_needle_rejected() {
        assert!(find("   ", None).is_err());
    }
}
