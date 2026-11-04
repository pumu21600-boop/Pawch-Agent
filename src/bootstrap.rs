//! 启动引导：应用数据目录确定的旧数据迁移。
//!
//! 数据目录从「安装目录」搬到一个用户可选的位置后，老用户盘上的数据不能丢：
//! 这里把旧位置里**新目录没有的**条目复制过去——绝不覆盖、绝不删除旧数据
//! （什么时候清理由用户决定）。迁移是幂等的：新目录已有条目一律跳过。

use std::path::Path;

/// 旧数据 → 数据目录。返回搬过来的条目描述（供启动日志）。`install == data` 时无操作。
pub(crate) fn migrate_legacy_data(install: &Path, data: &Path) -> Vec<String> {
    if install == data || !install.is_dir() {
        return Vec::new();
    }
    let mut moved: Vec<String> = Vec::new();

    // 单文件：设置、MCP token
    for name in ["settings.json", "mcp_tokens.json"] {
        if copy_file_if_missing(&install.join(name), &data.join(name)) {
            moved.push(name.to_string());
        }
    }

    // 目录：日志（含 sessions/usage/memory）、插件数据。
    // **plugins/ 与 skills/ 不复制**：安装目录的这两处继续作为「随包内置根」参与扫描，
    // 用户旧装的插件/技能留在原地照样能用（新装的落数据目录）；整份复制会和内置重复，
    // 还会在程序升级后留一份不再更新的旧副本。
    for name in ["logs", "plugin_data"] {
        let n = copy_dir_missing(&install.join(name), &data.join(name));
        if n > 0 {
            moved.push(format!("{name}/（{n} 项）"));
        }
    }

    // 旧安装状态根（改名前后两种）：index.json / profile.md / memory/ → 数据目录根
    for state in [".pawch-agent", ".agent"] {
        let old = install.join(state);
        for name in ["index.json", "profile.md"] {
            if copy_file_if_missing(&old.join(name), &data.join(name)) {
                moved.push(format!("{state}/{name}"));
            }
        }
        let n = copy_dir_missing(&old.join("memory"), &data.join("memory"));
        if n > 0 {
            moved.push(format!("{state}/memory/（{n} 项）"));
        }
    }

    // 技能**不复制**：安装目录的 skills/ 继续作为内置技能根参与扫描，数据目录的
    // skills/ 作为用户技能根（同名时数据目录覆盖）；复制整份会和内置技能重复。
    // 自定义数据目录时：旧用户根（`~/.pawch-agent`）里的约定/记忆/技能也复制过来
    // （默认数据目录就是它，这段自动跳过）。skill-modules 是共享 Node 依赖（可能很大），
    // 不复制——技能脚本下次缺依赖时会按原路径重新装。
    if let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))
        && !home.is_empty()
    {
        let old_user = std::path::PathBuf::from(home).join(".pawch-agent");
        if old_user != data {
            for name in ["AGENTS.md", "index.json", "profile.md"] {
                if copy_file_if_missing(&old_user.join(name), &data.join(name)) {
                    moved.push(format!("~/.pawch-agent/{name}"));
                }
            }
            for name in ["skills", "memory"] {
                let n = copy_dir_missing(&old_user.join(name), &data.join(name));
                if n > 0 {
                    moved.push(format!("~/.pawch-agent/{name}/（{n} 项）"));
                }
            }
        }
    }

    moved
}

fn copy_file_if_missing(src: &Path, dst: &Path) -> bool {
    if !src.is_file() || dst.exists() {
        return false;
    }
    if let Some(parent) = dst.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    std::fs::copy(src, dst).is_ok()
}

/// 递归复制「目标没有的」条目，返回复制成功的文件数。已存在的文件不覆盖。
fn copy_dir_missing(src: &Path, dst: &Path) -> usize {
    if !src.is_dir() {
        return 0;
    }
    let Ok(rd) = std::fs::read_dir(src) else { return 0 };
    let mut n = 0usize;
    for e in rd.flatten() {
        let from = e.path();
        let to = dst.join(e.file_name());
        let Ok(ty) = e.file_type() else { continue };
        if ty.is_dir() {
            if std::fs::create_dir_all(&to).is_err() {
                continue;
            }
            n += copy_dir_missing(&from, &to);
        } else if ty.is_file() && !to.exists() {
            if std::fs::create_dir_all(dst).is_err() {
                continue;
            }
            if std::fs::copy(&from, &to).is_ok() {
                n += 1;
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!(
            "pawch-bootstrap-{tag}-{}-{}",
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
    fn copies_missing_and_never_overwrites() {
        let install = tmp("install");
        let data = tmp("data");
        std::fs::write(install.join("settings.json"), "old").unwrap();
        std::fs::create_dir_all(install.join("logs").join("sessions")).unwrap();
        std::fs::write(install.join("logs").join("sessions").join("a.json"), "s").unwrap();
        std::fs::create_dir_all(install.join(".pawch-agent")).unwrap();
        std::fs::write(install.join(".pawch-agent").join("profile.md"), "p").unwrap();
        // 新目录已有一份 settings.json：不覆盖
        std::fs::write(data.join("settings.json"), "new").unwrap();

        let moved = migrate_legacy_data(&install, &data);
        assert_eq!(std::fs::read_to_string(data.join("settings.json")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(data.join("logs").join("sessions").join("a.json")).unwrap(), "s");
        assert_eq!(std::fs::read_to_string(data.join("profile.md")).unwrap(), "p");
        assert!(moved.iter().any(|m| m.starts_with("logs/")), "{moved:?}");
        // 幂等：第二次没有新搬的
        assert!(migrate_legacy_data(&install, &data).is_empty());

        let _ = std::fs::remove_dir_all(&install);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn same_root_is_noop() {
        let d = tmp("same");
        assert!(migrate_legacy_data(&d, &d).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }
}
