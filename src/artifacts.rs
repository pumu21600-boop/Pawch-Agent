//! 会话产物账本 —— 「这个对话改到过哪些文件」的唯一数据源。
//!
//! 为什么不能用工作区 diff / 会话起点基线代替（2026-11-06 实测）：
//! 产物面板原来拿「会话起点快照 → 工作区当前状态」做差，一个会话只要活得够久，
//! 期间别的会话往同一工作区写的文件就必然算到它头上。校园网会话（11-05 11:39 创建）
//! 的产物面板里冒出便利店会话 22:02–23:45 才生成的 index.html 与整目录截图，
//! 就是这条路径。时间下界（session.created_at）也救不了：那些文件的 mtime 确实
//! 晚于会话创建时间。判据本身不成立，必须换成「哪个会话在哪一轮改了哪些文件」的记录。
//!
//! 记账时机：每轮任务收尾，用任务起点基线到任务结束快照的 diff 取候选路径，再按
//! **本轮任务时间窗**过滤（mtime ≥ 任务开始），把真正在本轮被改动/创建的文件并入
//! 会话账本。删除态不记（产物面板点开已不存在的文件没有意义）。
//!
//! 账本按 session id 分文件，存 `<数据目录>/logs/artifacts/<session>.json`；
//! workspace 一并存下，切工作区后旧账本不会冒充当前工作区的产物。
//! 旧会话没有账本时前端只回退到消息里的 write_file/edit_file 路径，绝不回退到
//! 工作区全局 diff（那正是串会话的来源）。

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

use crate::recovery::ChangedFile;

/// 同一秒内的边界误差：任务开始时间取在快照/工具执行之前，允许 2 秒回看。
const MTIME_SLACK: Duration = Duration::from_secs(2);
/// 单个会话最多记住的路径数（去重后的上限）。正常对话几十到几百条，够用且不撑爆文件。
const MAX_PATHS: usize = 5000;

/// 一个会话的产物账本。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Ledger {
    #[serde(default)]
    pub session: String,
    /// 记账时的工作区（用于切工作区后作废旧账本）
    #[serde(default)]
    pub workspace: String,
    /// 工作区相对路径（正斜杠），按首次记录顺序去重
    #[serde(default)]
    pub paths: Vec<String>,
    /// 是否由旧会话检查点回溯写入（前端据此提示「历史记录可能不全」）
    #[serde(default)]
    pub backfilled: bool,
    /// 最近一次记账时间（unix 毫秒）
    #[serde(default)]
    pub updated_ms: u64,
}

/// 读-改-写串行锁：同一会话的并发任务不常见，但定时任务/子代理可能撞上。
static LEDGER_LOCK: Mutex<()> = Mutex::new(());

/// 会话 id 只允许安全字符（与 session::safe_id 同规，直接拼文件名防穿越）。
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// 工作区路径归一化：Windows 不区分大小写与分隔符，去掉尾部分隔符再比。
fn norm_workspace(p: &str) -> String {
    p.replace('/', "\\").trim_end_matches('\\').to_lowercase()
}

/// 两个工作区是否同一个（大小写/分隔符无关）。
pub fn same_workspace(a: &str, b: &Path) -> bool {
    !a.trim().is_empty() && norm_workspace(a) == norm_workspace(&b.display().to_string())
}

/// 账本目录：`<数据目录>/logs/artifacts/`。
fn ledger_dir(logs_root: &Path) -> PathBuf {
    logs_root.join("artifacts")
}

fn ledger_path(logs_root: &Path, session: &str) -> Option<PathBuf> {
    safe_id(session).then(|| ledger_dir(logs_root).join(format!("{session}.json")))
}

/// 单个路径必须是工作区相对、无穿越、无盘符的普通文件路径。
pub(crate) fn safe_rel_path(p: &str) -> bool {
    let p = p.trim();
    if p.is_empty() || p.len() > 4096 {
        return false;
    }
    if p.starts_with('/') || p.starts_with('\\') || p.contains(':') || p.contains('\0') {
        return false;
    }
    !p.split(['/', '\\']).any(|seg| seg == "..")
}

/// 依赖与缓存目录/文件不算交付产物：装一次 venv / npm install 会把几千个第三方
/// 文件全部落进任务 mtime 时间窗，账本被刷成依赖清单（2026-11-14 实锤：抖音工作区
/// 装包后面板 2164 条里几乎全是 .venv/Lib/site-packages，且经检查点回溯路径涌入）。
/// 按路径段匹配，出现在任何层级都算噪声；段名单取惯例 .gitignore 口径里
/// 「装出来/编出来」的那类，不收 dist/build/target 这类可能是真产物的名字。
const NOISE_SEGMENTS: &[&str] = &[
    ".venv", "venv", "site-packages", "node_modules", "__pycache__", ".git", ".tox",
    ".pytest_cache", ".mypy_cache", ".ruff_cache", ".gradle",
];

