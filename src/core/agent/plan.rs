use super::*;

/// B3：从模型回复解析 ```plan 围栏块（B3：计划产物结构化）。
/// 行前缀容错匹配（中英文冒号、markdown 加粗/列表符都剥），小节顺序 目标→步骤→文件。
/// 解析结果全空（无任何有效字段）返回 None。
pub fn parse_plan(text: &str) -> Option<Plan> {
    let fence = text.find("```plan")?;
    let body_start = fence + text[fence..].find('\n')? + 1;
    let body_end = text[body_start..]
        .find("```")
        .map(|i| body_start + i)
        .unwrap_or(text.len());
    let body = text[body_start..body_end].trim();
    if body.is_empty() {
        return None;
    }

    let mut goal = String::new();
    let mut steps: Vec<String> = Vec::new();
    let mut files: Vec<String> = Vec::new();
    let mut in_steps = false;

    for line in body.lines() {
        let t = line.trim().trim_start_matches(['*', '#']).trim();
        if t.is_empty() {
            continue;
        }
        if t.starts_with("目标") {
            goal = after_colon(t);
        } else if t.starts_with("步骤") {
            in_steps = true;
        } else if t.starts_with("文件") {
            in_steps = false;
            files = after_colon(t)
                .split([',', '，', '、', ';', '；', ' '])
                .map(|s| s.trim().trim_matches('`').to_string())
                .filter(|s| !s.is_empty())
                .collect();
        } else if in_steps {
            // 步骤项：剥序号 "1. " / "1、" / "1)" / "- "
            let core = t
                .trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '、' || c == ')' || c == '-')
                .trim()
                .to_string();
            if !core.is_empty() {
                steps.push(core);
            }
        }
        // 小节外的散行忽略
    }

    if goal.is_empty() && steps.is_empty() && files.is_empty() {
        None
    } else {
        Some(Plan { goal, steps, files })
    }
}

/// 018：同轮 write_file 同路径写冲突检测——返回应跳过的索引集合。
/// 输入 (工具名, arguments JSON)；仅认 write_file；路径小写 + 反斜杠归一后判重。
pub(crate) fn write_conflict_skips(calls: &[(String, String)]) -> std::collections::HashSet<usize> {
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut skip: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i, (name, args_json)) in calls.iter().enumerate() {
        if name != "write_file" {
            continue;
        }
        let path = serde_json::from_str::<Value>(args_json)
            .ok()
            .and_then(|v| v.get("path").and_then(Value::as_str).map(|p| p.to_lowercase().replace('\\', "/")))
            .unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        if !seen.insert(path) {
            skip.insert(i);
        }
    }
    skip
}

/// 同轮里对**同一路径**的多次写（write_file / edit_file）：这些调用彼此必须串行。
/// 2026-11-02 并行放开后的配套——以前所有写都是串行，天然不会撞；现在并发跑时，
/// 两个 edit_file 改同一个文件会各自 read→replace→write，后写的把前一次的改动盖掉
/// （静默丢修改）。返回这些调用下标，调度层把它们踢回串行组，按顺序执行。
pub(crate) fn same_path_write_indices(calls: &[(String, String)]) -> std::collections::HashSet<usize> {
    let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    let mut clashing: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i, (name, args_json)) in calls.iter().enumerate() {
        if name != "write_file" && name != "edit_file" {
            continue;
        }
        let path = serde_json::from_str::<Value>(args_json)
            .ok()
            .and_then(|v| v.get("path").and_then(Value::as_str).map(|p| p.to_lowercase().replace('\\', "/")))
            .unwrap_or_default();
        if path.is_empty() {
            continue;
        }
        match seen.get(&path) {
            Some(&first) => {
                clashing.insert(first);
                clashing.insert(i);
            }
            None => {
                seen.insert(path, i);
            }
        }
    }
    clashing
}

/// 取行内第一个中/英文冒号后的内容（剥 markdown 加粗星号）。
/// 用 char_indices 定位，避免全角冒号（3 字节）切出非字符边界。
pub(crate) fn after_colon(line: &str) -> String {
    for (i, c) in line.char_indices() {
        if c == ':' || c == '：' {
            return line[i + c.len_utf8()..].trim().trim_matches('*').trim().to_string();
        }
    }
    String::new()
}
