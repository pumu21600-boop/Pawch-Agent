//! 版本更新（2026-11-12）：检查 GitHub Release、下载安装包（直连失败走镜像）、
//! SHA256 校验、静默覆盖安装并重启。
//!
//! 安装包是程序本体，用户数据不在这里：升级只覆盖安装目录，数据目录由指针带着走。

use axum::{Json, http::StatusCode};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const API_URL: &str =
    "https://api.github.com/repos/pumu21600-boop/Pawch-Agent/releases/latest";
/// GitHub API 走这个镜像（ghfast 对 api.github.com 返回 403，实测 gh-proxy 可以）
const API_MIRROR: &str = "https://gh-proxy.com/";
/// 安装包下载的镜像前缀（按顺序回退；可用 PAWCH_GITHUB_MIRROR 覆盖）
const MIRRORS: &[&str] = &["https://ghfast.top/", "https://gh-proxy.com/", "https://ghproxy.net/"];
const USER_AGENT: &str = "pawch-agent-updater";

fn mirrors() -> Vec<String> {
    if let Ok(m) = std::env::var("PAWCH_GITHUB_MIRROR") {
        let m = m.trim();
        if !m.is_empty() {
            return vec![if m.ends_with('/') { m.to_string() } else { format!("{m}/") }];
        }
    }
    MIRRORS.iter().map(|s| s.to_string()).collect()
}

fn client(timeout: Duration) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(timeout)
        .user_agent(USER_AGENT)
        .build()
        .map_err(|e| format!("建 HTTP 客户端失败: {e}"))
}

#[derive(Clone)]
struct Asset {
    name: String,
    url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Clone)]
struct Release {
    version: String,
    page_url: String,
    notes: String,
    setup: Asset,
    checksums_url: Option<String>,
    via_mirror: bool,
}

fn parse_version(s: &str) -> (u64, u64, u64) {
    let s = s.trim().trim_start_matches('v');
    let mut it = s.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (it.next().unwrap_or(0), it.next().unwrap_or(0), it.next().unwrap_or(0))
}

fn is_newer(latest: &str, current: &str) -> bool {
    parse_version(latest) > parse_version(current)
}

fn pick_asset(v: &Value, pred: impl Fn(&str) -> bool) -> Option<Asset> {
    let arr = v.get("assets")?.as_array()?;
    for a in arr {
        let name = a.get("name").and_then(|x| x.as_str()).unwrap_or("");
        if !pred(name) {
            continue;
        }
        return Some(Asset {
            name: name.to_string(),
            url: a
                .get("browser_download_url")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            size: a.get("size").and_then(|x| x.as_u64()).unwrap_or(0),
            digest: a.get("digest").and_then(|x| x.as_str()).map(|s| s.to_string()),
        });
    }
    None
}

async fn fetch_release_uncached() -> Result<Release, String> {
    let c = client(Duration::from_secs(15))?;
    let mut urls = vec![API_URL.to_string()];
    urls.push(format!("{API_MIRROR}{API_URL}"));
    let mut last_err = String::new();
    for (i, url) in urls.iter().enumerate() {
        match c.get(url).header("Accept", "application/vnd.github+json").send().await {
            Ok(r) if r.status().is_success() => {
                let via_mirror = i > 0;
                let v: Value = r.json().await.map_err(|e| format!("解析 Release 失败: {e}"))?;
                let tag = v.get("tag_name").and_then(|x| x.as_str()).unwrap_or("");
                if tag.is_empty() {
                    last_err = "Release 里没有 tag_name".into();
                    continue;
                }
                let setup = pick_asset(&v, |n| n.starts_with("Pawch-Agent-Setup-") && n.ends_with(".exe"))
                    .ok_or("Release 里没有安装包资产")?;
                let checksums = pick_asset(&v, |n| n == "SHA256SUMS").map(|a| a.url);
                return Ok(Release {
                    version: tag.trim_start_matches('v').to_string(),
                    page_url: v
                        .get("html_url")
                        .and_then(|x| x.as_str())
                        .unwrap_or("https://github.com/pumu21600-boop/Pawch-Agent/releases")
                        .to_string(),
                    notes: v
                        .get("body")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .chars()
                        .take(4000)
                        .collect(),
                    setup,
                    checksums_url: checksums,
                    via_mirror,
                });
            }
            Ok(r) => last_err = format!("HTTP {}", r.status()),
            Err(e) => last_err = e.to_string(),
        }
    }
    Err(format!("检查更新失败：{last_err}"))
}

static CHECK_CACHE: Mutex<Option<(Instant, Value)>> = Mutex::new(None);

