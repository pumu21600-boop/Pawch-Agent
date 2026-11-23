//! 错误恢复 —— 总架构 §14 /
//!
//! 「假设一定会崩，把能恢复做成默认」：
//! - checkpoint：把工作区文件提交进影子仓库 `<workspace>/.agent/shadow`
//!   （独立 git dir，work-tree 指向 workspace）。git 工作区只跟踪真仓库 `git ls-files`
//!   的文件；非 git 工作区整目录扫描兜底（2026-10-02 产物面板实测后加），均套排除规则，
//!   防 IO 爆炸；失败只记日志，绝不阻断工具执行。
//! - restore：影子仓库 `reset --hard` 回指定检查点（含删除其后新建的已跟踪文件）；
//!   shell 副作用不可回滚，靠沙箱兜底。
//!
//! HANDOFF 交接文档已于 2026-10-01 移除，跨任务续命靠会话落盘 + 滚动摘要。
//!
//! 所有 git 调用都是阻塞的，调用方（核心循环/server）包 spawn_blocking。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, anyhow, bail};
use chrono::{SecondsFormat, Utc};

/// 单个检查点（id 为影子仓库完整 commit 哈希）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct Checkpoint {
    pub id: String,
    pub ts: String,
    pub reason: String,
    /// 拍点时所属会话 id（旧检查点无此信息，缺省空串）。
    /// 有了归属，切回某条会话时才能只显示它自己的改动、只回滚它自己的基线。
    #[serde(default)]
    pub session: String,
}

/// 真仓库 origin remote url（无 remote/非仓库返回 None）。记忆系统项目身份用。
pub fn remote_url(ws: &Path) -> Option<String> {
    let bin = git_bin().ok()?;
    git_ok(&bin, None, ws, &["remote", "get-url", "origin"]).ok().filter(|s| !s.is_empty())
}

/// 真仓库根目录（非 git 仓库返回 None）。
pub fn root_path(ws: &Path) -> Option<PathBuf> {
    let bin = git_bin().ok()?;
    let out = git_ok(&bin, None, ws, &["rev-parse", "--show-toplevel"]).ok()?;
    if out.is_empty() { None } else { Some(PathBuf::from(out)) }
}

/// 影子仓库 git dir（workspace/.agent/shadow/.git）。
fn shadow_gitdir(ws: &Path) -> PathBuf {
    ws.join(".agent").join("shadow").join(".git")
}

/// git 面板的数据源（scope 选择器：未暂存 / 上一轮…）。
/// - `Shadow`：影子仓库相对某个检查点 = 「本任务改动」（默认，产物/git 一直用这条）
/// - `Workspace`：工作区自己的真仓库相对 HEAD = 「未提交改动」
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoOrigin {
    Shadow,
    Workspace,
    /// 「全部分支更改」（ZCode 同款，源码核实 packages/services/src/git/repo/gitCliRepo.ts）：
    /// 基线 = 当前分支上游（tracking branch），`git diff <上游>...HEAD` —— 只含已提交到
    /// 本地分支的改动，不含未提交工作树、不含未跟踪文件；没有上游分支 = 来源不可用。
    Branch,
}

impl RepoOrigin {
    /// 查询串 → 枚举（认 "workspace"/"work"/"branch"，其余一律影子仓库，保持旧行为）。
    pub fn from_query(s: Option<&str>) -> Self {
        match s.map(|x| x.trim()) {
            Some("workspace") | Some("work") => Self::Workspace,
            Some("branch") => Self::Branch,
            _ => Self::Shadow,
        }
    }

    /// 对应的 git dir。None = 让 git 从工作区自己发现真仓库（与 `status()` 同一条路）。
    fn gitdir(&self, ws: &Path) -> Option<PathBuf> {
        match self {
            Self::Shadow => Some(shadow_gitdir(ws)),
            Self::Workspace | Self::Branch => None,
        }
    }

    /// 基线形态校验。真仓库这一档额外放行 "HEAD"（工作树自身的最新提交）；
    /// branch 档的基线（"<上游>...HEAD"）由 branch_base 在服务端自己算，不再校验。
    fn check_baseline(&self, baseline: &str) -> anyhow::Result<()> {
        if *self == Self::Branch {
            return Ok(());
        }
        let bare = baseline.trim();
        if bare.is_empty() {
            bail!("缺少 baseline");
        }
        if *self == Self::Workspace && bare.eq_ignore_ascii_case("HEAD") {
            return Ok(());
        }
        if !bare.chars().all(|c| c.is_ascii_hexdigit()) || !(7..=40).contains(&bare.len()) {
            bail!("非法基线 id: {bare}");
        }
        Ok(())
    }
}

/// 「全部分支更改」的基线：`<当前分支上游>...HEAD`（ZCode 同款，三点 = merge-base 起）。
/// 上游从 `status -b` 的 `## main...origin/main` 头里解析（与 gitInfo 同一来源）；
/// 没设上游 = None（前端显示「来源不可用」，与 ZCode 行为一致）。
pub fn branch_base(ws: &Path) -> Option<String> {
    let st = status(ws);
    if st.upstream.trim().is_empty() {
        return None;
    }
    Some(format!("{}...HEAD", st.upstream.trim()))
}

/// 排除目录：影子仓库永不跟踪（真仓库 ls-files 之外的第二道防线，防 IO 爆炸）。
/// .attachments 是粘贴媒体的中转目录（turn.rs 会把它写进 .gitignore），同样不进影子仓库。
/// 排除目录：影子仓库永不跟踪（真仓库 ls-files 之外的第二道防线，防 IO 爆炸）。
/// .attachments 是粘贴媒体的中转目录（turn.rs 会把它写进 .gitignore），同样不进影子仓库。
/// "build" 是 gradle/cmake/make 的通用构建输出名（与 target/dist 同类）——android/build
/// 的 .class 编译产物曾刷满清单且无行数可算（2026-11-22 用户实锤）。
const EXCLUDED_DIRS: &[&str] = &["node_modules", "target", "dist", "build", "logs", ".agent", ".git", ".attachments"];

/// 工具链安装目录（2026-11-21 实机）：装一次 Android SDK/JDK 会落上万文件，既不是
/// 交付产物、也不该出现在 git 面板/产物账本里——那次 1.1 万工具链文件把 87 个 app
/// 源码冲出了账本、把清单刷成 11k 行。按目录名判定（任意层级、大小写不敏感）。
/// 刻意不收 `platforms` 这类通用名（容易误伤真项目）；SDK 根按 `sdk` 整枝剪掉后，
/// 子目录不会再被遍历到。git 侧对应 `TOOLCHAIN_EXCLUDE_GLOBS`。
pub(crate) fn is_toolchain_dir(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "sdk" | "android-sdk" | "cmdline-tools" | "system-images" | "platform-tools" | "build-tools" | "emulator"
    ) || n == "jdk"
        || n == "jre"
        || n.starts_with("jdk-")
        || n.starts_with("jre-")
}

/// gitignore 风格的排除 glob（给 `git ls-files --exclude=` 用，省掉上万文件的枚举）。
const TOOLCHAIN_EXCLUDE_GLOBS: &[&str] = &[
    "sdk",
    "android-sdk",
    "cmdline-tools",
    "system-images",
    "platform-tools",
    "build-tools",
    "emulator",
    "jdk",
    "jdk-*",
    "jre",
    "jre-*",
];

fn is_excluded_dir(name: &str) -> bool {
    EXCLUDED_DIRS.contains(&name) || is_toolchain_dir(name)
}
/// 单文件体积上限，超过直接跳过（大二进制不走影子仓库）。
const MAX_FILE_BYTES: u64 = 10 * 1024 * 1024;

fn git_bin() -> anyhow::Result<PathBuf> {
    crate::tools::git_exe().map_err(|e| anyhow!(e))
}

fn git(bin: &Path, gitdir: Option<&Path>, ws: &Path, args: &[&str]) -> anyhow::Result<std::process::Output> {
    git_spawn(bin, gitdir, ws, args, Stdio::null())?
        .wait_with_output()
        .context("等待 git 失败")
}