/// 依赖/缓存路径判定：命中噪声目录段（任何层级）或编译缓存文件后缀。
pub(crate) fn is_noise_path(p: &str) -> bool {
    let lower = p.to_ascii_lowercase();
    for seg in lower.split(['/', '\\']) {
        if NOISE_SEGMENTS.contains(&seg) {
            return true;
        }
        // 工具链安装目录（SDK/JDK/cmdline-tools…）：装一次上万文件，既不是交付产物，
        // 也不该占账本上限（2026-11-21 实机：1.1 万 SDK 文件把 87 个源码挤出账本）
        if crate::recovery::is_toolchain_dir(seg) {
            return true;
        }
        // Python 包装元数据目录（xxx-1.0.dist-info / xxx.egg-info）
        if seg.ends_with(".dist-info") || seg.ends_with(".egg-info") {
            return true;
        }
    }
    lower.ends_with(".pyc") || lower.ends_with(".pyo")
}

/// 把「相对基线的改动清单」过滤成「本轮任务真正改到的文件」：
/// - 删除态不记（面板里点开已不存在的文件没有意义）；
/// - 只认 mtime 落在任务时间窗内的文件——基线可能是一天前的旧快照，
///   落在窗口外的条目是别的会话/用户早先弄的，不能算本轮产物；
/// - `.agent/`（agent 临时脚本/诊断区）不算交付产物。
pub fn paths_from_changed(files: &[ChangedFile], ws: &Path, since: SystemTime) -> Vec<String> {
    let since = since.checked_sub(MTIME_SLACK).unwrap_or(since);
    let mut out: Vec<String> = Vec::new();
    for f in files {
        if f.status == 'D' {
            continue;
        }
        let raw = f.path.replace('\\', "/");
        let raw = raw.trim_start_matches("./").to_string();
        if !safe_rel_path(&raw) || raw.to_lowercase().starts_with(".agent/") || is_noise_path(&raw) {
            continue;
        }
        let Ok(meta) = std::fs::metadata(ws.join(&raw)) else { continue };
        if !meta.is_file() {
            continue;
        }
        // 读不到 mtime（权限/竞态）时不猜，宁可不记
        let Ok(mtime) = meta.modified() else { continue };
        if mtime >= since {
            out.push(raw);
        }
    }
    out.sort();
    out.dedup();
    out
}

/// 读账本（不存在/损坏一律 None，记账是派生数据，坏了重建即可）。
pub fn read_in(logs_root: &Path, session: &str) -> Option<Ledger> {
    let path = ledger_path(logs_root, session)?;
    let text = std::fs::read_to_string(path).ok()?;
    let mut ledger: Ledger = serde_json::from_str(&text).ok()?;
    // 旧文件缺 session 字段时以文件名为准
    if ledger.session.is_empty() {
        ledger.session = session.to_string();
    }
    Some(ledger)
}

/// 把本轮改动并入会话账本。即使本轮零改动也会落一个空账本 —— 前端据此区分
/// 「已开始记账、本轮确实没产物」与「旧会话没有归属记录」，不再拿全局 diff 充数。
pub fn record_in(logs_root: &Path, session: &str, ws: &Path, paths: Vec<String>) -> anyhow::Result<()> {
    merge(logs_root, session, ws, paths, false)
}

/// 旧会话回溯：把从检查点链算出来的历史路径并进账本，并打上 backfilled 标记。
/// 幂等：已经回溯过就直接返回，后续任务只做增量记账。
pub fn record_backfill_in(logs_root: &Path, session: &str, ws: &Path, paths: Vec<String>) -> anyhow::Result<()> {
    merge(logs_root, session, ws, paths, true)
}