/// GET /api/update/check —— 查最新 Release（60 秒缓存，避免刷接口限流）。
pub(super) async fn update_check() -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if let Ok(g) = CHECK_CACHE.lock()
        && let Some((at, v)) = g.as_ref()
        && at.elapsed() < Duration::from_secs(60)
    {
        return Ok(Json(v.clone()));
    }
    let rel = fetch_release_uncached()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))))?;
    let current = env!("CARGO_PKG_VERSION");
    let out = json!({
        "ok": true,
        "current": current,
        "latest": rel.version,
        "has_update": is_newer(&rel.version, current),
        "page_url": rel.page_url,
        "notes": rel.notes,
        "asset_name": rel.setup.name,
        "asset_size": rel.setup.size,
        "via_mirror": rel.via_mirror,
    });
    if let Ok(mut g) = CHECK_CACHE.lock() {
        *g = Some((Instant::now(), out.clone()));
    }
    Ok(Json(out))
}

#[derive(Clone, Serialize, Default)]
struct DlState {
    running: bool,
    done: bool,
    error: Option<String>,
    bytes: u64,
    total: u64,
    path: Option<String>,
    verified: bool,
    version: String,
    used_mirror: bool,
}

static DL: Mutex<Option<DlState>> = Mutex::new(None);

fn set_dl(f: impl FnOnce(&mut DlState)) {
    if let Ok(mut g) = DL.lock() {
        let st = g.get_or_insert_with(DlState::default);
        f(st);
    }
}

fn get_dl() -> DlState {
    DL.lock().ok().and_then(|g| g.clone()).unwrap_or_default()
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("读安装包失败: {e}"))?;
    let mut h = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).map_err(|e| format!("读安装包失败: {e}"))?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(hex::encode(h.finalize()))
}

/// 从 SHA256SUMS 文本里取指定文件名的 hash。
fn sums_lookup(text: &str, name: &str) -> Option<String> {
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let file = it.next()?.trim_start_matches('*');
        if file == name && hash.len() == 64 {
            return Some(hash.to_ascii_lowercase());
        }
    }
    None
}

async fn fetch_checksum(release: &Release) -> Option<String> {
    let url = release.checksums_url.as_ref()?;
    let c = client(Duration::from_secs(20)).ok()?;
    let mut urls = vec![url.clone()];
    for m in mirrors() {
        urls.push(format!("{m}{url}"));
    }
    for u in urls {
        if let Ok(r) = c.get(&u).send().await
            && r.status().is_success()
            && let Ok(t) = r.text().await
            && let Some(h) = sums_lookup(&t, &release.setup.name)
        {
            return Some(h);
        }
    }
    None
}

async fn download_to(url: &str, dest: &Path, total_hint: u64) -> Result<(), String> {
    let c = client(Duration::from_secs(900))?;
    let r = c.get(url).send().await.map_err(|e| e.to_string())?;
    if !r.status().is_success() {
        return Err(format!("HTTP {}", r.status()));
    }
    let total = r.content_length().unwrap_or(total_hint);
    set_dl(|s| {
        s.total = total;
        s.bytes = 0;
    });
    let mut f = std::fs::File::create(dest).map_err(|e| format!("建临时文件失败: {e}"))?;
    let mut stream = r.bytes_stream();
    use futures_util::StreamExt;
    use std::io::Write;
    while let Some(chunk) = stream.next().await {
        let b = chunk.map_err(|e| format!("下载中断: {e}"))?;
        f.write_all(&b).map_err(|e| format!("写安装包失败: {e}"))?;
        let n = b.len() as u64;
        set_dl(|s| s.bytes += n);
    }
    f.flush().map_err(|e| format!("写安装包失败: {e}"))?;
    Ok(())
}

/// POST /api/update/download —— 后台下载最新安装包 + SHA256 校验。
pub(super) async fn update_download() -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    if get_dl().running {
        return Ok(Json(json!({ "ok": true, "started": false, "reason": "already running" })));
    }
    let release = fetch_release_uncached()
        .await
        .map_err(|e| (StatusCode::BAD_GATEWAY, Json(json!({ "error": e }))))?;
    let dest_dir = crate::config::data_root().join("cache").join("updates");
    std::fs::create_dir_all(&dest_dir)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({ "error": format!("建下载目录失败: {e}") }))))?;
    let dest = dest_dir.join(&release.setup.name);
    let part = dest.with_extension("exe.part");
    set_dl(|s| {
        *s = DlState {
            running: true,
            total: release.setup.size,
            version: release.version.clone(),
            ..DlState::default()
        }
    });
    tokio::spawn(async move {
        let mut used_mirror = false;
        let mut last_err = String::new();
        let mut ok = false;
        // 镜像优先（用户在国内，直连 release 资产经常挂住）；直连留作最后兜底。
        let mirror_urls: Vec<String> = mirrors()
            .iter()
            .map(|m| format!("{m}{}", release.setup.url))
            .collect();
        let mirror_count = mirror_urls.len();
        let mut urls = mirror_urls;
        urls.push(release.setup.url.clone());
        for (i, u) in urls.iter().enumerate() {
            used_mirror = i < mirror_count;
            match download_to(u, &part, release.setup.size).await {
                Ok(()) => {
                    ok = true;
                    break;
                }
                Err(e) => last_err = e,
            }
        }
        if !ok {
            let _ = std::fs::remove_file(&part);
            set_dl(|s| {
                s.running = false;
                s.error = Some(format!("下载失败：{last_err}"));
            });
            return;
        }
        let expected = match release.setup.digest.as_deref() {
            Some(d) => d.trim().trim_start_matches("sha256:").to_ascii_lowercase(),
            None => fetch_checksum(&release).await.unwrap_or_default(),
        };
        let actual = match sha256_file(&part) {
            Ok(h) => h,
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                set_dl(|s| {
                    s.running = false;
                    s.error = Some(e);
                });
                return;
            }
        };
        let verified = !expected.is_empty() && expected == actual;
        if !verified {
            let _ = std::fs::remove_file(&part);
            set_dl(|s| {
                s.running = false;
                s.error = Some(if expected.is_empty() {
                    "拿不到官方 SHA256，拒绝自动安装（请手动下载）".into()
                } else {
                    "SHA256 校验失败，安装包已删除".into()
                });
            });
            return;
        }
        let _ = std::fs::rename(&part, &dest);
        set_dl(|s| {
            s.running = false;
            s.done = true;
            s.verified = true;
            s.path = Some(dest.display().to_string());
            s.used_mirror = used_mirror;
        });
    });
    Ok(Json(json!({ "ok": true, "started": true })))
}

