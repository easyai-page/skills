//! add / fav install 的四步交互向导：选技能 → 选 agent → 选 scope → 选安装方式。
//! CLI 与 TUI 共用（两者都在普通终端模式下用 dialoguer）；Web 无弹窗，走 /api 参数。
//! 纯决策函数（expand_targets 等）与弹窗函数分离，前者可单测。

// 任务 5 才接线到 CLI/TUI，之前暂允许 dead_code
#![allow(dead_code)]

use crate::core::agents;
use crate::core::config::Config;
use crate::core::error::{Error, Result};
use crate::core::paths::{Scope, Target};
use crate::core::registry::Method;
// 仅测试用到（生产代码经 Scope::Project 携带 PathBuf，不直接引用）
#[cfg(test)]
use std::path::PathBuf;

/// 技能多选：第一行「全选 (n)」快捷项，默认全不选；空选 = 取消（返回 None）。
/// 单技能直接返回它，不弹窗。
pub fn pick_skills(names: &[String]) -> Result<Option<Vec<String>>> {
    if names.len() == 1 {
        return Ok(Some(vec![names[0].clone()]));
    }
    let mut items: Vec<String> = vec![format!("全选 ({})", names.len())];
    items.extend(names.iter().cloned());
    let idx = dialoguer::MultiSelect::new()
        .with_prompt("选择要安装的技能（空格多选，回车确认）")
        .items(&items)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    if idx.is_empty() {
        return Ok(None);
    }
    // 选中首行快捷项 = 全选，忽略其余勾选状态
    if idx.contains(&0) {
        return Ok(Some(names.to_vec()));
    }
    Ok(Some(
        idx.into_iter().map(|i| names[i - 1].clone()).collect(),
    ))
}

/// agent 多选：detected 里的默认勾选；detected 为空时预选 ["agents"]。
/// 返回 None = 空选取消。
/// 只列内置 agent（config 自定义 target 无法检测本机安装，走 -a 参数指定）。
pub fn pick_agents(detected: &[&'static str]) -> Result<Option<Vec<String>>> {
    let specs = agents::all(); // 注册表已按 id 字典序排列
    let labels: Vec<String> = specs
        .iter()
        .map(|a| format!("{}（{}）", a.id, a.name))
        .collect();
    let defaults: Vec<bool> = if detected.is_empty() {
        specs.iter().map(|a| a.id == "agents").collect()
    } else {
        specs.iter().map(|a| detected.contains(&a.id)).collect()
    };
    let idx = dialoguer::MultiSelect::new()
        .with_prompt("安装到哪些 agent（空格多选，回车确认）")
        .items(&labels)
        .defaults(&defaults)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    if idx.is_empty() {
        return Ok(None);
    }
    Ok(Some(
        idx.into_iter().map(|i| specs[i].id.to_string()).collect(),
    ))
}

/// scope 单选：项目（默认）/ 全局。所有 agent 都不支持全局时直接返回项目，不弹窗。
pub fn pick_scope(agent_ids: &[String]) -> Result<Scope> {
    let ids: Vec<&str> = agent_ids.iter().map(|s| s.as_str()).collect();
    if !needs_scope_prompt(&ids) {
        return Ok(Scope::Project(std::env::current_dir()?));
    }
    let items = ["项目（当前目录）", "全局（用户主目录）"];
    let sel = dialoguer::Select::new()
        .with_prompt("安装范围")
        .items(&items)
        .default(0)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    match sel {
        0 => Ok(Scope::Project(std::env::current_dir()?)),
        _ => Ok(Scope::Global),
    }
}

/// 安装方式单选：symlink（默认，推荐）/ copy。
pub fn pick_method() -> Result<Method> {
    let items = [
        "symlink（符号链接，指向缓存，推荐）",
        "copy（独立副本，脱离缓存）",
    ];
    let sel = dialoguer::Select::new()
        .with_prompt("安装方式")
        .items(&items)
        .default(0)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    Ok(match sel {
        0 => Method::Symlink,
        _ => Method::Copy,
    })
}

/// 选中的 agent × scope 展开为 Target 列表：项目级按解析后的 install_dir 去重
/// （多个 universal agent 项目级同目录只装一份，记录保留排序最前的 id；全局级不去重，
/// 每个 agent 保留独立记录）；跳过 scope 不支持的 agent（如 eve 无全局目录）。
/// 先排序再展开去重，保证结果确定性。
pub fn expand_targets(agent_ids: &[String], scope: &Scope, cfg: &Config) -> Result<Vec<Target>> {
    let mut ids = agent_ids.to_vec();
    ids.sort();
    let project_scope = matches!(scope, Scope::Project(_));
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for id in ids {
        let t = Target {
            agent: id,
            scope: scope.clone(),
        };
        let dir = match t.install_dir(cfg) {
            Ok(d) => d,
            // scope 不支持（Msg）：UI 层已过滤，这里双保险跳过；未知 agent（UnknownTarget）照常报错
            Err(Error::Msg(_)) => continue,
            Err(e) => return Err(e),
        };
        if !project_scope || seen.insert(dir) {
            out.push(t);
        }
    }
    Ok(out)
}

/// 是否需要弹 scope：至少一个选中 agent 支持全局。
/// 未知 / 仅 config 自定义的 id 视为不支持全局（自定义 target 的弹窗由任务 5 处理）。
pub fn needs_scope_prompt(agent_ids: &[&str]) -> bool {
    agent_ids.iter().any(|id| {
        agents::resolve(id, &Default::default())
            .map(|a| a.global_dir.is_some())
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;

    #[test]
    fn dedup_targets_by_install_dir() {
        // 多个 universal agent 同 scope 解析到同一目录，只留一个
        let cfg = Config::default();
        let ids = vec![
            "agents".to_string(),
            "cursor".to_string(),
            "cline".to_string(),
        ];
        let ts = expand_targets(&ids, &Scope::Project(PathBuf::from("/p")), &cfg).unwrap();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].agent, "agents");
        // 全局 scope 下 cursor 与 cline 目录不同，不去重
        let ts = expand_targets(&ids, &Scope::Global, &cfg).unwrap();
        assert_eq!(ts.len(), 3);
    }

    #[test]
    fn expand_skips_global_unsupported() {
        // eve 无 global_dir：expand 时跳过而不是报错（UI 层已过滤，双保险）
        let cfg = Config::default();
        let ts = expand_targets(&["eve".to_string()], &Scope::Global, &cfg).unwrap();
        assert!(ts.is_empty());
    }

    #[test]
    fn needs_scope_prompt_only_when_some_global_capable() {
        assert!(needs_scope_prompt(&["claude-code"]));
        assert!(!needs_scope_prompt(&["eve"])); // 全都不支持全局则不问
    }
}