fn git_ok(bin: &Path, gitdir: Option<&Path>, ws: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = git(bin, gitdir, ws, args)?;
    if !out.status.success() {
        return Err(anyhow!(
            "git {} 失败: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 带 stdin 输入的 git 调用（check-ignore --stdin 批量查忽略路径用）。
fn git_stdin(
    bin: &Path,
    gitdir: Option<&Path>,
    ws: &Path,
    args: &[&str],
    input: &[u8],
) -> anyhow::Result<std::process::Output> {
    use std::io::Write;
    let mut child = git_spawn(bin, gitdir, ws, args, Stdio::piped())?;
    {
        let mut sin = child.stdin.take().context("git stdin 打不开")?;
        sin.write_all(input)?;
    }
    child.wait_with_output().context("等待 git 失败")
}

fn git_spawn(
    bin: &Path,
    gitdir: Option<&Path>,
    ws: &Path,
    args: &[&str],
    stdin: Stdio,
) -> anyhow::Result<std::process::Child> {
    use std::os::windows::process::CommandExt;
    let mut cmd = Command::new(bin);
    // 后台无窗口运行时，控制台子进程会闪黑框：一律不创建新控制台窗口
    cmd.creation_flags(0x0800_0000);
    // 2026-10-08 修：CJK 路径默认被 quotepath 转义（中文.txt → \346\226\207.txt），
    // 解析 diff/status 输出时按转义串拼路径必然找不到文件（本机 ~/.gitconfig 恰好
    // 设了 core.quotepath=false 会掩盖，换机器/换用户即复现）。所有 git 调用统一关掉。
    cmd.arg("-c").arg("core.quotepath=false");
    match gitdir {
        Some(g) => {
            cmd.arg("--git-dir").arg(g);
            cmd.arg("--work-tree").arg(ws);
        }
        None => {
            cmd.arg("-C").arg(ws);
        }
    }
    cmd.args(args)
        .current_dir(ws)
        .stdin(stdin)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_OBJECT_DIRECTORY", "GIT_COMMON_DIR"] {
        cmd.env_remove(k);
    }
    cmd.spawn().context("启动 git 失败")
}

/// 确保影子仓库就绪（幂等）：git init + 本地 config（user/autocrlf/gpgsign）。
fn ensure_shadow(bin: &Path, ws: &Path) -> anyhow::Result<PathBuf> {
    let gitdir = shadow_gitdir(ws);
    if !gitdir.exists() {
        let shadow_root = ws.join(".agent").join("shadow");
        std::fs::create_dir_all(&shadow_root).with_context(|| format!("建影子仓库目录失败: {}", shadow_root.display()))?;
        git_ok(bin, None, &shadow_root, &["init", "--quiet"])?;
        for cfg in [
            ["user.name", "agent-checkpoint"],
            ["user.email", "agent@local"],
            ["core.autocrlf", "false"],
            ["commit.gpgsign", "false"],
        ] {
            git_ok(bin, Some(&gitdir), ws, &["config", cfg[0], cfg[1]])?;
        }
    }
    Ok(gitdir)
}

/// 工作区本身是不是 git 仓库（非 git 走整目录扫描，另有一套语义）。
fn is_git_workspace(bin: &Path, ws: &Path) -> bool {
    let probe = git(bin, None, ws, &["rev-parse", "--is-inside-work-tree"]);
    matches!(&probe, Ok(out) if out.status.success() && String::from_utf8_lossy(&out.stdout).trim() == "true")
}

/// 影子仓库要跟踪的文件清单。真仓库是 git 仓库时沿用它的 ls-files（老语义：
/// untracked 文件不属于影子仓库，restore 不动它）；工作区**不是 git 仓库**时
/// git ls-files 无从谈起，退化为目录扫描（排除目录 + 体积上限）——产物/检查点
/// 的真机实测：普通文件夹（Desktop\th 这类）不是 git 仓库，检查点永远拍不出来，
/// 「产物」面板就没有基线可看。两种来源都套同一套排除与体积规则。
fn tracked_files(bin: &Path, ws: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if !is_git_workspace(bin, ws) {
        return walk_files(ws);
    }
    let out = git(bin, None, ws, &["ls-files", "-z"])?;
    if !out.status.success() {
        bail!("ls-files 失败: {}", String::from_utf8_lossy(&out.stderr).trim());
    }
    let mut files = Vec::new();
    for entry in out.stdout.split(|b| *b == 0) {
        if entry.is_empty() {
            continue;
        }
        let rel = PathBuf::from(String::from_utf8_lossy(entry).replace('\\', "/"));
        let excluded = rel.components().any(|c| {
            matches!(c, std::path::Component::Normal(_))
                && is_excluded_dir(&c.as_os_str().to_string_lossy())
        });
        if excluded {
            continue;
        }
        let full = ws.join(&rel);
        // 工作区里已不存在（用户删掉/挪走）：**必须保留**进 pathspec——影子仓库要如实记
        // 删除。此前在这里被跳过，删除永远进不了基线，鬼影文件一代代往后传
        // （2026-10-31 真机实测「产物不是当前工作区的」根因）。
        let Ok(meta) = std::fs::metadata(&full) else {
            files.push(rel);
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            continue;
        }
        files.push(rel);
    }
    Ok(files)
}

/// 非 git 工作区的兜底清单：整棵扫（软链接不进，防环；单文件 >10MB 跳过；
/// 排除目录整枝剪掉）。文件数硬上限 20000——普通项目目录远够，失控目录不拖死检查点。
fn walk_files(ws: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        if files.len() >= 20_000 {
            bail!("工作区文件数超过 20000，影子仓库拒收（用 git 仓库或清理目录后重试）");
        }
        let full = ws.join(&rel);
        let Ok(rd) = std::fs::read_dir(&full) else { continue };
        for entry in rd.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let child = rel.join(&*name);
            let Ok(meta) = entry.metadata() else { continue };
            if meta.is_symlink() {
                continue;
            }
            if meta.is_dir() {
                if is_excluded_dir(&name) || name.starts_with('.') {
                    continue;
                }
                stack.push(child);
            } else if meta.is_file() && meta.len() <= MAX_FILE_BYTES {
                files.push(child);
            }
        }
    }
    files.sort();
    Ok(files)
}

/// 未跟踪文件清单（工作区相对路径、`/` 分隔）。
/// `--directory` 折叠的大目录会展开成逐文件——产物面板/任务快照需要逐文件粒度。
/// changed_files 与「任务起点未跟踪快照」共用这一份口径，两边集合必须一致。
fn list_untracked(bin: &Path, gitdir: Option<&Path>, ws: &Path) -> anyhow::Result<Vec<String>> {
    let mut others_cmd = vec![
        "ls-files".to_string(),
        "--others".to_string(),
        "-z".to_string(),
        "--directory".to_string(),
        "--exclude-standard".to_string(),
    ];
    for dir in EXCLUDED_DIRS {
        others_cmd.push(format!("--exclude={dir}"));
    }
    // 工具链安装目录整枝排除（gitignore glob 语义）；不排除的话 `--directory` 只回一条
    // 折叠目录，后面还得逐文件展开上万次。
    for pat in TOOLCHAIN_EXCLUDE_GLOBS {
        others_cmd.push(format!("--exclude={pat}"));
    }
    let others = git_ok(bin, gitdir, ws, &others_cmd.iter().map(String::as_str).collect::<Vec<_>>())?;
    let mut out: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut collapsed: Vec<String> = Vec::new();
    let mut push_file = |rel: &str| {
        if rel.is_empty() || out.len() >= 20_000 {
            return;
        }
        let path = PathBuf::from(rel);
        let excluded = path.components().any(|c| {
            matches!(c, std::path::Component::Normal(_))
                && is_excluded_dir(&c.as_os_str().to_string_lossy())
        });
        if excluded {
            return;
        }
        let Ok(meta) = std::fs::metadata(ws.join(&path)) else { return };
        if !meta.is_file() || meta.len() > MAX_FILE_BYTES {
            return;
        }
        let rel = rel.replace('\\', "/");
        if seen.insert(rel.clone()) {
            out.push(rel);
        }
    };
    for entry in others.split('\0') {
        if entry.is_empty() {
            continue;
        }
        let is_dir_entry = entry.ends_with('/') || entry.ends_with('\\');
        let rel = entry.trim_end_matches(['/', '\\']);
        if rel.is_empty() {
            continue;
        }
        if is_dir_entry {
            let path = PathBuf::from(rel);
            let excluded = path.components().any(|c| {
                matches!(c, std::path::Component::Normal(_))
                    && is_excluded_dir(&c.as_os_str().to_string_lossy())
            });
            if !excluded {
                collapsed.push(rel.replace('\\', "/"));
            }
            continue;
        }
        push_file(rel);
    }
    for dir in collapsed {
        let pattern = format!("{dir}/");
        let Ok(expanded) = git_ok(
            bin,
            gitdir,
            ws,
            &["ls-files", "--others", "-z", "--exclude-standard", "--", &pattern],
        ) else {
            continue;
        };
        for entry in expanded.split('\0') {
            if !entry.is_empty() {
                push_file(entry.trim_end_matches('/'));
            }
        }
    }
    Ok(out)
}

/// 影子仓库操作串行锁（进程级，2026-10-08 修）：影子仓库单个 index 没有任何跨任务
/// 互斥——两个会话同时拍检查点时，B 的 `git add` 可能落在 A 的 add 与 commit 之间，
/// A 的提交吞掉 B 的文件且只带 A 的 Agent-Session trailer，B 随后 `diff --cached`
/// 干净 → 返 None（B 丢检查点，改动条/撤销的会话归属失真）；另一种交错是 B 的 add
/// 撞 index.lock 直接失败。restore 同样碰这个 index，一并排大队。
/// 频率低（任务起点/结束/高危工具前各一次），全局锁的代价可忽略。
static SHADOW_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// 拍检查点：add + 有变更才 commit。返回 None 表示没有需要落的变化。
/// 任何失败都由调用方决定降级（核心循环只记日志不阻断）。
/// `session_id` 写进提交正文的 trailer，供改动条/撤销按会话对齐。
/// 跟踪范围：git 工作区沿用真仓库 ls-files；非 git 工作区整目录扫（见 tracked_files）。
pub fn checkpoint(ws: &Path, reason: &str, session_id: &str) -> anyhow::Result<Option<Checkpoint>> {
    let _serial = SHADOW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bin = git_bin()?;
    let gitdir = ensure_shadow(&bin, ws)?;
    let git_ws = is_git_workspace(&bin, ws);
    let task_start = reason.trim() == "任务起点";
    // 全量重装拍：任务起点 / 任务结束快照 / 手动重拍基线——这三类是「基线时刻」，
    // 影子树必须精确等于那一刻的过滤工作区（见下面的纯函数快照说明）
    let full_refresh = task_start
        || reason.trim() == "任务结束快照"
        || reason.trim() == "手动重拍基线";

    // 纯函数快照（2026-11-23 用户拍板「治本」）：影子 index 是跨任务存活的活账本，
    // 规则一变历史残留就变鬼影——_App.cpp 这类影子有现实无的旧文件、被 .gitignore
    // 挂名的旧截图让 add 整批拒、任务间删除漏记……连修三次全是「漂移」这一类。
    // 改成：全量拍先把 index 清空，再把「当前过滤后的工作区」整个装进去提交——
    // 影子仓库的每棵树永远精确等于那一刻的过滤工作区，跨拍漂移整类消失：
    // ignore 变卦的文件自动退出树（下轮 diff 出 D 行，前端过滤）、任务间的删除/
    // 新增如实落在前后基线树上、规则变更无需迁移。中途检查点（高危工具执行前，
    // 频率高）保持增量——全量重装按 2 万文件一次几百毫秒，挂在高频路径太贵；
    // 它的树只服务回滚粒度，下个全量拍会重置。
    // 全量拍直接枚举工作树（排除目录/工具链/10MB/2万上限同口径）：树的定义就是
    // 「当前过滤后的工作区」，跟任何清单残留无关——未跟踪文件入基线后就成了影子
    // tracked，list_untracked 不再列它们，靠「真跟踪∪影子未跟踪」拼全量必然漏，
    // 第二拍就会看到空清单、整棵树冻死在上一拍（第一版实现实测翻在这里）。
    // 已删除/已 ignore 的文件不枚举到 → 自然退出树，diff 出 D 行，前端过滤。
    let files = if full_refresh {
        walk_files(ws)?
    } else {
        tracked_files(&bin, ws)?
    };
    let mut spec_paths: Vec<String> = files
        .iter()
        .map(|p| p.to_string_lossy().replace('\\', "/"))
        .collect();
    // 真仓库 .gitignore 里挂名的路径必须从 spec 剔除：git add 遇到显式给的忽略
    // 路径会整批拒掉（2026-11-23 实锤）。git 工作区两份清单天然不含（ls-files 对
    // tracked 免疫 ignore、list_untracked 带 --exclude-standard）；非 git 工作区的
    // walk_files 不读 ignore，这里兜底。剔掉的路径不进树：diff 出 D 行，前端过滤。
    if !spec_paths.is_empty() {
        let spec_input: Vec<u8> = spec_paths
            .iter()
            .flat_map(|p| {
                let mut b = p.clone().into_bytes();
                b.push(0);
                b
            })
            .collect();
        if let Ok(out) = git_stdin(&bin, None, ws, &["check-ignore", "-z", "--stdin"], &spec_input) {
            // 退出码 0=有命中、1=全没命中，都算查成功（-z 下每条就是纯路径）
            if out.status.code().is_none_or(|c| c == 0 || c == 1) {
                let ignored: std::collections::HashSet<String> = String::from_utf8_lossy(&out.stdout)
                    .split('\0')
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
                if !ignored.is_empty() {
                    spec_paths.retain(|p| !ignored.contains(p));
                }
            }
        }
    }

    // 全量拍：先清空 index 再全量装入（纯函数语义）。空清单且已有 HEAD 时不清——
    // 空清单多半是枚举抖动而非工作区真被清空，保持旧树、走原有 None/空提交语义。
    let did_reset = if full_refresh && (!spec_paths.is_empty() || !git(&bin, Some(&gitdir), ws, &["rev-parse", "--verify", "HEAD"]).map(|o| o.status.success()).unwrap_or(false)) {
        git_ok(&bin, Some(&gitdir), ws, &["read-tree", "--empty"])?;
        true
    } else {
        false
    };
    if !spec_paths.is_empty() {
        // pathspec 走文件，防 Windows 命令行 8191 字符上限
        let spec = std::env::temp_dir().join(format!("agent-cp-{}.nul", uuid::Uuid::new_v4().simple()));
        let mut buf = Vec::new();
        for f in &spec_paths {
            buf.extend_from_slice(f.as_bytes());
            buf.push(0);
        }
        std::fs::write(&spec, &buf).context("写 pathspec 文件失败")?;

        let add = git(
            &bin,
            Some(&gitdir),
            ws,
            // -A：删除如实入基线的双保险（pathspec 含已删除路径时，个别形态下默认 add
            // 会「did not match any files」fatal）。
            &["add", "-A", "--pathspec-from-file", &spec.to_string_lossy(), "--pathspec-file-nul"],
        );
        let _ = std::fs::remove_file(&spec);
        let mut out = add?;
        if !out.status.success() {
            // 容错：pathspec 里个别路径匹配不到 = 该文件从未入过任何基线（影子 index 里
            // 也没有）且工作区已删——它的「删除」不构成对任何基线的差异，剔除后重试一次。
            // 真异常（权限/仓库损坏）重试后仍会原样报错。
            let existing: Vec<String> =
                spec_paths.iter().filter(|f| ws.join(f).exists()).cloned().collect();
            if existing.len() == spec_paths.len() {
                if did_reset {
                    // 清过账本又装不进去：把 index 还原回 HEAD，不留半空中间态
                    let _ = git(&bin, Some(&gitdir), ws, &["read-tree", "HEAD"]);
                }
                return Err(anyhow!("git add 失败: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
            let spec2 = std::env::temp_dir().join(format!("agent-cp-{}.nul", uuid::Uuid::new_v4().simple()));
            let mut buf2 = Vec::new();
            for f in &existing {
                buf2.extend_from_slice(f.as_bytes());
                buf2.push(0);
            }
            std::fs::write(&spec2, &buf2).context("写 pathspec 文件失败")?;
            out = git(
                &bin,
                Some(&gitdir),
                ws,
                &["add", "-A", "--pathspec-from-file", &spec2.to_string_lossy(), "--pathspec-file-nul"],
            )?;
            let _ = std::fs::remove_file(&spec2);
            if !out.status.success() {
                if did_reset {
                    let _ = git(&bin, Some(&gitdir), ws, &["read-tree", "HEAD"]);
                }
                return Err(anyhow!("git add 失败: {}", String::from_utf8_lossy(&out.stderr).trim()));
            }
        }
    }

    let has_head = git(&bin, Some(&gitdir), ws, &["rev-parse", "--verify", "HEAD"])
        .map(|o| o.status.success())
        .unwrap_or(false);
    let tracked_changed = !git(&bin, Some(&gitdir), ws, &["diff", "--cached", "--quiet"])?.status.success();
    // 空工作区起步的第一拍仍留空树提交（老行为）；已有提交之后的空拍不造提交，
    // 但 git 工作区的「任务起点」例外——它是改动条/产物归属的基线，必须有 id。
    let need = tracked_changed || (!has_head && files.is_empty()) || (git_ws && task_start);
    if !need {
        return Ok(None);
    }

    let sid = session_id.trim();
    let msg = if sid.is_empty() {
        format!("checkpoint: {}", one_line(reason, 120))
    } else {
        // trailer 单独成段：git log %b 可解析，人看也清楚
        format!("checkpoint: {}\n\nAgent-Session: {}", one_line(reason, 120), sid)
    };
    git_ok(&bin, Some(&gitdir), ws, &["commit", "--quiet", "--allow-empty", "-m", &msg])?;
    let id = git_ok(&bin, Some(&gitdir), ws, &["rev-parse", "HEAD"])?;
    Ok(Some(Checkpoint {
        id,
        ts: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
        reason: one_line(reason, 200),
        session: sid.to_string(),
    }))
}

/// 最近 limit 个检查点（新→旧）。
/// 正文带 `Agent-Session:` trailer 的解析出归属会话；`-z` 让记录以 NUL 分隔，
/// 这样多行正文不会破坏按行切分。
pub fn list(ws: &Path, limit: usize) -> anyhow::Result<Vec<Checkpoint>> {
    let bin = git_bin()?;
    let gitdir = shadow_gitdir(ws);
    if !gitdir.exists() {
        return Ok(vec![]);
    }
    let out = git_ok(
        &bin,
        Some(&gitdir),
        ws,
        &["log", "-n", &limit.to_string(), "-z", "--pretty=%H%x1f%aI%x1f%s%x1f%b"],
    )?;
    Ok(out
        .split('\0')
        .filter_map(|rec| {
            let mut it = rec.splitn(4, '\x1f');
            let id = it.next()?.trim().to_string();
            if id.is_empty() {
                return None;
            }
            let ts = it.next().unwrap_or("").to_string();
            let reason = it.next().unwrap_or("").strip_prefix("checkpoint: ").unwrap_or("").to_string();
            let session = it
                .next()
                .unwrap_or("")
                .lines()
                .find_map(|l| l.trim().strip_prefix("Agent-Session:"))
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            Some(Checkpoint { id, ts, reason, session })
        })
        .collect())
}

/// 工作区 git 概览：是否仓库、当前分支、HEAD、脏文件数、上游领先/落后、未提交文件清单。
/// `is_repo=false` 时分两种，前端要区别说话（缺 git 时报「没检测到 git」+ 安装指引，
/// 不是静默隐藏——2026-10-06 定稿）：`git_missing=true` 是机器上没装 git（撤销/
/// 检查点/改动/审查全都不可用），`git_missing=false` 是装了 git 但当前工作区不是仓库。
#[derive(Debug, Clone, serde::Serialize)]
pub struct GitStatus {
    pub is_repo: bool,
    /// git.exe 没找到（发现链六步全空）：机器缺 git，全套 git 功能不可用
    pub git_missing: bool,
    pub branch: String,
    pub head: String,
    pub dirty: usize,
    pub upstream: String,
    pub ahead: u64,
    pub behind: u64,
    /// 暂存区文件数（porcelain 第一列有改动，`??` 不算）
    pub staged: usize,
    /// 工作区未暂存改动数（porcelain 第二列有改动，含冲突）
    pub unstaged: usize,
    /// 未跟踪文件数
    pub untracked: usize,
    /// 冲突文件数
    pub conflicts: usize,
    /// 未提交文件明细（前端「待提交」列表用），最多 `GIT_FILES_CAP` 条
    pub files: Vec<GitFile>,
}

/// 一个未提交文件：`status` 是 git porcelain 的两字符状态码（`M` / `??` / `A` / `D` / `R` …）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct GitFile {
    pub status: String,
    pub path: String,
}

/// 明细列表上限：够用又不至于把 SSE/HTTP 响应撑爆。
const GIT_FILES_CAP: usize = 500;

pub fn status(ws: &Path) -> GitStatus {
    let blank = GitStatus {
        is_repo: false,
        git_missing: true,
        branch: String::new(),
        head: String::new(),
        dirty: 0,
        upstream: String::new(),
        ahead: 0,
        behind: 0,
        staged: 0,
        unstaged: 0,
        untracked: 0,
        conflicts: 0,
        files: Vec::new(),
    };
    let Ok(bin) = git_bin() else { return blank };
    match git_ok(&bin, None, ws, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(t) if t.trim() == "true" => {}
        // 装了 git、只是这目录不是仓库：git_missing=false（前端说「不是 git 仓库」）
        _ => return GitStatus { git_missing: false, ..blank },
    }
    let mut st = GitStatus { is_repo: true, git_missing: false, ..blank };
    st.branch = git_ok(&bin, None, ws, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    if st.branch == "HEAD" {
        st.branch = "（分离头指针）".to_string();
    }
    st.head = git_ok(&bin, None, ws, &["rev-parse", "--short", "HEAD"]).unwrap_or_default();
    // core.quotepath=false：中文/特殊字符路径按原样输出，不转义成 \xxx 八进制
    if let Ok(out) = git_ok(&bin, None, ws, &["-c", "core.quotepath=false", "status", "--porcelain=v1", "-b"]) {
        for line in out.lines() {
            if let Some(rest) = line.strip_prefix("## ") {
                // `## main...origin/main [ahead 1, behind 2]`
                if let Some((_, tail)) = rest.split_once("...") {
                    st.upstream = tail.split_whitespace().next().unwrap_or("").to_string();
                }
                if let Some(idx) = rest.find('[') {
                    let inner = rest[idx + 1..].trim_end_matches(']');
                    for part in inner.split(',') {
                        let p = part.trim();
                        if let Some(v) = p.strip_prefix("ahead ") {
                            st.ahead = v.trim().parse().unwrap_or(0);
                        } else if let Some(v) = p.strip_prefix("behind ") {
                            st.behind = v.trim().parse().unwrap_or(0);
                        }
                    }
                }
                continue;
            }
            if line.trim().is_empty() {
                continue;
            }
            let rest: String = line.chars().skip(2).collect();
            // 重命名形如 `old -> new`，取新名字；路径两侧可能带引号（含特殊字符时）
            let path = rest
                .trim()
                .rsplit(" -> ")
                .next()
                .unwrap_or("")
                .trim()
                .trim_matches('"')
                .to_string();
            // 本应用自己的状态目录（.agent 影子仓库）、工具链、构建产物不进面板账目——
            // 否则「未提交 N」会把 .agent/ 这样的目录算成一个文件（2026-11-23 实测）。
            let rel = path.trim_end_matches(['/', '\\']);
            if rel.is_empty()
                || Path::new(rel)
                    .components()
                    .any(|c| matches!(c, std::path::Component::Normal(_)) && is_excluded_dir(&c.as_os_str().to_string_lossy()))
            {
                continue;
            }
            st.dirty += 1;
            // 两列状态分开计数（X=暂存区 / Y=工作区），面板「未提交/已暂存/已提交」
            // 三个数就来自这里；文件明细仍然只保留 trim 后的短码（旧口径不动）。
            let mut xy = line.chars();
            let x = xy.next().unwrap_or(' ');
            let y = xy.next().unwrap_or(' ');
            if x == '?' && y == '?' {
                st.untracked += 1;
            } else {
                if x != ' ' {
                    st.staged += 1;
                }
                if y != ' ' {
                    st.unstaged += 1;
                }
                if x == 'U' || y == 'U' || (x == 'A' && y == 'A') || (x == 'D' && y == 'D') {
                    st.conflicts += 1;
                }
            }
            if st.files.len() >= GIT_FILES_CAP {
                continue;
            }
            let status: String = line.chars().take(2).collect::<String>().trim().to_string();
            st.files.push(GitFile { status, path });
        }
    }
    st
}

/// 未跟踪文本文件的新增行数（与 git 面板「+N」同口径：≤1MB、无 NUL、按行数）。
pub(crate) fn count_text_lines(path: &Path) -> Option<u64> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > 1024 * 1024 {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    if bytes.contains(&0) {
        return None;
    }
    let text = String::from_utf8(bytes).ok()?;
    Some(text.lines().count() as u64)
}

/// 未跟踪文件按当前文件行数补「+N」——任务/累计/未提交三档共用同一口径。
/// cap 防大目录一次读太多文件；二进制/超大文件如实不回数。
pub(crate) fn fill_untracked_adds(ws: &Path, files: &mut [ChangedFile], cap: usize) {
    let mut counted = 0usize;
    for f in files.iter_mut().filter(|f| f.status == '?') {
        if counted >= cap {
            break;
        }
        if let Some(lines) = count_text_lines(&ws.join(&f.path)) {
            f.adds = lines;
            counted += 1;
        }
    }
}

/// 改动汇总卡（「本任务改动」/「累计改动」两张卡的数字）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct ChangeSummary {
    /// 比较基线（任务档=检查点 id；累计档=上游 merge-base 或 HEAD）
    pub base: String,
    /// 展示用短 id
    pub short: String,
    pub files: usize,
    pub adds: u64,
    pub dels: u64,
}

/// git 面板总览（2026-11-23 内容改版）：一次算齐摘要卡、状态计数、
/// 归属标签所需的路径集合、冲突/rename/binary 统计。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct GitOverview {
    pub is_repo: bool,
    pub git_missing: bool,
    pub branch: String,
    pub head: String,
    pub upstream: String,
    pub ahead: u64,
    pub behind: u64,
    pub staged: usize,
    pub unstaged: usize,
    pub untracked: usize,
    pub conflicts: usize,
    pub renames: usize,
    pub binaries: usize,
    pub stash: usize,
    pub task: ChangeSummary,
    pub cumulative: ChangeSummary,
    /// 当前工作区里未提交（暂存/未暂存/未跟踪）的路径——逐行「未提交」标签用
    pub dirty: Vec<String>,
    /// 真仓库里未跟踪的路径（默认不勾提交，防误把日志/产物带进 commit）
    pub untracked_paths: Vec<String>,
    /// stash 里出现过的路径
    pub stash_paths: Vec<String>,
    /// 最近 24 小时被检查点回滚碰过的路径
    pub rollback_paths: Vec<String>,
    /// 人话提醒（无上游退化等），空串 = 没有
    pub warning: String,
}

fn short_id(id: &str) -> String {
    id.chars().take(7).collect()
}

fn summary_of(base: &str, files: &[ChangedFile]) -> ChangeSummary {
    ChangeSummary {
        base: base.to_string(),
        short: short_id(base),
        files: files.len(),
        adds: files.iter().map(|f| f.adds).sum(),
        dels: files.iter().map(|f| f.dels).sum(),
    }
}

/// 上游与 HEAD 的分叉点；上游 ref 不存在/无共同祖先返回 None。
fn merge_base(ws: &Path, upstream: &str) -> Option<String> {
    let bin = git_bin().ok()?;
    let out = git_ok(&bin, None, ws, &["merge-base", upstream, "HEAD"]).ok()?;
    let id = out.trim();
    (!id.is_empty()).then(|| id.to_string())
}

/// 累计 diff 里的 rename/copy 与二进制文件数（未跟踪文件不在 diff 里，不计）。
fn diff_kinds(ws: &Path, base: &str) -> (usize, usize) {
    let Ok(bin) = git_bin() else { return (0, 0) };
    let renames = git_ok(&bin, None, ws, &["-c", "core.quotepath=false", "diff", "--name-status", base])
        .map(|out| out.lines().filter(|l| l.starts_with('R') || l.starts_with('C')).count())
        .unwrap_or(0);
    let binaries = git_ok(&bin, None, ws, &["diff", "--numstat", base])
        .map(|out| {
            out.lines()
                .filter(|l| {
                    let mut it = l.splitn(3, '\t');
                    it.next() == Some("-") || it.next() == Some("-")
                })
                .count()
        })
        .unwrap_or(0);
    (renames, binaries)
}

/// stash 条数与里面出现过的路径（最多看 10 条 stash / 500 个路径）。
fn stash_info(ws: &Path) -> (usize, Vec<String>) {
    let Ok(bin) = git_bin() else { return (0, vec![]) };
    let Ok(list) = git_ok(&bin, None, ws, &["stash", "list", "--format=%gd"]) else {
        return (0, vec![]);
    };
    let refs: Vec<&str> = list.lines().map(str::trim).filter(|s| !s.is_empty()).collect();
    let mut paths = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for r in refs.iter().take(10) {
        let Ok(out) = git_ok(&bin, None, ws, &["stash", "show", "--name-only", "--pretty=format:", r]) else {
            continue;
        };
        for p in out.lines().map(str::trim).filter(|s| !s.is_empty()) {
            let p = p.replace('\\', "/");
            if seen.insert(p.clone()) && paths.len() < 500 {
                paths.push(p);
            }
        }
    }
    (refs.len(), paths)
}

/// 当前工作区未提交路径集合（暂存/未暂存/未跟踪；未跟踪目录展开成文件）。
/// 「本任务改动」提交后要消失：用它把已提交的文件从任务清单里筛掉。
pub(crate) fn dirty_paths(ws: &Path) -> std::collections::HashSet<String> {
    let st = status(ws);
    let mut set: std::collections::HashSet<String> =
        st.files.iter().filter(|f| f.status != "??").map(|f| f.path.clone()).collect();
    if let Ok(bin) = git_bin()
        && let Ok(list) = list_untracked(&bin, None, ws)
    {
        set.extend(list);
    }
    set
}

/// 未提交文件的首见快照缓存：`<shadow gitdir>|<rel>` → 影子仓库里第一次记录它的提交。
/// 首见提交一旦确定不会变，进程内永久缓存；只缓存命中的，稍后才进快照的文件下次再查。
static FIRST_SEEN: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, String>>> =
    std::sync::OnceLock::new();

fn first_seen_commit(bin: &Path, gitdir: &Path, ws: &Path, rel: &str) -> Option<String> {
    let key = format!("{}|{}", gitdir.display(), rel.replace('\\', "/"));
    let cache = FIRST_SEEN.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    if let Some(hit) = cache.lock().unwrap_or_else(|e| e.into_inner()).get(&key) {
        return Some(hit.clone());
    }
    let out = git_ok(bin, Some(gitdir), ws, &["log", "--reverse", "--format=%H", "--", rel]).ok()?;
    let id = out.lines().next()?.trim().to_string();
    if id.is_empty() {
        return None;
    }
    cache.lock().unwrap_or_else(|e| e.into_inner()).insert(key, id.clone());
    Some(id)
}

/// 仓库根提交（无上游时的稳定累计基线：提交新内容不会让它移动）。
fn repo_root_commit(bin: &Path, ws: &Path) -> Option<String> {
    let out = git_ok(bin, None, ws, &["rev-list", "--max-parents=0", "HEAD"]).ok()?;
    out.lines().rfind(|s| !s.trim().is_empty()).map(|s| s.trim().to_string())
}

/// 「累计改动」：相对一个**稳定基线**（上游分叉点 / 仓库根提交）列出改过的文件，
/// 再对每个文件用「第一次被影子仓库记录的快照」算真实增删——这样提交以后改动不会
/// 从累计里消失（2026-11-23 用户要求：累计提交后要常驻并标已提交），后来修改也看得到。
/// 从未被记录过的文件按整文件新增算。
/// 返回 (累计基线, 文件清单)；文件条目带 `base`（各自的比较基点）。
pub fn cumulative_files(ws: &Path) -> anyhow::Result<(String, Vec<ChangedFile>)> {
    let bin = git_bin()?;
    let inside = git_ok(&bin, None, ws, &["rev-parse", "--is-inside-work-tree"])
        .map(|s| s.trim() == "true")
        .unwrap_or(false);
    if !inside {
        bail!("工作区不在 git 仓库中");
    }
    let st = status(ws);
    // 稳定基线：有上游=分叉点（fetch/pull 前不动）；无上游=根提交（commit 不会移动它）。
    let cum_base = if st.upstream.trim().is_empty() {
        repo_root_commit(&bin, ws).unwrap_or_else(|| "HEAD".to_string())
    } else {
        merge_base(ws, &st.upstream)
            .or_else(|| repo_root_commit(&bin, ws))
            .unwrap_or_else(|| "HEAD".to_string())
    };
    // 候选 = 相对稳定基线改过的全部文件（含已提交改动），不只是工作区脏文件。
    let mut files = changed_files(ws, RepoOrigin::Workspace, &cum_base)?;
    let gitdir = shadow_gitdir(ws);
    let shadow_ok = gitdir.exists();
    let mut out: Vec<ChangedFile> = Vec::new();
    let mut groups: std::collections::HashMap<String, Vec<ChangedFile>> = std::collections::HashMap::new();
    for f in files.drain(..) {
        let base = if shadow_ok { first_seen_commit(&bin, &gitdir, ws, &f.path) } else { None };
        match base {
            Some(base) => groups.entry(base).or_default().push(f),
            None => out.push(f), // 从没被记录过：整文件新增（调用方补行数）
        }
    }
    let mut group_bases: Vec<String> = Vec::new();
    let mut missing: Vec<ChangedFile> = Vec::new();
    for (base, items) in groups {
        group_bases.push(base.clone());
        let paths: Vec<&str> = items.iter().map(|f| f.path.as_str()).collect();
        let mut ns: Vec<&str> = vec!["-c", "core.quotepath=false", "diff", "--no-renames", "--numstat", &base, "--"];
        ns.extend(paths.iter().copied());
        let Ok(numstat) = git_ok(&bin, Some(&gitdir), ws, &ns) else {
            // 算不了真实增删：退回整文件新增，别让文件从清单里消失
            out.extend(items);
            continue;
        };
        let mut stargs: Vec<&str> =
            vec!["-c", "core.quotepath=false", "diff", "--no-renames", "--name-status", &base, "--"];
        stargs.extend(paths.iter().copied());
        let status_out = git_ok(&bin, Some(&gitdir), ws, &stargs).unwrap_or_default();
        let mut status_of: std::collections::HashMap<String, char> = std::collections::HashMap::new();
        for line in status_out.lines() {
            let mut it = line.splitn(2, '\t');
            if let (Some(s), Some(p)) = (it.next(), it.next())
                && let Some(code) = s.chars().next()
            {
                status_of.insert(p.trim().to_string(), code);
            }
        }
        let mut pending: std::collections::HashMap<String, ChangedFile> =
            items.into_iter().map(|f| (f.path.clone(), f)).collect();
        for line in numstat.lines() {
            let mut it = line.splitn(3, '\t');
            let (Some(a), Some(d), Some(p)) = (it.next(), it.next(), it.next()) else { continue };
            let path = p.trim().to_string();
            let Some(mut f) = pending.remove(&path) else { continue };
            f.status = status_of.get(&path).copied().unwrap_or('M');
            f.adds = a.parse().unwrap_or(0); // 二进制为 "-"，按 0
            f.dels = d.parse().unwrap_or(0);
            f.base = Some(base.clone());
            out.push(f);
        }
        // 剩下的 = 自首见后没再改过：是不是「累计改动」取决于它当时在不在项目基线里
        missing.extend(pending.into_values());
    }
    // 项目基线 B = 这批未提交文件里最早的首见提交。文件首见后没再改过、且 B 里
    // 已经有它 = 项目早就有的文件、这期间没动过，不算累计改动；B 里没有它 =
    // 它是项目开始之后才新建的，整文件算新增（不能因为「首见后没改」就从清单消失）。
    if !missing.is_empty() {
        let mut oldest: Option<(i64, String)> = None;
        for b in &group_bases {
            if let Ok(ts) = git_ok(&bin, Some(&gitdir), ws, &["show", "-s", "--format=%ct", b])
                && let Ok(t) = ts.trim().parse::<i64>()
                && oldest.as_ref().map(|(o, _)| t < *o).unwrap_or(true)
            {
                oldest = Some((t, b.clone()));
            }
        }
        let base_tree: std::collections::HashSet<String> = oldest
            .as_ref()
            .and_then(|(_, b)| git_ok(&bin, Some(&gitdir), ws, &["ls-tree", "-r", "--name-only", "-z", b]).ok())
            .map(|s| {
                s.split('\0')
                    .filter(|p| !p.is_empty())
                    .map(|p| p.replace('\\', "/"))
                    .collect()
            })
            .unwrap_or_default();
        for f in missing {
            if !base_tree.contains(&f.path) {
                out.push(f); // 项目基线之后新建的文件：整文件新增
            }
        }
    }
    // 删除：文件已不在 HEAD，但删除本身是已跟踪文件的改动（提交删除不能标成未跟踪）
    for f in out.iter_mut() {
        if f.status == 'D' {
            f.tracked = true;
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    Ok((cum_base, out))
}

fn restore_log_path(ws: &Path) -> PathBuf {
    ws.join(".agent").join("shadow").join("restore.jsonl")
}

fn append_restore_log(ws: &Path, id: &str, files: &[String]) {
    if files.is_empty() {
        return;
    }
    let path = restore_log_path(ws);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let rec = serde_json::json!({
        "ts": Utc::now().timestamp(),
        "id": short_id(id),
        "files": files,
    });
    let mut lines: Vec<String> = std::fs::read_to_string(&path)
        .ok()
        .map(|s| s.lines().map(str::to_string).collect())
        .unwrap_or_default();
    lines.push(rec.to_string());
    if lines.len() > 50 {
        lines.drain(0..lines.len() - 50);
    }
    let _ = std::fs::write(&path, lines.join("\n") + "\n");
}

/// 最近 24 小时内被检查点回滚碰过的路径（git 面板「回滚」归属标签用）。
pub fn recent_restored_paths(ws: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(restore_log_path(ws)) else { return vec![] };
    let cutoff = Utc::now().timestamp() - 24 * 3600;
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for line in text.lines().rev() {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        if v.get("ts").and_then(serde_json::Value::as_i64).unwrap_or(0) < cutoff {
            continue;
        }
        let Some(files) = v.get("files").and_then(serde_json::Value::as_array) else { continue };
        for f in files {
            if let Some(p) = f.as_str() {
                let p = p.replace('\\', "/");
                if seen.insert(p.clone()) && out.len() < 500 {
                    out.push(p);
                }
            }
        }
    }
    out
}

/// git 面板总览。`task_baseline` 是前端解析出的本任务起点（空=还没有任务基线）。
pub fn overview(ws: &Path, task_baseline: &str) -> anyhow::Result<GitOverview> {
    let st = status(ws);
    // status 的未跟踪是「目录折叠」形态（`?? android/`），面板要逐文件归属与计数：
    // 用 list_untracked 展开成文件清单（同一套排除/上限规则）。
    let untracked_paths: Vec<String> = if st.is_repo {
        git_bin().ok().and_then(|bin| list_untracked(&bin, None, ws).ok()).unwrap_or_default()
    } else {
        vec![]
    };
    let mut dirty: Vec<String> =
        st.files.iter().filter(|f| f.status != "??").map(|f| f.path.clone()).collect();
    dirty.extend(untracked_paths.iter().cloned());
    let mut ov = GitOverview {
        is_repo: st.is_repo,
        git_missing: st.git_missing,
        branch: st.branch.clone(),
        head: st.head.clone(),
        upstream: st.upstream.clone(),
        ahead: st.ahead,
        behind: st.behind,
        staged: st.staged,
        unstaged: st.unstaged,
        untracked: untracked_paths.len(),
        conflicts: st.conflicts,
        dirty,
        untracked_paths,
        ..GitOverview::default()
    };
    if !st.is_repo {
        return Ok(ov);
    }
    let mut warnings: Vec<String> = Vec::new();

    // 累计基线：有上游=merge-base（分叉点），没有=最后一次提交。
    // 无上游不写长 warning（摘要行已有「无上游」，累计卡片 tooltip 说明口径）；
    // 只有读取失败才提示。
    let cum_base = if st.upstream.trim().is_empty() {
        "HEAD".to_string()
    } else {
        match merge_base(ws, &st.upstream) {
            Some(b) => b,
            None => {
                warnings.push("上游合并基点读取失败，累计改动按最后一次提交对比".to_string());
                "HEAD".to_string()
            }
        }
    };

    // 本任务改动：影子仓库相对任务起点检查点，但**只留仍未提交的文件**
    // （提交之后该项从任务清单消失；改动的历史留在累计档里标已提交）。
    if !task_baseline.trim().is_empty() {
        match changed_files(ws, RepoOrigin::Shadow, task_baseline) {
            Ok(files) => {
                let pending: std::collections::HashSet<&str> =
                    ov.dirty.iter().map(String::as_str).collect();
                let mut files: Vec<ChangedFile> = files
                    .into_iter()
                    .filter(|f| f.status != 'D' && pending.contains(f.path.as_str()))
                    .collect();
                fill_untracked_adds(ws, &mut files, 2000);
                ov.task = summary_of(task_baseline, &files);
            }
            Err(e) => warnings.push(format!("本任务改动读取失败：{e}")),
        }
    }

    // 累计改动：已跟踪文件相对分叉点；未提交文件相对各自首见快照
    match cumulative_files(ws) {
        Ok((base, mut files)) => {
            fill_untracked_adds(ws, &mut files, 2000);
            ov.cumulative = summary_of(&base, &files);
        }
        Err(e) => warnings.push(format!("累计改动读取失败：{e}")),
    }

    let (renames, binaries) = diff_kinds(ws, &cum_base);
    ov.renames = renames;
    ov.binaries = binaries;
    let (stash, stash_paths) = stash_info(ws);
    ov.stash = stash;
    ov.stash_paths = stash_paths;
    ov.rollback_paths = recent_restored_paths(ws);
    ov.warning = warnings.join("；");
    Ok(ov)
}

/// 提交工作区改动（用户在界面上手动提交）。`files` 为空 = `add -A` 全部，
/// 否则只 stage 指定路径。返回短哈希；失败（含「没有可提交的改动」）原样上抛 git 的报错。
pub fn commit(ws: &Path, message: &str, files: &[String]) -> anyhow::Result<String> {
    let msg = message.trim();
    if msg.is_empty() {
        bail!("提交信息不能为空");
    }
    let bin = git_bin()?;
    match git_ok(&bin, None, ws, &["rev-parse", "--is-inside-work-tree"]) {
        Ok(t) if t.trim() == "true" => {}
        _ => bail!("当前工作区不是 git 仓库"),
    }
    if files.is_empty() {
        git_ok(&bin, None, ws, &["add", "-A"])?;
    } else {
        let mut args: Vec<&str> = vec!["add", "--"];
        args.extend(files.iter().map(String::as_str));
        git_ok(&bin, None, ws, &args)?;
    }
    git_ok(&bin, None, ws, &["commit", "-m", msg])?;
    git_ok(&bin, None, ws, &["rev-parse", "--short", "HEAD"])
}

/// 工作树恢复到指定检查点。只回退**该检查点之后被影子仓库跟踪且发生变化的文件**
/// （agent 实际碰过的），绝不整树 reset——基线过旧时不会误伤 agent 没碰过的文件
/// （如用户自己已提交/未提交的改动）。只动影子仓库与工作区文件，不碰真仓库 HEAD。
/// 返回被回滚的路径，并记一条 restore 日志（git 面板逐行「回滚」归属标签用）。
pub fn restore(ws: &Path, id: &str) -> anyhow::Result<Vec<String>> {
    if !id.chars().all(|c| c.is_ascii_hexdigit()) || !(7..=40).contains(&id.len()) {
        bail!("非法检查点 id: {id}");
    }
    // 与 checkpoint 同一把串行锁：restore 改工作树 + 动影子 index，不能与在拍的
    // 检查点交错（2026-10-08 修）
    let _serial = SHADOW_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let bin = git_bin()?;
    let gitdir = shadow_gitdir(ws);
    if !gitdir.exists() {
        bail!("影子仓库不存在");
    }
    // 该检查点与当前 HEAD 之间 agent 改了哪些文件（M/D=改/删 → 恢复；A=新增 → 删除）
    let out = git_ok(&bin, Some(&gitdir), ws, &["diff", "--name-status", id, "HEAD"])?;
    let mut touched = 0usize;
    let mut restored: Vec<String> = Vec::new();
    let mut restored_seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let note = |p: &str, out: &mut Vec<String>, seen: &mut std::collections::HashSet<String>| {
        let p = p.replace('\\', "/");
        if seen.insert(p.clone()) {
            out.push(p);
        }
    };
    for line in out.lines() {
        // 2026-10-08 修两处：① rename 检测默认开着，输出 `R100\told\tnew` 三字段——
        // 原来 splitn(2) 把 "old\tnew" 当一个 path，checkout 必失败，且循环前面的行
        // 已改完，工作区停在半回滚态；② CJK 路径默认被 quotepath 转义
        // （\346\226\207.txt），ws.join 找不到文件——与 status() 一样带
        // core.quotepath=false（本机 gitconfig 恰好设了会掩盖，换机器即复现）。
        let mut it = line.split('\t');
        let status = it.next().unwrap_or("").to_string();
        let status0 = status.chars().next().unwrap_or('M');
        // R/C（rename/copy）有两个路径：恢复到旧路径意味着删新的、还原旧的
        let (path, renamed_from) = if matches!(status0, 'R' | 'C') {
            match (it.next(), it.next()) {
                (Some(from), Some(to)) => (to.trim().to_string(), Some(from.trim().to_string())),
                _ => continue,
            }
        } else {
            match it.next() {
                Some(p) => (p.trim().to_string(), None),
                None => continue,
            }
        };
        if status0 == 'A' {
            // 影子仓库在检查点后新增的文件：直接从工作区删掉
            let full = ws.join(&path);
            let _ = std::fs::remove_file(&full);
            let _ = git_ok(&bin, Some(&gitdir), ws, &["reset", "--quiet", id, "--", &path]);
        } else {
            // M / D：把该文件恢复回检查点时的内容
            git_ok(&bin, Some(&gitdir), ws, &["checkout", id, "--", &path])?;
            // rename 的旧路径也要还原（新路径上面的 A 分支已删）
            if let Some(from) = renamed_from {
                git_ok(&bin, Some(&gitdir), ws, &["checkout", id, "--", &from])?;
                note(&from, &mut restored, &mut restored_seen);
            }
        }
        note(&path, &mut restored, &mut restored_seen);
        touched += 1;
    }
    if touched == 0 {
        // 没有文件改动不是失败：这条消息之后可能只聊了天/只调了只读工具。
        // 回滚还要继续回滚对话状态，这里必须当成功的空操作。
        return Ok(Vec::new());
    }
    append_restore_log(ws, id, &restored);
    Ok(restored)
}

/// 影子仓库当前 HEAD（最近一次检查点）；空仓库/不存在返回 None。
/// 用作任务起点基线：结束时的改动统计与「撤销」都以它为参照。
pub fn head_id(ws: &Path) -> anyhow::Result<Option<String>> {
    let gitdir = shadow_gitdir(ws);
    if !gitdir.exists() {
        return Ok(None);
    }
    let bin = git_bin()?;
    match git_ok(&bin, Some(&gitdir), ws, &["rev-parse", "HEAD"]) {
        Ok(id) => Ok(Some(id)),
        // 空仓库 rev-parse 失败属正常
        Err(_) => Ok(None),
    }
}

/// 未跟踪文本文件 → 合成的「全部新增」unified diff（≤256KB、无 NUL 字节）。
/// 查不到/已跟踪/二进制/太大一律 None，调用方回退原说明行。
fn untracked_text_diff(bin: &Path, gitdir: Option<&Path>, ws: &Path, rel: &str) -> Option<String> {
    if rel.is_empty() {
        return None;
    }
    // 已跟踪 = 空 diff 代表「没改动」，不能把整文件冒充成本次新增
    let tracked = git(bin, gitdir, ws, &["ls-files", "--error-unmatch", "--", rel])
        .map(|o| o.status.success())
        .unwrap_or(true); // 查询失败时保守：不当未跟踪处理
    if tracked {
        return None;
    }
    let meta = std::fs::metadata(ws.join(rel)).ok()?;
    if !meta.is_file() || meta.len() > 256 * 1024 {
        return None;
    }
    let bytes = std::fs::read(ws.join(rel)).ok()?;
    if bytes.contains(&0) {
        return None; // 二进制
    }
    let text = String::from_utf8(bytes).ok()?;
    let n = text.lines().count();
    let mut body = String::with_capacity(text.len() + n);
    for ln in text.lines() {
        body.push('+');
        body.push_str(ln);
        body.push('\n');
    }
    Some(format!(
        "diff --git a/{rel} b/{rel}\nnew file mode 100644\n--- /dev/null\n+++ b/{rel}\n@@ -0,0 +1,{n} @@\n{body}"
    ))
}

/// 工作树相对基线 commit 的统一 diff 文本（git 面板逐文件看改动用）。
/// 上限 512KB，防长会话 diff 把面板撑爆；超限尾部标省略。
/// `origin` 决定跑影子仓库还是工作区自己的真仓库（「未提交改动」档）。
pub fn diff_text(
    ws: &Path,
    origin: RepoOrigin,
    baseline: &str,
    path: Option<&str>,
) -> anyhow::Result<String> {
    origin.check_baseline(baseline)?;
    let bin = git_bin()?;
    let gitdir = origin.gitdir(ws);
    match (&origin, &gitdir) {
        (RepoOrigin::Shadow, Some(g)) if !g.exists() => bail!("影子仓库不存在"),
        (RepoOrigin::Workspace, _) | (RepoOrigin::Branch, _) => {
            let inside = git_ok(&bin, None, ws, &["rev-parse", "--is-inside-work-tree"])
                .map(|s| s.trim() == "true")
                .unwrap_or(false);
            if !inside {
                bail!("工作区不在 git 仓库中");
            }
        }
        _ => {}
    }
    let mut args = vec!["diff".to_string(), baseline.trim().to_string()];
    if let Some(p) = path {
        // 只做形态校验，**不 resolve 到磁盘**——已删除的文件（技能迁移/agent 删的）
        // 工作树里已不存在，但 diff 照样要能出「删除」段（真机实测：os error 3）。
        // 禁止绝对路径/盘符/.. 穿越。
        let p = p.trim();
        if p.is_empty() || Path::new(p).is_absolute() || p.contains(':') || p.split(['/', '\\']).any(|s| s == "..") {
            bail!("非法路径: {p}");
        }
        args.push("--".to_string());
        args.push(p.replace('\\', "/"));
    }
    let out = git_ok(&bin, gitdir.as_deref(), ws, &args.iter().map(String::as_str).collect::<Vec<_>>())?;
    // 未跟踪文件没有 diff（git 只在跟踪文件上出 diff）。新建的源码展开时合成一整段
    // 「全部新增」，否则面板只能写「未跟踪新增」+0 -0，源码看起来像没改过
    // （2026-11-21 用户实锤：一整个 Android 项目在清单里点不开任何内容）。
    // 只对文本、≤256KB、确实未跟踪的文件合成；二进制/大文件保持原来的说明行。
    if let Some(p) = path {
        let rel = p.trim().replace('\\', "/");
        if out.trim().is_empty()
            && let Some(text) = untracked_text_diff(&bin, gitdir.as_deref(), ws, &rel)
        {
            return Ok(text);
        }
    }
    const MAX: usize = 512 * 1024;
    // 2026-10-08 修：按字节下标切 String，第 524288 字节落在多字节字符中间即
    // 「byte index is not a char boundary」panic（CJK diff 里约 2/3 的边界字节是
    // 续字节）→ spawn_blocking 任务 panic、端点 500。按字符边界回退。
    if out.len() > MAX {
        let mut end = MAX;
        while end > 0 && !out.is_char_boundary(end) {
            end -= 1;
        }
        Ok(format!("{}…（diff 超过 512KB 已截断）", &out[..end]))
    } else {
        Ok(out)
    }
}

/// 改动统计（保留原样）：工作树相对基线 commit 的 (文件数, 增行, 删行)。
/// 用 `diff --numstat`（覆盖暂存+未暂存，即当前工作树全量相对基线）。
pub fn diff_stat(ws: &Path, baseline: &str) -> anyhow::Result<(usize, u64, u64)> {
    if !baseline.chars().all(|c| c.is_ascii_hexdigit()) || !(7..=40).contains(&baseline.len()) {
        bail!("非法基线 id: {baseline}");
    }
    let bin = git_bin()?;
    let gitdir = shadow_gitdir(ws);
    if !gitdir.exists() {
        bail!("影子仓库不存在");
    }
    let out = git_ok(&bin, Some(&gitdir), ws, &["diff", "--numstat", baseline])?;
    let mut files = 0usize;
    let (mut adds, mut dels) = (0u64, 0u64);
    for line in out.lines() {
        let mut it = line.splitn(3, '\t');
        let (a, d) = match (it.next(), it.next()) {
            (Some(a), Some(d)) => (a, d),
            _ => continue,
        };
        files += 1;
        adds += a.parse::<u64>().unwrap_or(0); // 二进制文件为 "-"，按 0 计
        dels += d.parse::<u64>().unwrap_or(0);
    }
    Ok((files, adds, dels))
}

/// 一个改动文件（2026-10-02 审查面板/展开列表用，改动条原来只有计数）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct ChangedFile {
    /// 工作区相对路径
    pub path: String,
    /// git 两字状态码首字母：M 改 / A 增 / D 删；未跟踪为 '?'
    pub status: char,
    /// 新增行数（二进制/未跟踪为 0）
    pub adds: u64,
    /// 删除行数（二进制/未跟踪为 0）
    pub dels: u64,
    /// 该路径是否已在真仓库 HEAD 里（「已提交」归属标签用；被 ignore/从未提交为 false）
    #[serde(default)]
    pub tracked: bool,
    /// 累计档专用：未提交文件各自的比较基点（影子仓库里第一次记录它的提交）。
    /// None = 用整档基线（已跟踪文件）或没有历史（整文件算新增）。
    #[serde(default)]
    pub base: Option<String>,
}

/// numstat 对重命名输出的路径 → 真正的新路径。两种形态：
/// `dir/{old => new}/rest`（共同目录，git 最常用）与裸 `old => new`。
/// 原样保留会让产物面板出现 "old => new" 这种点不开的假路径（2026-10-08 修）。
fn numstat_path(p: &str) -> String {
    let Some(i) = p.find(" => ") else { return p.to_string() };
    let left = &p[..i];
    let right = &p[i + 4..];
    if let Some(lb) = left.rfind('{') {
        let inner_left = &left[lb + 1..];
        let dir_prefix = inner_left.rfind('/').map(|k| &inner_left[..=k]).unwrap_or("");
        let outer_prefix = &left[..lb];
        let (new_tail, suffix) = match right.find('}') {
            Some(rb) => (&right[..rb], &right[rb + 1..]),
            None => (right, ""),
        };
        format!("{outer_prefix}{dir_prefix}{new_tail}{suffix}")
    } else {
        right.to_string()
    }
}

/// 工作树相对基线 commit 的改动文件清单（`diff --numstat` + `--name-status` 合并）。
/// **追加未跟踪文件**（`ls-files --others`）：任务里新建的文件不入 git diff
/// （基线检查点之后 add 过才可见），产物面板「本任务还没有产物」的第二个实测根因——
/// agent 造出的新文件必须原样出现在清单里。排除目录/大文件与检查点同一套规则。
pub fn changed_files(
    ws: &Path,
    origin: RepoOrigin,
    baseline: &str,
) -> anyhow::Result<Vec<ChangedFile>> {
    origin.check_baseline(baseline)?;
    let bin = git_bin()?;
    let gitdir = origin.gitdir(ws);
    match (&origin, &gitdir) {
        (RepoOrigin::Shadow, Some(g)) if !g.exists() => bail!("影子仓库不存在"),
        (RepoOrigin::Workspace, _) | (RepoOrigin::Branch, _) => {
            let inside = git_ok(&bin, None, ws, &["rev-parse", "--is-inside-work-tree"])
                .map(|s| s.trim() == "true")
                .unwrap_or(false);
            if !inside {
                bail!("工作区不在 git 仓库中");
            }
        }
        _ => {}
    }
    let base = baseline.trim().to_string();
    // numstat：每行 "adds\tdels\tpath"
    let numstat = git_ok(&bin, gitdir.as_deref(), ws, &["diff", "--numstat", &base])?;
    // name-status：每行 "M\tpath"（未跟踪不在此列）
    let status = git_ok(&bin, gitdir.as_deref(), ws, &["diff", "--name-status", &base])?;
    let mut status_of = std::collections::HashMap::new();
    for line in status.lines() {
        let mut it = line.splitn(2, '\t');
        if let (Some(s), Some(p)) = (it.next(), it.next()) {
            let code = s.chars().next().unwrap_or('M');
            // 2026-10-08 修：R/C 行有两个路径（`R100\told\tnew`），splitn(2) 把
            // "old\tnew" 整个当键——与 numstat 侧重命名行的新路径永远对不上，
            // 产物面板于是出现一条 path="old => new" 的假路径、status 退化成 'M'。
            // 取最后一个 tab 后的新路径，与 numstat_path 的口径一致。
            let path = if matches!(code, 'R' | 'C') {
                p.rsplit('\t').next().unwrap_or(p).to_string()
            } else {
                p.to_string()
            };
            status_of.insert(path, code);
        }
    }
    // 真仓库 HEAD 里的路径集合：逐行「已提交」归属标签用（被 ignore 或从未提交
    // 的文件不能因为「当前不脏」就被当成已提交；2026-11-23 真机实锤：android/
    // 未跟踪目录里的源码在任务清单里被误标已提交）。
    let head_tree: Option<std::collections::HashSet<String>> = if origin == RepoOrigin::Branch {
        None
    } else {
        git_ok(&bin, None, ws, &["ls-tree", "-r", "--name-only", "-z", "HEAD"]).ok().map(|s| {
            s.split('\0')
                .filter(|s| !s.is_empty())
                .map(|p| p.replace('\\', "/"))
                .collect()
        })
    };
    let is_tracked = |p: &str| -> bool {
        head_tree.as_ref().map(|t| t.contains(p)).unwrap_or(origin == RepoOrigin::Branch)
    };
    let mut out: Vec<ChangedFile> = Vec::new();
    for line in numstat.lines() {
        let mut it = line.splitn(3, '\t');
        let a = it.next().unwrap_or("0");
        let d = it.next().unwrap_or("0");
        if let Some(p) = it.next() {
            let path = numstat_path(p);
            let tracked = is_tracked(&path);
            out.push(ChangedFile {
                status: status_of.get(&path).copied().unwrap_or('M'),
                path,
                adds: a.parse().unwrap_or(0),
                dels: d.parse().unwrap_or(0),
                tracked,
                base: None,
            });
        }
    }
    // 未跟踪文件：基线里已有的不能算本轮改动（2026-11-21 实锤：工作区 1.1 万个
    // 历史未跟踪文件全被列进「本任务改动」）。纯函数快照后基线树就是那一刻的
    // 全量清单，「基线里有没有」直接查树成员，不再依赖平行的路径 txt + mtime 猜
    // （mtime 会因拷贝保留时间戳而说谎）。基线树查不到（仓库损坏等）才退回
    // 基线提交时间兜底。Workspace（未提交改动档）：未跟踪本来就该全列，保持老行为。
    let base_tree: Option<std::collections::HashSet<String>> = if origin == RepoOrigin::Shadow {
        git_ok(&bin, gitdir.as_deref(), ws, &["ls-tree", "-r", "--name-only", "-z", &base]).ok().map(
            |s| {
                s.split('\0')
                    .filter(|s| !s.is_empty())
                    .map(|p| p.replace('\\', "/"))
                    .collect()
            },
        )
    } else {
        None
    };
    let base_ts = if base_tree.is_none() && origin == RepoOrigin::Shadow {
        git_ok(&bin, gitdir.as_deref(), ws, &["show", "-s", "--format=%ct", &base])
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
    } else {
        None
    };
    let mtime_after_base = |rel: &str| -> bool {
        let Some(t) = base_ts else { return true };
        std::fs::metadata(ws.join(rel))
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() + 1 >= t) // 1s 容差：秒级 commit 时间与文件 mtime 对齐
            .unwrap_or(false)
    };
    let mut seen: std::collections::HashSet<String> = out.iter().map(|f| f.path.clone()).collect();
    // branch 档（全部分支更改，ZCode 同款）只算已提交改动：`git diff <上游>...HEAD`
    // 本来就不含未跟踪文件，这里也不追加，保持口径一致
    if origin != RepoOrigin::Branch {
        for rel in list_untracked(&bin, gitdir.as_deref(), ws)? {
            if origin == RepoOrigin::Shadow {
                let keep = match &base_tree {
                    // 基线树里有 = 基线时刻就存在：它的改动已由 numstat 覆盖（内容基线），
                    // 再按 '?' 列一遍会重复
                    Some(tree) => !tree.contains(&rel),
                    None => mtime_after_base(&rel), // 基线树查不到才退回时间兜底
                };
                if !keep {
                    continue;
                }
            }
            if seen.insert(rel.clone()) {
                let tracked = is_tracked(&rel);
                out.push(ChangedFile { path: rel, status: '?', adds: 0, dels: 0, tracked, base: None });
            }
        }
    }
    Ok(out)
}

/// 旧会话产物回溯：只沿影子仓库里**该会话自己的提交链**算改动。
///
/// 检查点都在工具执行前拍，所以相邻两个本会话提交的 diff = 上一个工具/任务产生的
/// 改动。规则：
/// - 会话第一个提交不算（它是任务起点/首个工具前的快照，diff 属于之前的会话）；
/// - 父子链上夹了别的会话（或手动）提交的窗口直接跳过——无法安全归属；
/// - 只返回**现在仍存在**的文件，删除态没有意义；
/// - git 工作区的检查点只跟踪真仓库 ls-files，新文件用 mtime 落在
///   [首个提交, 末个提交] 窗口内的未跟踪文件补回来（截图/产物目录常是未跟踪的）。
///
/// 这是给「账本上线前的老会话」补历史用的一次性回溯，结果会写进 artifacts 账本。
/// 返回 `None` = 当前工作区的影子仓库里没有这个会话的任何提交（会话属于别的工作区，
/// 或检查点已被清理）；调用方不要据此写空账本，否则会把会话错误绑定到当前工作区。
pub fn session_changed_paths(ws: &Path, session: &str) -> anyhow::Result<Option<Vec<String>>> {
    let session = session.trim();
    if session.is_empty()
        || session.len() > 64
        || !session.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        bail!("非法 session id");
    }
    let gitdir = shadow_gitdir(ws);
    if !gitdir.exists() {
        bail!("影子仓库不存在");
    }
    let bin = git_bin()?;
    let grep = format!("Agent-Session: {session}");
    let out = git_ok(
        &bin,
        Some(&gitdir),
        ws,
        &[
            "log",
            "--reverse",
            "--fixed-strings",
            "--grep",
            &grep,
            "--pretty=%H%x1f%P%x1f%ct%x1f%b%x1e",
        ],
    )?;
    struct Commit {
        id: String,
        parents: String,
        ts: i64,
    }
    let mut commits: Vec<Commit> = Vec::new();
    for rec in out.split('\x1e') {
        let rec = rec.trim_matches('\n');
        if rec.trim().is_empty() {
            continue;
        }
        let mut it = rec.splitn(4, '\x1f');
        let Some(id) = it.next().map(str::trim).filter(|s| !s.is_empty()) else { continue };
        let parents = it.next().unwrap_or("").trim().to_string();
        let ts: i64 = it.next().unwrap_or("0").trim().parse().unwrap_or(0);
        let body = it.next().unwrap_or("");
        // --grep 是全文匹配，这里再精确核对 trailer，防命令文本里恰好带同样字样
        let sid = body
            .lines()
            .find_map(|l| l.trim().strip_prefix("Agent-Session:"))
            .map(str::trim)
            .unwrap_or("");
        if sid != session {
            continue;
        }
        commits.push(Commit { id: id.to_string(), parents, ts });
    }
    if commits.is_empty() {
        return Ok(None);
    }

    let mut paths: Vec<String> = Vec::new();
    let push = |p: &str, ws: &Path, out: &mut Vec<String>| {
        let p = p.replace('\\', "/");
        let p = p.trim_start_matches("./").to_string();
        if !crate::artifacts::safe_rel_path(&p) || p.to_lowercase().starts_with(".agent/") {
            return;
        }
        if ws.join(&p).is_file() && !out.iter().any(|x| x == &p) {
            out.push(p);
        }
    };
    for i in 1..commits.len() {
        let prev = &commits[i - 1];
        let cur = &commits[i];
        // 只认直接父子：中间夹了别的会话提交，这个窗口就不归属本会话
        if !cur.parents.split_whitespace().any(|p| p == prev.id) {
            continue;
        }
        let Ok(diff) = git_ok(
            &bin,
            Some(&gitdir),
            ws,
            &["diff", "--name-status", "-z", &prev.id, &cur.id],
        ) else {
            continue;
        };
        let mut toks = diff.split('\0');
        while let Some(status) = toks.next() {
            if status.is_empty() {
                continue;
            }
            let code = status.chars().next().unwrap_or('M');
            let Some(p1) = toks.next() else { break };
            if matches!(code, 'R' | 'C') {
                let Some(p2) = toks.next() else { break };
                push(p2, ws, &mut paths);
            } else if code != 'D' {
                push(p1, ws, &mut paths);
            }
        }
    }
    // 未跟踪文件补充：git 工作区的检查点不含未跟踪文件，只能按会话提交时间窗补
    if commits.len() >= 2 {
        let first_ts = commits.first().map(|c| c.ts).unwrap_or(0);
        let last = commits.last().expect("len >= 2");
        let last_ts = last.ts;
        if first_ts > 0
            && last_ts >= first_ts
            && let Ok(files) = changed_files(ws, RepoOrigin::Shadow, &last.id)
        {
            for f in files {
                if f.status != '?' {
                    continue;
                }
                let Ok(meta) = std::fs::metadata(ws.join(&f.path)) else { continue };
                let Ok(mtime) = meta.modified() else { continue };
                let secs = mtime
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                // 2 秒容差：提交时间与文件 mtime 的边界误差
                if secs + 2 >= first_ts && secs <= last_ts + 2 {
                    push(&f.path, ws, &mut paths);
                }
            }
        }
    }
    paths.sort();
    Ok(Some(paths))
}

/// 基线树里的**顶层目录**在当前工作区已整体消失，且其文件在 diff 里呈删除态——
/// 典型成因是用户在文件管理器里把目录挪走/删掉，不是本任务干的。
/// 2026-10-13 真机实测：智云目录从工作区挪走 5 天后，旧基线把它下面 81 个文件
/// 全 diff 成删除，产物/git 面板报出一堆幽灵删除（-7186 行），看着像别的
/// 工作区的改动串过来了。返回人话警告；无可疑子树返回 None。
pub fn stale_subtree_warning(
    ws: &Path,
    origin: RepoOrigin,
    baseline: &str,
    files: &[ChangedFile],
) -> Option<String> {
    // 删除态文件按顶层段归组
    let mut deleted: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for f in files {
        if f.status != 'D' {
            continue;
        }
        if let Some(top) = f.path.split('/').next().filter(|s| !s.is_empty()) {
            *deleted.entry(top).or_insert(0) += 1;
        }
    }
    if deleted.is_empty() {
        return None;
    }
    // 当前工作区的顶层条目名（文件与目录都在内）
    let live: std::collections::BTreeSet<String> = std::fs::read_dir(ws)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    // 基线树里的顶层**目录**（-d 只要 tree；根级单文件删除不算「子树消失」）
    let bin = git_bin().ok()?;
    let gitdir = origin.gitdir(ws);
    let trees = git_ok(&bin, gitdir.as_deref(), ws, &["ls-tree", "-d", "--name-only", baseline.trim()]).ok()?;
    let gone: Vec<(&str, usize)> = trees
        .lines()
        .map(str::trim)
        .filter(|t| !t.is_empty() && !live.contains(*t))
        .filter_map(|t| deleted.get(t).map(|n| (t, *n)))
        .collect();
    if gone.is_empty() {
        return None;
    }
    let names: Vec<String> = gone.iter().map(|(t, n)| format!("{t}/（{n} 个文件）")).collect();
    Some(format!(
        "基线快照里的顶层目录 {} 已不在当前工作区——这些文件的「删除」很可能是目录被移走/删掉造成的，不是本任务改的；撤销或提交前先确认。",
        names.join("、")
    ))
}

/// 测试与排障用：影子仓库当前跟踪的文件清单。
#[cfg(test)]
pub fn shadow_files(ws: &Path) -> Vec<String> {
    let bin = crate::tools::git_exe().expect("git 可用");
    let gitdir = shadow_gitdir(ws);
    git_ok(&bin, Some(&gitdir), ws, &["ls-files"])
        .unwrap_or_default()
        .lines()
        .map(String::from)
        .collect()
}

fn one_line(s: &str, max_chars: usize) -> String {
    let flat: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    flat.chars().take(max_chars).collect()
}



#[cfg(test)]
mod tests {
    use super::*;

    fn git_have() -> bool {
        crate::tools::git_exe().is_ok()
    }

    /// 建临时工作区：git init + 文件入 index（staged 即算 tracked，无需真 commit）
    fn temp_ws(tag: &str) -> TestWs {
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("agent-p6-{tag}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        TestWs(p)
    }
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

    struct TestWs(PathBuf);
    impl TestWs {
        fn path(&self) -> &Path {
            &self.0
        }
        fn git(&self, args: &[&str]) {
            let bin = crate::tools::git_exe().unwrap();
            let out = git(&bin, None, &self.0, args).unwrap();
            assert!(out.status.success(), "git {:?} 失败: {}", args, String::from_utf8_lossy(&out.stderr));
        }
    }
    impl Drop for TestWs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn checkpoint_create_list_restore() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("cp");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        // a.txt 入 index（tracked）；new.txt 是 untracked（影子仓库不应跟踪）
        std::fs::write(w.join("a.txt"), "v1\n").unwrap();
        ws.git(&["add", "a.txt"]);
        std::fs::write(w.join("new.txt"), "agent-made\n").unwrap();

        // 第一次检查点：a.txt=v1
        let cp1 = checkpoint(w, "任务 A 开始改 a.txt", "sess-test").unwrap().expect("有 tracked 文件应有检查点");

        // 改 a.txt + 新建 tracked 文件 b.txt
        std::fs::write(w.join("a.txt"), "v2\n").unwrap();
        std::fs::write(w.join("b.txt"), "b\n").unwrap();
        ws.git(&["add", "b.txt"]);
        let cp2 = checkpoint(w, "任务 A 第二步", "sess-test").unwrap().expect("有变更应有检查点");
        assert_ne!(cp1.id, cp2.id);

        // 无变更再拍 → None
        assert!(checkpoint(w, "重复拍", "sess-test").unwrap().is_none(), "无变更不应产出检查点");

        let list = list(w, 10).unwrap();
        assert_eq!(list.len(), 2, "两个检查点: {list:?}");
        assert_eq!(list[0].id, cp2.id, "新→旧");
        assert!(list[1].reason.contains("任务 A 开始"), "reason 保留: {:?}", list[1].reason);

        // restore 到 cp1：a.txt 回 v1；cp2 引入的 b.txt（影子仓库已跟踪）应被删；new.txt 不受影响
        restore(w, &cp1.id).unwrap();
        assert_eq!(std::fs::read_to_string(w.join("a.txt")).unwrap(), "v1\n");
        assert!(!w.join("b.txt").exists(), "cp2 新建的已跟踪文件应被回滚删除");
        assert!(w.join("new.txt").exists(), "untracked 文件不属于影子仓库，不动");

        // 影子仓库只跟踪真仓库 tracked 的文件
        let files = shadow_files(w);
        assert!(files.contains(&"a.txt".to_string()));
        assert!(!files.iter().any(|f| f == "new.txt"), "untracked 不进影子仓库: {files:?}");
    }

    /// 检查点归属：会话 id 写进提交正文，list() 能解析回来；旧检查点（无 trailer）为空串。
    #[test]
    fn checkpoint_records_session_owner() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("owner");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("a.txt"), "v1\n").unwrap();
        ws.git(&["add", "a.txt"]);

        checkpoint(w, "会话甲的第一步", "sess-aaa").unwrap().expect("应有检查点");
        std::fs::write(w.join("a.txt"), "v2\n").unwrap();
        // 空 session（旧行为/无会话上下文）不应写出 trailer
        checkpoint(w, "无归属", "").unwrap().expect("应有检查点");

        let list = list(w, 10).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].session, "", "空 session 不写 trailer");
        assert_eq!(list[1].session, "sess-aaa", "会话 id 应能从 %b 解析回来");
        assert!(list[1].reason.contains("会话甲"), "reason 不受 trailer 影响: {:?}", list[1].reason);

        // 按会话过滤（server 层 /api/checkpoints?session= 用的就是这条判定）
        let mine: Vec<_> = list.iter().filter(|c| c.session == "sess-aaa").collect();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0].reason, "会话甲的第一步");
    }

    #[test]
    fn checkpoint_excludes_and_size_filter() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("exclude");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::create_dir_all(w.join("node_modules")).unwrap();
        std::fs::write(w.join("node_modules").join("pkg.js"), "big dep\n").unwrap();
        std::fs::write(w.join("normal.txt"), "ok\n").unwrap();
        ws.git(&["add", "node_modules/pkg.js", "normal.txt"]);

        assert!(checkpoint(w, "排除规则", "sess-test").unwrap().is_some());
        let files = shadow_files(w);
        assert!(files.contains(&"normal.txt".to_string()));
        assert!(!files.iter().any(|f| f.starts_with("node_modules")), "排除目录不进影子仓库: {files:?}");

        // 大文件跳过：影子仓库不跟踪它 → 相对上次检查点无可见变更 → None
        std::fs::write(w.join("huge.bin"), vec![0u8; 11 * 1024 * 1024]).unwrap();
        ws.git(&["add", "huge.bin"]);
        assert!(checkpoint(w, "大文件前", "sess-test").unwrap().is_none(), "被排除的大文件不应触发检查点");
        let files = shadow_files(w);
        assert!(!files.iter().any(|f| f == "huge.bin"), "超 10MB 文件不进影子仓库: {files:?}");
    }

    /// 全部分支更改（origin=branch）：基线=上游分支...HEAD（ZCode 同款），只含已提交改动；
    /// 未跟踪文件不入清单；没设上游 = branch_base None（前端显示来源不可用）。
    #[test]
    fn branch_origin_follows_upstream_merge_base() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("branchorigin");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("a.txt"), "v1\nv1b\nv0\n").unwrap();
        ws.git(&["add", "a.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "c1"]);
        // 本地分支 base 作为上游（免建远端；status -b 显示 ## <当前分支>...base）
        ws.git(&["branch", "base"]);
        std::fs::write(w.join("a.txt"), "v1\nv2\nv3\n").unwrap();
        ws.git(&["add", "a.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "c2"]);

        // 还没设上游：branch_base = None
        assert!(branch_base(w).is_none(), "无上游时 branch_base 应为 None");
        ws.git(&["branch", "--set-upstream-to=base"]);
        assert_eq!(branch_base(w).as_deref(), Some("base...HEAD"));

        // 未跟踪文件不进分支改动清单
        std::fs::write(w.join("c.txt"), "hello\nworld\n").unwrap();
        let base = branch_base(w).unwrap();
        let files = changed_files(w, RepoOrigin::Branch, &base).unwrap();
        let a = files.iter().find(|f| f.path == "a.txt").expect("a.txt 应在分支改动里");
        assert_eq!((a.status, a.adds, a.dels), ('M', 2, 2), "上游...HEAD 的真实增删: {:?}", a);
        assert!(!files.iter().any(|f| f.path == "c.txt"), "未跟踪文件不进分支改动: {files:?}");
        assert_eq!(files.len(), 1, "只含上游...HEAD 的已提交改动: {files:?}");
    }

    #[test]
    fn plain_directory_workspace_gets_checkpoints_and_artifacts() {
        // 产物面板实测（2026-10-02）：用户工作区 Desktop\th 不是 git 仓库，
        // ls-files 无从谈起 → 检查点永远拍不出来 → 产物永远空。非 git 工作区
        // 现在走目录扫描兜底；agent 新建（未跟踪）文件也必须进 changed_files。
        if !git_have() {
            return;
        }
        let ws = temp_ws("plain");
        let w = ws.path();
        // 故意不 git init——普通文件夹
        std::fs::write(w.join("doc.txt"), "v1\n").unwrap();
        let cp1 = checkpoint(w, "普通目录基线", "sess-plain")
            .unwrap()
            .expect("非 git 工作区也应有检查点");

        std::fs::write(w.join("doc.txt"), "v2\n").unwrap();
        std::fs::write(w.join("made-by-agent.txt"), "new\n").unwrap();
        let files = changed_files(w, RepoOrigin::Shadow, &cp1.id).unwrap();
        assert!(files.iter().any(|f| f.path == "doc.txt"), "修改的文件要列出: {files:?}");
        assert!(files.iter().any(|f| f.path == "made-by-agent.txt"), "新建文件要列出: {files:?}");

        // 收尾检查点（真实流程里任务结束会再拍一张），restore 才有「检查点之后」的提交可回退
        let cp2 = checkpoint(w, "普通目录收尾", "sess-plain").unwrap().expect("有变更应有检查点");
        assert_ne!(cp1.id, cp2.id);
        restore(w, &cp1.id).unwrap();
        assert_eq!(std::fs::read_to_string(w.join("doc.txt")).unwrap(), "v1\n");
        assert!(!w.join("made-by-agent.txt").exists(), "检查点后新建的文件已被影子仓库跟踪，回滚应删除");
    }

    #[test]
    fn workspace_origin_reads_real_repo_against_head() {
        // 审查面板「未提交改动」档（origin=workspace）：跑的是工作区自己的真仓库，
        // 基线是 HEAD，不是影子仓库检查点。非 git 工作区要给「不在 git 仓库中」。
        if !git_have() {
            return;
        }
        let ws = temp_ws("wsorigin");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("tracked.txt"), "v1\n").unwrap();
        ws.git(&["add", "tracked.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "base"]);

        // 工作树改动 + 未跟踪新文件
        std::fs::write(w.join("tracked.txt"), "v2\n").unwrap();
        std::fs::write(w.join("fresh.txt"), "new\n").unwrap();

        let files = changed_files(w, RepoOrigin::Workspace, "HEAD").unwrap();
        assert!(files.iter().any(|f| f.path == "tracked.txt"), "相对 HEAD 的改动要列出: {files:?}");
        assert!(files.iter().any(|f| f.path == "fresh.txt" && f.status == '?'), "真仓库未跟踪文件也要列出: {files:?}");

        let d = diff_text(w, RepoOrigin::Workspace, "HEAD", Some("tracked.txt")).unwrap();
        assert!(d.contains("-v1") && d.contains("+v2"), "真仓库 diff 要出改动行: {d}");

        // 非 git 工作区：不给影子仓库兜底，明确报「不在 git 仓库中」
        let plain = temp_ws("wsorigin-plain");
        let e = changed_files(plain.path(), RepoOrigin::Workspace, "HEAD").unwrap_err().to_string();
        assert!(e.contains("不在 git 仓库"), "应说人话: {e}");
        // 影子仓库这一档仍然只要检查点 id
        assert!(RepoOrigin::from_query(Some("shadow")).eq(&RepoOrigin::Shadow));
        assert!(RepoOrigin::from_query(Some("work")).eq(&RepoOrigin::Workspace));
        assert!(RepoOrigin::from_query(None).eq(&RepoOrigin::Shadow));
    }

    #[test]
    fn untracked_directory_is_expanded_to_files() {
        // 2026-11-06 产物归属轮：git 工作区里整个未跟踪目录（截图/产物目录）会被
        // --directory 折叠成 "screenshots/"，早先按目录 metadata 当非文件丢掉，
        // 产物面板看不到 agent 新建的目录内容。现在要展开成具体文件。
        if !git_have() {
            return;
        }
        let ws = temp_ws("untracked-dir");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("base.txt"), "v1\n").unwrap();
        ws.git(&["add", "base.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "base"]);
        std::fs::create_dir_all(w.join("screenshots")).unwrap();
        std::fs::write(w.join("screenshots").join("a.png"), "png").unwrap();
        std::fs::create_dir_all(w.join("node_modules").join("pkg")).unwrap();
        std::fs::write(w.join("node_modules").join("pkg").join("x.js"), "x").unwrap();

        let files = changed_files(w, RepoOrigin::Workspace, "HEAD").unwrap();
        assert!(
            files.iter().any(|f| f.path == "screenshots/a.png" && f.status == '?'),
            "未跟踪目录要展开到文件: {files:?}"
        );
        assert!(
            !files.iter().any(|f| f.path.starts_with("node_modules")),
            "排除目录仍不能进清单: {files:?}"
        );
    }

    /// 任务起点的未跟踪快照要能把「任务前就有的未跟踪文件」挡掉（2026-11-21 实锤：
    /// 历史未跟踪文件全被算成本任务改动）。
    #[test]
    fn task_start_snapshot_scopes_untracked_changes() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("untracked-scope");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("tracked.txt"), "v1").unwrap();
        ws.git(&["add", "tracked.txt"]);
        std::fs::write(w.join("old-untracked.txt"), "old").unwrap();
        std::fs::create_dir_all(w.join("old-dir")).unwrap();
        std::fs::write(w.join("old-dir/a.txt"), "a").unwrap();

        let cp = checkpoint(w, "任务起点", "sess-scope").unwrap().expect("任务起点必须有基线");
        std::thread::sleep(std::time::Duration::from_millis(1100));
        std::fs::write(w.join("new.txt"), "new").unwrap();
        std::fs::write(w.join("old-untracked.txt"), "changed").unwrap();

        let files = changed_files(w, RepoOrigin::Shadow, &cp.id).unwrap();
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert!(paths.contains(&"new.txt"), "任务内新增未跟踪文件要算: {paths:?}");
        assert!(paths.contains(&"old-untracked.txt"), "任务内改过的旧未跟踪文件要算: {paths:?}");
        assert!(!paths.contains(&"old-dir/a.txt"), "任务前就有的未跟踪文件不算: {paths:?}");
    }

    /// 任务起点要把未跟踪文件的**内容**收进影子仓库基线（2026-11-23 实锤：真仓库
    /// 从没 commit 过的整棵项目目录（android/ 全树 untracked）在旧口径下永远进不了
    /// 影子仓库，任务改动只能按「当前行数」补数 → 跨任务修补永远 +N -0，删改看不见）。
    /// 内容入基线后，任务内修补要走 numstat 真实增删。
    #[test]
    fn task_start_commits_untracked_content_for_real_diff() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("untracked-content");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        // 项目文件未跟踪（复刻真仓库只有一次初始化提交、项目目录从未纳入的状态）
        std::fs::create_dir_all(w.join("app")).unwrap();
        std::fs::write(w.join("app/App.java"), "class App {\n    int a = 1;\n}\n").unwrap();

        let cp = checkpoint(w, "任务起点", "s1").unwrap().expect("任务起点必须有基线");
        // 任务内修补：改一行 + 加一行 → 1 删 2 增
        std::fs::write(w.join("app/App.java"), "class App {\n    int a = 2;\n    int b = 3;\n}\n").unwrap();

        let files = changed_files(w, RepoOrigin::Shadow, &cp.id).unwrap();
        let f = files
            .iter()
            .find(|f| f.path == "app/App.java")
            .expect("改动里要有 App.java");
        assert!(
            f.status != '?',
            "内容已入基线要走 numstat 真实增删，不能停留在 '?' 行数口径: {files:?}"
        );
        assert_eq!(f.dels, 1, "修补要有删除行: {files:?}");
        assert_eq!(f.adds, 2, "修补要有新增行: {files:?}");
    }

    /// 上一个任务之后、下一个任务之前删掉的文件，不能算进新任务的改动
    /// （鬼影删除：影子 index 里留着旧内容，新基线没如实记删除就会背锅）。
    #[test]
    fn task_start_records_deletions_since_last_baseline() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("untracked-del");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::create_dir_all(w.join("app")).unwrap();
        std::fs::write(w.join("app/A.java"), "class A {}\n").unwrap();
        std::fs::write(w.join("app/B.java"), "class B {}\n").unwrap();

        let cp1 = checkpoint(w, "任务起点", "s1").unwrap().expect("基线1");
        std::fs::remove_file(w.join("app/B.java")).unwrap();
        // 两个任务之间没有留下任何检查点（用户手动删的文件）
        let cp2 = checkpoint(w, "任务起点", "s2").unwrap().expect("基线2");

        let files = changed_files(w, RepoOrigin::Shadow, &cp2.id).unwrap();
        assert!(
            !files.iter().any(|f| f.path == "app/B.java"),
            "基线2之前的删除不能算进新任务: {files:?}"
        );
        let _ = cp1;
    }

    /// 基线 staging 遇「后来被 .gitignore 挂名的路径」不许整批拒（2026-11-23 实锤：
    /// 任务把 screenshots/ 写进真仓库 .gitignore，影子仓库还跟踪着旧截图——git add
    /// 显式给忽略路径直接报错，任务起点检查点静默失败，基线停在旧提交，改动条
    /// 全项目变 +N）。忽略路径从 spec 剔除，检查点照常成立。
    #[test]
    fn task_start_tolerates_now_ignored_paths() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("ignored-spec");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("base.txt"), "v1\n").unwrap();
        ws.git(&["add", "base.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "init"]);
        std::fs::create_dir_all(w.join("screenshots")).unwrap();
        std::fs::write(w.join("screenshots/a.png"), "PNGDATA").unwrap();

        let cp1 = checkpoint(w, "任务起点", "s-ign").unwrap().expect("首拍");
        // 任务把 screenshots 写进 .gitignore 并提交（真仓库口径变了）
        std::fs::write(w.join(".gitignore"), "screenshots\n").unwrap();
        ws.git(&["add", ".gitignore"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "ignore"]);

        let cp2 = checkpoint(w, "任务起点", "s-ign")
            .unwrap()
            .expect("影子跟踪的路径被 ignore 后，基线照常拍得出（不该静默失败）");
        let _ = cp1;
        assert_ne!(cp1.id, cp2.id);
    }

    /// 未跟踪文本文件要合成「全部新增」diff；已跟踪但没改动的不许冒充新增。
    #[test]
    fn untracked_text_file_gets_synthetic_diff() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("untracked-diff");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("tracked.txt"), "v1").unwrap();
        ws.git(&["add", "tracked.txt"]);
        let cp = checkpoint(w, "任务起点", "sess-utd").unwrap().expect("任务起点必须有基线");
        std::fs::write(w.join("Hello.java"), "class Hello {}\n").unwrap();
        let text = diff_text(w, RepoOrigin::Shadow, &cp.id, Some("Hello.java")).unwrap();
        assert!(text.contains("new file mode"), "{text}");
        assert!(text.contains("+class Hello {}"), "{text}");
        let tracked_nochange = diff_text(w, RepoOrigin::Shadow, &cp.id, Some("tracked.txt")).unwrap();
        assert!(tracked_nochange.is_empty(), "已跟踪无改动不能合成: {tracked_nochange:?}");
    }

    #[test]
    fn session_backfill_only_own_commit_chain() {
        // 2026-11-06 旧会话回溯：只认本会话相邻提交之间的 diff。
        // 第一个提交是会话开始前的快照，不算；中间夹了别的会话提交的窗口跳过。
        if !git_have() {
            return;
        }
        let ws = temp_ws("backfill");
        let w = ws.path();
        std::fs::write(w.join("before.txt"), "x").unwrap();
        checkpoint(w, "任务起点", "sess-a").unwrap().expect("A 首拍");
        std::fs::write(w.join("a2.txt"), "a2").unwrap();
        checkpoint(w, "write_file", "sess-a").unwrap().expect("A 第二个提交");
        std::fs::write(w.join("b1.txt"), "b1").unwrap();
        checkpoint(w, "任务起点", "sess-b").unwrap().expect("B 首拍");
        std::fs::write(w.join("a3.txt"), "a3").unwrap();
        checkpoint(w, "write_file", "sess-a").unwrap().expect("A 第三个提交");
        std::fs::write(w.join("a4.txt"), "a4").unwrap();
        checkpoint(w, "edit_file", "sess-a").unwrap().expect("A 第四个提交");

        let a = session_changed_paths(w, "sess-a").unwrap().expect("A 有提交");
        assert!(a.contains(&"a2.txt".to_string()), "{a:?}");
        assert!(a.contains(&"a4.txt".to_string()), "{a:?}");
        assert!(!a.contains(&"before.txt".to_string()), "会话开始前的文件不算: {a:?}");
        assert!(!a.contains(&"b1.txt".to_string()), "别的会话的文件不能算: {a:?}");
        assert!(!a.contains(&"a3.txt".to_string()), "夹了别会话提交的窗口跳过: {a:?}");
        let b = session_changed_paths(w, "sess-b").unwrap().expect("B 有提交");
        assert!(b.is_empty(), "只有第一个提交的会话没有可归属改动: {b:?}");
        assert!(
            session_changed_paths(w, "sess-missing").unwrap().is_none(),
            "影子仓库里没有的会话要返回 None，别写空账本"
        );
    }

    #[test]
    fn head_id_and_diff_stat() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("diffstat");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        assert!(head_id(w).unwrap().is_none(), "空影子仓库无 HEAD");

        std::fs::write(w.join("a.txt"), "v1\n").unwrap();
        ws.git(&["add", "a.txt"]);
        let cp = checkpoint(w, "基线", "sess-test").unwrap().expect("应有检查点");
        assert_eq!(head_id(w).unwrap().as_deref(), Some(cp.id.as_str()));

        // 检查点之后改 2 个文件：结束快照提交进影子仓库后，diff_stat 相对基线应为 2 文件 +7 -1
        std::fs::write(w.join("a.txt"), "v2\nline2\nline3\nline4\n").unwrap();
        std::fs::write(w.join("b.txt"), "1\n2\n3\n4\n").unwrap();
        ws.git(&["add", "b.txt"]);
        checkpoint(w, "任务结束快照", "sess-test").unwrap().expect("有变更应提交");
        let (files, adds, dels) = diff_stat(w, &cp.id).unwrap();
        assert_eq!(files, 2, "numstat 两行: {files}");
        assert_eq!((adds, dels), (8, 1), "a.txt +4/-1，b.txt 新文件 +4/-0: ({adds},{dels})");

        // 撤销语义验证：reset 回基线 → 改动还原、任务内新建文件被删、diff 归零
        restore(w, &cp.id).unwrap();
        assert!(!w.join("b.txt").exists(), "结束快照已跟踪的新文件应被回滚删除");
        assert_eq!(diff_stat(w, &cp.id).unwrap(), (0, 0, 0));
    }

    #[test]
    fn restore_without_file_changes_is_ok_noop() {
        // 2026-10-27 真机实测：没有文件改动的消息也必须能回滚（回滚=对话+文件），
        // 原先这里报「没有需要回退的文件改动」把整个 truncate 请求拖成 500。
        if !git_have() {
            return;
        }
        let ws = temp_ws("restore-noop");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("a.txt"), "v1\n").unwrap();
        ws.git(&["add", "a.txt"]);
        let cp = checkpoint(w, "基线", "sess-test").unwrap().expect("应有检查点");

        restore(w, &cp.id).unwrap();
        assert_eq!(std::fs::read_to_string(w.join("a.txt")).unwrap(), "v1\n");
        assert_eq!(diff_stat(w, &cp.id).unwrap(), (0, 0, 0));
    }

    #[test]
    fn restore_only_touches_agent_changed_files() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("restore-scope");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("a.txt"), "v1\n").unwrap();
        std::fs::write(w.join("b.txt"), "b1\n").unwrap();
        ws.git(&["add", "a.txt", "b.txt"]);
        let cp1 = checkpoint(w, "基线", "sess-test").unwrap().expect("应有检查点");

        // agent 只改 a.txt；b.txt 在快照里没变
        std::fs::write(w.join("a.txt"), "v2\n").unwrap();
        checkpoint(w, "结束快照", "sess-test").unwrap().expect("应有提交");

        // 用户在快照之后自己改了 b.txt（影子仓库没见过这次改动）
        std::fs::write(w.join("b.txt"), "b2-user\n").unwrap();

        restore(w, &cp1.id).unwrap();
        assert_eq!(std::fs::read_to_string(w.join("a.txt")).unwrap(), "v1\n", "agent 改的文件应回退");
        assert_eq!(
            std::fs::read_to_string(w.join("b.txt")).unwrap(),
            "b2-user\n",
            "用户自己的改动不得被回滚误伤"
        );
    }

    #[test]
    fn restore_rejects_bad_id() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("badid");
        let err = restore(ws.path(), "../../etc").unwrap_err();
        assert!(err.to_string().contains("非法"));
    }



    /// 2026-10-13 真机实测场景：基线拍下时智云还是工作区子目录，之后用户把整个目录
    /// 挪出工作区——diff 把该目录下所有文件报成删除（幽灵删除，看着像别的项目的
    /// 改动串过来了）。stale_subtree_warning 要把这种情况说成白话说清楚。
    #[test]
    fn stale_subtree_warning_flags_moved_away_directory() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("stalesubtree");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::create_dir_all(w.join("智云/assets")).unwrap();
        std::fs::write(w.join("智云/app.js"), "console.log(1)
").unwrap();
        std::fs::write(w.join("智云/app.wxss"), "page { color: red }").unwrap();
        std::fs::write(w.join("智云/assets/logo.png"), "PNGDATA").unwrap();
        std::fs::write(w.join("keep.txt"), "留下
").unwrap();
        ws.git(&["add", "-A"]);

        let cp = checkpoint(w, "任务起点", "sess-test").unwrap().expect("有 tracked 文件应有检查点");

        // 用户把智云整个挪出工作区（文件管理器里的移动，agent 毫不知情）
        std::fs::rename(w.join("智云"), w.join("智云-moved")).unwrap();
        // 另有一个根级文件被真删掉（对照：单文件删除不该触发警告）
        std::fs::remove_file(w.join("keep.txt")).unwrap();

        let files = changed_files(w, RepoOrigin::Shadow, &cp.id).unwrap();
        let deleted: Vec<&str> = files.iter().filter(|f| f.status == 'D').map(|f| f.path.as_str()).collect();
        assert_eq!(deleted.len(), 4, "智云 3 个 + keep.txt 1 个全报删除: {deleted:?}");

        let warn = stale_subtree_warning(w, RepoOrigin::Shadow, &cp.id, &files)
            .expect("顶层目录整体消失必须给出警告");
        assert!(warn.contains("智云/"), "警告要点名挪走的目录: {warn}");
        assert!(warn.contains("3 个文件"), "警告要带文件数: {warn}");
        assert!(!warn.contains("keep.txt"), "根级单文件删除不是子树消失，不该点名: {warn}");
    }

    /// 反向：顶层目录还在（哪怕里面有文件被删/改）→ 不警告。
    #[test]
    fn stale_subtree_warning_silent_when_tree_intact() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("staletree2");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::create_dir_all(w.join("src")).unwrap();
        std::fs::write(w.join("src/a.rs"), "fn a() {}
").unwrap();
        std::fs::write(w.join("src/b.rs"), "fn b() {}
").unwrap();
        ws.git(&["add", "-A"]);

        let cp = checkpoint(w, "任务起点", "sess-test").unwrap().expect("应有检查点");
        // 删掉 src 下一个文件（目录还在）——正常任务语义，不是「目录被挪走」
        std::fs::remove_file(w.join("src/b.rs")).unwrap();

        let files = changed_files(w, RepoOrigin::Shadow, &cp.id).unwrap();
        assert!(files.iter().any(|f| f.status == 'D' && f.path == "src/b.rs"));
        assert!(
            stale_subtree_warning(w, RepoOrigin::Shadow, &cp.id, &files).is_none(),
            "顶层目录还在就不该警告"
        );
    }

    /// 空工作区起步的会话：第一拍留空树提交当任务起点；之后产出的一切相对它都是新增
    /// （2026-10-31 实测：测 工作区空起步，任务起点被当「无变更」跳过，产物面板只能
    /// 拿半路工具快照当基线，index.html 等会话产物全部隐身）。
    #[test]
    fn checkpoint_empty_start_records_baseline() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("cpempty");
        let w = ws.path();
        // 不 git init——复刻真实场景：测 工作区就不是 git 仓库，走整目录扫分支
        // 工作区全空时拍任务起点 → 必须落一个空树提交（不能再当「无变更」跳过）
        let cp1 = checkpoint(w, "任务起点", "sess-empty").unwrap().expect("空起步也要有基线");
        std::fs::write(w.join("index.html"), "<html></html>").unwrap();
        let cp2 = checkpoint(w, "做完了", "sess-empty").unwrap().expect("新文件应有变更");
        let files = changed_files(w, RepoOrigin::Shadow, &cp1.id).unwrap();
        assert!(
            files.iter().any(|f| f.status == 'A' && f.path == "index.html"),
            "相对空起点，会话产出要如实显示为新增: {files:?}"
        );
        let _ = cp2;
    }

    /// 2026-10-31 真机实测「产物不是当前工作区的」根因回归：默认 `git add` 忽略工作区
    /// 删除——用户在文件管理器删掉/挪走目录后拍的新检查点根本没把删除记进影子仓库，
    /// 鬼影文件一代代往后传，产物面板永久挂着幽灵条目。`add -A` 后：删除必须入基线，
    /// 以新基线再看改动就干净了（重拍 = 以当前状态为起点）。
    #[test]
    fn checkpoint_records_worktree_deletions() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("cpdelete");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("a.txt"), "v1\n").unwrap();
        std::fs::create_dir_all(w.join("src")).unwrap();
        std::fs::write(w.join("src/x.txt"), "x\n").unwrap();
        ws.git(&["add", "-A"]);

        let cp1 = checkpoint(w, "任务起点", "sess-del").unwrap().expect("应有检查点");
        // 文件管理器式删除（不 git rm）→ 再拍基线
        std::fs::remove_file(w.join("src/x.txt")).unwrap();
        let cp2 = checkpoint(w, "手动重拍基线", "sess-del").unwrap().expect("删除必须能进基线");

        // 以 cp2 为基线：清单必须干净（无幽灵 D）——重拍语义成立
        let files2 = changed_files(w, RepoOrigin::Shadow, &cp2.id).unwrap();
        assert!(
            !files2.iter().any(|f| f.path == "src/x.txt"),
            "新基线后再看改动不得再有 src/x.txt 鬼影: {files2:?}"
        );
        assert!(
            stale_subtree_warning(w, RepoOrigin::Shadow, &cp2.id, &files2).is_none(),
            "新基线下不该再有陈旧警告"
        );
        // 以 cp1 为基线：删除如实报出（对照：删除确实被记录了）
        let files1 = changed_files(w, RepoOrigin::Shadow, &cp1.id).unwrap();
        assert!(
            files1.iter().any(|f| f.status == 'D' && f.path == "src/x.txt"),
            "相对旧基线的删除要如实列出: {files1:?}"
        );
    }

    /// git 面板总览（2026-11-23 内容改版）：状态三档计数、两张改动卡、归属集合、
    /// 时间线要一次算齐。
    #[test]
    fn overview_counts_status_and_change_summaries() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("overview");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("tracked.txt"), "v1\nv2\n").unwrap();
        ws.git(&["add", "tracked.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "init"]);

        let cp = checkpoint(w, "任务起点", "s-ov").unwrap().expect("任务起点");

        // 任务内：改跟踪文件（未暂存）+ 新建未跟踪 + 暂存一个新增文件
        std::fs::write(w.join("tracked.txt"), "v1\nv2\nv3\n").unwrap();
        std::fs::write(w.join("new-untracked.txt"), "a\nb\n").unwrap();
        std::fs::write(w.join("staged.txt"), "s\n").unwrap();
        ws.git(&["add", "staged.txt"]);

        let ov = overview(w, &cp.id).unwrap();
        assert_eq!(ov.staged, 1, "暂存区一个新增文件: {ov:?}");
        assert_eq!(ov.unstaged, 1, "工作区一个修改: {ov:?}");
        assert_eq!(ov.untracked, 1, "一个未跟踪文件: {ov:?}");
        assert_eq!(ov.conflicts, 0, "{ov:?}");
        assert_eq!(ov.task.base, cp.id, "任务卡基线=任务起点检查点");
        assert_eq!(ov.task.files, 3, "任务改动 = 修改 + 未跟踪 + 暂存新增: {ov:?}");
        assert_eq!(ov.task.adds, 4, "1(修改)+2(未跟踪)+1(暂存新增): {ov:?}");
        assert_eq!(ov.cumulative.files, 3, "无上游时累计=相对 HEAD 的全部改动: {ov:?}");
        assert_eq!(ov.cumulative.adds, 4, "{ov:?}");
        assert!(ov.dirty.iter().any(|p| p == "tracked.txt"), "未提交集合要有脏文件: {ov:?}");
        assert!(
            ov.untracked_paths.iter().any(|p| p == "new-untracked.txt"),
            "真仓库未跟踪集合要有新文件（默认不勾提交）: {ov:?}"
        );
    }

    /// 「累计改动」：真仓库没提交过的文件，要相对它第一次被影子仓库记录的快照算，
    /// 后来改动的 +/- 必须看得见（2026-11-23 用户实锤：只显示整文件新增）。
    #[test]
    fn cumulative_files_uses_first_seen_snapshot_for_untracked() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("cumulative-first-seen");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("base.txt"), "base\n").unwrap();
        ws.git(&["add", "base.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "init"]);

        std::fs::create_dir_all(w.join("src")).unwrap();
        std::fs::write(w.join("src/app.java"), "a\nb\nc\n").unwrap(); // 未提交的项目文件
        let cp1 = checkpoint(w, "任务起点", "s-cum").unwrap().expect("基线1");
        std::fs::write(w.join("src/app.java"), "a\nB\n").unwrap(); // 后来改：+1 -2（相对首见）
        std::fs::write(w.join("src/new.java"), "x\n").unwrap(); // 首见之后才创建
        let _ = checkpoint(w, "任务结束快照", "s-cum").unwrap();

        let (_, files) = cumulative_files(w).unwrap();
        let f = files.iter().find(|f| f.path == "src/app.java").expect("要列出 app.java");
        assert_eq!(f.base.as_deref(), Some(cp1.id.as_str()), "未提交文件要用首见快照当基点: {f:?}");
        assert_eq!(f.adds, 1, "后来的修改要看到新增行: {f:?}");
        assert_eq!(f.dels, 2, "后来的修改要看到删除行: {f:?}");
        assert_ne!(f.status, '?', "首见有底本就不能再按整文件新增: {f:?}");
        let n = files.iter().find(|f| f.path == "src/new.java").expect("新建文件也要列出");
        assert!(n.base.is_none(), "首见之后创建的文件只能整文件新增: {n:?}");
        assert_eq!(n.status, '?', "{n:?}");
    }

    /// 提交之后：累计档保留（标已提交），本任务档靠 pending 过滤清空。
    #[test]
    fn cumulative_keeps_committed_changes_after_commit() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("cumulative-after-commit");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("root.txt"), "root\n").unwrap();
        ws.git(&["add", "root.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "init"]);

        std::fs::create_dir_all(w.join("src")).unwrap();
        std::fs::write(w.join("src/a.txt"), "a\n").unwrap(); // 未提交项目文件
        let cp = checkpoint(w, "任务起点", "s-commit").unwrap().expect("任务起点");
        std::fs::write(w.join("src/a.txt"), "a\nb\n").unwrap();
        // 用户把改动提交了
        ws.git(&["add", "src/a.txt"]);
        ws.git(&["-c", "user.name=t", "-c", "user.email=t@t", "commit", "--quiet", "-m", "feat"]);

        assert!(dirty_paths(w).is_empty(), "提交后工作区应为干净");
        let (_, files) = cumulative_files(w).unwrap();
        let f = files.iter().find(|f| f.path == "src/a.txt").expect("提交后累计档不能清空");
        assert_eq!(f.base.as_deref(), Some(cp.id.as_str()), "仍用首见快照当基点: {f:?}");
        assert_eq!((f.adds, f.dels), (1, 0), "首见后的修改仍要显示: {f:?}");
        assert!(f.tracked, "提交后应能标成已提交: {f:?}");
        // 任务档的原始影子 diff 还在（由 server 的 pending=1 过滤掉）
        let task = changed_files(w, RepoOrigin::Shadow, &cp.id).unwrap();
        assert!(task.iter().any(|f| f.path == "src/a.txt"), "任务档过滤前仍有该文件: {task:?}");
    }

    /// restore 要返回并记录回滚过的路径（git 面板逐行「回滚」归属标签的数据源）。
    #[test]
    fn restore_records_paths_for_ownership_badges() {
        if !git_have() {
            return;
        }
        let ws = temp_ws("restore-log");
        let w = ws.path();
        ws.git(&["init", "--quiet"]);
        std::fs::write(w.join("a.txt"), "v1\n").unwrap();
        let cp = checkpoint(w, "任务起点", "s-r").unwrap().expect("任务起点");
        std::fs::write(w.join("a.txt"), "v2\n").unwrap();
        let _ = checkpoint(w, "任务结束快照", "s-r").unwrap();

        let restored = restore(w, &cp.id).unwrap();
        assert!(restored.iter().any(|p| p == "a.txt"), "回滚要返回路径: {restored:?}");
        let paths = recent_restored_paths(w);
        assert!(paths.iter().any(|p| p == "a.txt"), "回滚日志要能读出路径: {paths:?}");
    }
}