/// GET /api/update/progress —— 下载进度（前端 400ms 轮询）。
pub(super) async fn update_progress() -> Json<Value> {
    Json(json!(get_dl()))
}

fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "''"))
}

/// POST /api/update/install —— 停服务 → 静默覆盖安装 → 重启壳。
pub(super) async fn update_install() -> Result<Json<Value>, (StatusCode, Json<Value>)> {
    let st = get_dl();
    let Some(path) = st.path.clone().filter(|_| st.done && st.verified) else {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "安装包还没下载/校验完成" })),
        ));
    };
    let setup = PathBuf::from(path);
    if !setup.is_file() {
        return Err((StatusCode::BAD_REQUEST, Json(json!({ "error": "安装包文件不存在" }))));
    }
    let install = crate::config::app_root();
    let data = crate::config::data_root();
    let exe = std::env::current_exe().unwrap_or_else(|_| install.join("pawch-agent.exe"));
    let script = format!(
        "$env:PAWCH_AGENT_DATA_DIR = {data_q}; \
         Remove-Item Env:AGENT_DATA_DIR -ErrorAction SilentlyContinue; \
         Start-Sleep -Milliseconds 900; \
         & {exe_q} stop | Out-Null; \
         Start-Sleep -Milliseconds 800; \
         Get-Process -Name 'Pawch Agent' -ErrorAction SilentlyContinue | Stop-Process -Force; \
         $setupArgs = '--silent --install-dir \"' + {install_q} + '\" --data-dir \"' + {data_q} + '\" --no-launch'; \
         Start-Process -FilePath {setup_q} -ArgumentList $setupArgs -Wait; \
         $shell = Join-Path {install_q} 'Pawch Agent.exe'; \
         if (Test-Path -LiteralPath $shell) {{ Start-Process -FilePath $shell -WorkingDirectory {install_q} }}",
        data_q = ps_quote(&data),
        exe_q = ps_quote(&exe),
        setup_q = ps_quote(&setup),
        install_q = ps_quote(&install),
    );
    spawn_ps(&script).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "error": format!("启动安装失败: {e}") })),
        )
    })?;
    Ok(Json(json!({ "ok": true, "restarting": true })))
}

fn spawn_ps(script: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let stamp = std::process::id();
    let path = std::env::temp_dir().join(format!("pawch-update-{stamp}.ps1"));
    let log = std::env::temp_dir().join(format!("pawch-update-{stamp}.log"));
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice(script.as_bytes());
    std::fs::write(&path, bytes).map_err(|e| format!("写更新脚本失败: {e}"))?;
    let out = std::fs::File::create(&log).map_err(|e| format!("建更新日志失败: {e}"))?;
    let err = out.try_clone().map_err(|e| format!("复制日志句柄失败: {e}"))?;
    std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-WindowStyle", "Hidden", "-ExecutionPolicy", "Bypass", "-File"])
        .arg(&path)
        .stdout(out)
        .stderr(err)
        .creation_flags(0x0800_0000)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("启动 PowerShell 失败: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_compare() {
        assert!(is_newer("1.3.1", "1.3.0"));
        assert!(is_newer("v1.4.0", "1.3.9"));
        assert!(!is_newer("1.3.0", "1.3.0"));
        assert!(!is_newer("1.2.9", "1.3.0"));
    }

    #[test]
    fn sums_lookup_matches_binary_and_star() {
        let t = "abc  file-a\n".to_string() + &"d".repeat(64) + " *Pawch-Agent-Setup-1.3.0.exe\n";
        assert_eq!(
            sums_lookup(&t, "Pawch-Agent-Setup-1.3.0.exe"),
            Some("d".repeat(64))
        );
        assert_eq!(sums_lookup(&t, "other"), None);
    }
}