fn merge(logs_root: &Path, session: &str, ws: &Path, paths: Vec<String>, backfilled: bool) -> anyhow::Result<()> {
    let Some(path) = ledger_path(logs_root, session) else {
        anyhow::bail!("非法 session id");
    };
    let _g = LEDGER_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut ledger = read_in(logs_root, session).unwrap_or_default();
    if backfilled && ledger.backfilled {
        return Ok(());
    }
    // 工作区变了：旧账本属于别的项目，不能继续挂在这个会话名下
    if !same_workspace(&ledger.workspace, ws) {
        ledger = Ledger {
            session: session.to_string(),
            workspace: ws.display().to_string(),
            paths: Vec::new(),
            backfilled: false,
            updated_ms: 0,
        };
    }
    let mut added: Vec<String> = Vec::new();
    for p in paths {
        let p = p.replace('\\', "/");
        // 新增路径过噪声过滤：正常记账走 paths_from_changed 已滤过，检查点回溯
        // （session_changed_paths）不经过它，必须在这里再滤一道（11-14 venv 洪水
        // 就是走回溯路径涌入的）
        if safe_rel_path(&p) && !is_noise_path(&p) && !ledger.paths.iter().any(|x| x == &p) {
            ledger.paths.push(p.clone());
            added.push(p);
        }
    }
    // 存量自愈：老账本里已经躺着的噪声路径，趁重写一次性清掉
    let before = ledger.paths.len();
    ledger.paths.retain(|p| !is_noise_path(p));
    if ledger.paths.len() != before {
        crate::observability::try_event(
            crate::observability::EventType::State,
            serde_json::json!({ "kind": "artifacts.noise_dropped", "session": session, "count": before - ledger.paths.len() }),
        );
    }
    // 上限：正常增量记账按 LRU（丢最老的）；但**一次大批量新增**不能把账本里已有的
    // 真产物整批挤掉。2026-11-21 实锤：一次任务装 Android SDK，1.1 万个工具链文件
    // 一次 merge 进来，drain(0..) 把前面记下的 87 个 app 源码全冲走了。
    // 大批次（超过容量一半）= 大概率是安装/解压，从这批自己的尾部裁，旧条目不动；
    // 小批次照旧 LRU，保证正常产物能进账本。
    if ledger.paths.len() > MAX_PATHS {
        let overflow = ledger.paths.len() - MAX_PATHS;
        let added_set: std::collections::HashSet<&str> = added.iter().map(String::as_str).collect();
        let first_new = ledger
            .paths
            .iter()
            .position(|p| added_set.contains(p.as_str()))
            .unwrap_or(ledger.paths.len());
        let new_count = ledger.paths.len() - first_new;
        if new_count > MAX_PATHS / 2 {
            let drop_new = overflow.min(new_count);
            let len = ledger.paths.len();
            ledger.paths.drain(len - drop_new..len);
            let still = ledger.paths.len().saturating_sub(MAX_PATHS);
            if still > 0 {
                ledger.paths.drain(0..still);
            }
        } else {
            ledger.paths.drain(0..overflow);
        }
    }
    if backfilled {
        ledger.backfilled = true;
    }
    ledger.updated_ms = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    std::fs::create_dir_all(ledger_dir(logs_root))?;
    let text = serde_json::to_string(&ledger)?;
    crate::fsx::write_atomic_str(&path, &text)?;
    Ok(())
}

/// 删除会话时清掉账本（会话文件都没了，产物归属也没有意义）。
pub fn clear_in(logs_root: &Path, session: &str) {
    if let Some(path) = ledger_path(logs_root, session) {
        let _ = std::fs::remove_file(path);
    }
}

/// 当前工作区里仍然存在的产物路径（账本里删掉/挪走的条目不显示）。
/// 依赖/缓存噪声在这里也滤一道：老账本在首次 merge 自愈前就能显示干净，
/// 不用等下一次记账（11-14 实锤的存量 2164 条 .venv 路径）。
pub fn existing_paths(ledger: &Ledger, ws: &Path) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for p in &ledger.paths {
        let p = p.replace('\\', "/");
        if safe_rel_path(&p) && !is_noise_path(&p) && ws.join(&p).is_file() && !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    }
    out
}

/// 运行时入口：数据目录下的 logs 根。
fn runtime_logs_root() -> PathBuf {
    crate::config::data_root().join("logs")
}

pub fn read(session: &str) -> Option<Ledger> {
    read_in(&runtime_logs_root(), session)
}

pub fn record(session: &str, ws: &Path, paths: Vec<String>) -> anyhow::Result<()> {
    record_in(&runtime_logs_root(), session, ws, paths)
}

pub fn record_backfill(session: &str, ws: &Path, paths: Vec<String>) -> anyhow::Result<()> {
    record_backfill_in(&runtime_logs_root(), session, ws, paths)
}


#[cfg(test)]
mod tests {
    use super::*;

    fn tmproot(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("agent-artifacts-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn plain_ws(tag: &str) -> PathBuf {
        let d = tmproot(tag);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn ledger_roundtrip_dedupes_and_marks_tracked() {
        let root = tmproot("roundtrip");
        let ws = plain_ws("roundtrip-ws");
        std::fs::write(ws.join("a.txt"), "a").unwrap();
        std::fs::write(ws.join("b.txt"), "b").unwrap();

        record_in(&root, "sess-1", &ws, vec!["a.txt".into(), "a.txt".into(), "b.txt".into()]).unwrap();
        let l = read_in(&root, "sess-1").expect("账本应存在");
        assert_eq!(l.paths, vec!["a.txt".to_string(), "b.txt".to_string()]);
        assert!(same_workspace(&l.workspace, &ws));

        // 零改动也落账本（tracked 标记），路径不变
        record_in(&root, "sess-1", &ws, vec![]).unwrap();
        let l = read_in(&root, "sess-1").unwrap();
        assert_eq!(l.paths.len(), 2);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn workspace_change_resets_ledger() {
        let root = tmproot("wsmove");
        let ws1 = plain_ws("wsmove-1");
        let ws2 = plain_ws("wsmove-2");
        std::fs::write(ws1.join("old.txt"), "x").unwrap();
        std::fs::write(ws2.join("new.txt"), "x").unwrap();
        record_in(&root, "sess-2", &ws1, vec!["old.txt".into()]).unwrap();
        record_in(&root, "sess-2", &ws2, vec!["new.txt".into()]).unwrap();
        let l = read_in(&root, "sess-2").unwrap();
        assert_eq!(l.paths, vec!["new.txt".to_string()], "切工作区后旧路径不能跟着走");
        assert!(same_workspace(&l.workspace, &ws2));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&ws1);
        let _ = std::fs::remove_dir_all(&ws2);
    }

    #[test]
    fn invalid_ids_and_paths_are_rejected() {
        let root = tmproot("invalid");
        assert!(ledger_path(&root, "../settings").is_none());
        assert!(ledger_path(&root, "").is_none());
        for bad in ["../x", "C:/x", "/abs", "a/../../b", ""] {
            assert!(!safe_rel_path(bad), "{bad} 不该通过");
        }
        for good in ["a.txt", "dir/b.png", "带空格 的文件.md"] {
            assert!(safe_rel_path(good), "{good} 应通过");
        }
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn changed_paths_respect_task_time_window() {
        let ws = plain_ws("window");
        std::fs::write(ws.join("fresh.txt"), "new").unwrap();
        std::fs::create_dir_all(ws.join(".agent")).unwrap();
        std::fs::write(ws.join(".agent").join("tmp.js"), "x").unwrap();
        // 依赖噪声：venv 深处的第三方文件（2026-11-14 实锤的洪水形态）
        std::fs::create_dir_all(ws.join(".venv/Lib/site-packages/PIL/__pycache__")).unwrap();
        std::fs::write(ws.join(".venv/Lib/site-packages/PIL/__pycache__/Image.cpython-313.pyc"), "x").unwrap();
        let mk = |path: &str, status: char| ChangedFile {
            path: path.to_string(),
            status,
            adds: 1,
            dels: 0,
            tracked: true,
            base: None,
        };
        let files = vec![
            mk("fresh.txt", 'M'),
            mk("gone.txt", 'D'),
            mk(".agent/tmp.js", 'M'),
            mk("../escape.txt", 'M'),
            mk(".venv/Lib/site-packages/PIL/__pycache__/Image.cpython-313.pyc", 'A'),
        ];
        // 任务窗口取在过去一小时：fresh.txt 的 mtime 在窗内
        let since = SystemTime::now() - Duration::from_secs(3600);
        assert_eq!(paths_from_changed(&files, &ws, since), vec!["fresh.txt".to_string()]);
        // 任务窗口取在将来：什么都不算本轮产物（模拟基线里混进了旧改动）
        let future = SystemTime::now() + Duration::from_secs(3600);
        assert!(paths_from_changed(&files, &ws, future).is_empty());
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn noise_paths_are_filtered_on_read_and_merge() {
        assert!(is_noise_path(".venv/Lib/site-packages/PIL/Image.py"));
        assert!(is_noise_path("web/node_modules/.bin/x.js"));
        assert!(is_noise_path("a/b/__pycache__/m.cpython-313.pyc"));
        assert!(is_noise_path("pkg/deepseek-1.2.dist-info/METADATA"));
        assert!(is_noise_path("libs/thing.egg-info/PKG-INFO"));
        // 工具链安装目录（2026-11-21）：SDK/JDK 落进工作区时不能进账本
        assert!(is_noise_path("tools/sdk/platforms/android-34/data/res/x.png"));
        assert!(is_noise_path("tools/jdk-17.0.21/bin/java.exe"));
        assert!(is_noise_path("cmdline-tools/latest/bin/sdkmanager.bat"));
        assert!(!is_noise_path("运营方案.md"));
        assert!(!is_noise_path("src/main.rs"), "正常源码不能误伤");

        let root = tmproot("noise");
        let ws = plain_ws("noise-ws");
        std::fs::write(ws.join("real.txt"), "x").unwrap();
        std::fs::create_dir_all(ws.join(".venv/Lib/site-packages/PIL")).unwrap();
        std::fs::write(ws.join(".venv/Lib/site-packages/PIL/Image.py"), "x").unwrap();
        // 模拟老账本里已躺着的垃圾（直接 record 不经过 capture 过滤）
        record_in(&root, "sess-noise", &ws, vec![".venv/Lib/site-packages/PIL/Image.py".into()]).unwrap();
        // 下一次记账把存量噪声一并清掉，且新增噪声进不来
        record_in(&root, "sess-noise", &ws, vec!["real.txt".into(), "node_modules/x.js".into()]).unwrap();
        let l = read_in(&root, "sess-noise").unwrap();
        assert_eq!(l.paths, vec!["real.txt".to_string()], "存量与新增噪声都该被清掉");

        // 读取侧过滤：老账本没来得及 merge 自愈也显示干净
        let l2 = Ledger {
            session: "s".into(),
            workspace: ws.display().to_string(),
            paths: vec!["real.txt".into(), ".venv/Lib/site-packages/PIL/Image.py".into()],
            backfilled: true,
            updated_ms: 0,
        };
        assert_eq!(existing_paths(&l2, &ws), vec!["real.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn existing_paths_drops_deleted_entries() {
        let ws = plain_ws("existing");
        std::fs::write(ws.join("kept.txt"), "x").unwrap();
        let l = Ledger {
            session: "s".into(),
            workspace: ws.display().to_string(),
            paths: vec!["kept.txt".into(), "deleted.txt".into()],
            backfilled: false,
            updated_ms: 0,
        };
        assert_eq!(existing_paths(&l, &ws), vec!["kept.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&ws);
    }

    #[test]
    fn clear_removes_ledger() {
        let root = tmproot("clear");
        let ws = plain_ws("clear-ws");
        record_in(&root, "sess-3", &ws, vec!["a.txt".into()]).unwrap();
        assert!(read_in(&root, "sess-3").is_some());
        clear_in(&root, "sess-3");
        assert!(read_in(&root, "sess-3").is_none());
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 大批量新增（安装/解压洪水）不能把已有真产物挤掉（2026-11-21 真机实锤：
    /// 1.1 万个 SDK 文件把 87 个 app 源码冲出了账本）。
    #[test]
    fn huge_batch_does_not_evict_existing_artifacts() {
        let root = tmproot("cap-huge");
        let ws = plain_ws("cap-huge-ws");
        let real: Vec<String> = (0..80).map(|i| format!("android/src/M{i}.java")).collect();
        record_in(&root, "sess-cap", &ws, real.clone()).unwrap();
        // 用非噪声路径模拟「一次大批量新增」——工具链目录现在会被 is_noise_path 直接
        // 滤掉，这里测的是上限裁剪策略本身。
        let flood: Vec<String> = (0..6000).map(|i| format!("vendor/pkg/f{i}.dat")).collect();
        record_in(&root, "sess-cap", &ws, flood).unwrap();
        let l = read_in(&root, "sess-cap").unwrap();
        assert_eq!(l.paths.len(), MAX_PATHS);
        for p in &real {
            assert!(l.paths.iter().any(|x| x == p), "真产物被挤掉了: {p}");
        }
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&ws);
    }

    /// 正常小批量仍按 LRU 淘汰最老的（上限语义不变）。
    #[test]
    fn small_batch_still_lru() {
        let root = tmproot("cap-small");
        let ws = plain_ws("cap-small-ws");
        let old: Vec<String> = (0..MAX_PATHS).map(|i| format!("old/f{i}.txt")).collect();
        record_in(&root, "sess-lru", &ws, old).unwrap();
        record_in(&root, "sess-lru", &ws, vec!["new/a.txt".into()]).unwrap();
        let l = read_in(&root, "sess-lru").unwrap();
        assert_eq!(l.paths.len(), MAX_PATHS);
        assert!(l.paths.iter().any(|p| p == "new/a.txt"), "新条目照常进入");
        assert!(!l.paths.iter().any(|p| p == "old/f0.txt"), "最老的被淘汰");
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&ws);
    }
}
