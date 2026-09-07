//! add / fav install 的四步交互向导：选技能 → 选 agent → 选 scope → 选安装方式。
//! CLI 与 TUI 共用（两者都在普通终端模式下用 inquire）；Web 无弹窗，走 /api 参数。
//! 纯决策函数（expand_targets 等）与弹窗函数分离，前者可单测。

use crate::core::agents;
use crate::core::config::Config;
use crate::core::error::{Error, Result};
use crate::core::paths::{Scope, Target};
use crate::core::registry::Method;
use inquire::error::InquireError;
// 仅测试用到（生产代码经 Scope::Project 携带 PathBuf，不直接引用）
#[cfg(test)]
use std::path::PathBuf;

/// 技能选项：多选行显示 "name — description"。
#[derive(Clone)]
pub struct SkillChoice {
    pub name: String,
    pub description: String,
}

impl std::fmt::Display for SkillChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.description.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{} — {}", self.name, self.description)
        }
    }
}

/// scope 选项（Display 即说明文案，返回后按枚举匹配，不做字符串解析）。
enum ScopeChoice {
    Project,
    Global,
}

impl std::fmt::Display for ScopeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeChoice::Project => write!(f, "项目（当前目录，跟随项目提交）"),
            ScopeChoice::Global => write!(f, "全局（用户主目录，跨项目可用）"),
        }
    }
}

enum MethodChoice {
    Symlink,
    Copy,
}

impl std::fmt::Display for MethodChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MethodChoice::Symlink => write!(f, "符号链接（指向缓存，单一来源易更新，推荐）"),
            MethodChoice::Copy => write!(f, "独立副本（脱离缓存，可单独控制更新）"),
        }
    }
}

/// 统一取消语义：ESC/Ctrl+C → Ok(None)（调用方视为「已取消」，退出码 0）；
/// 其余错误（非 TTY、IO 等）照旧向上抛。
fn cancel_to_none<T>(r: std::result::Result<T, InquireError>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(Error::Msg(e.to_string())),
    }
}

/// 技能多选：打字过滤、→ 全选、← 全清；单技能直接返回它，不弹窗。
/// 返回 None = 用户 ESC/Ctrl+C 取消。
pub fn pick_skills(skills: &[SkillChoice]) -> Result<Option<Vec<String>>> {
    if skills.len() == 1 {
        return Ok(Some(vec![skills[0].name.clone()]));
    }
    let r = inquire::MultiSelect::new("选择要安装的技能", skills.to_vec())
        .with_help_message("输入过滤，空格勾选，→ 全选，← 全清，回车确认，ESC 取消")
        .with_page_size(20)
        .with_validator(inquire::min_length!(1, "至少选择一个技能"))
        .prompt();
    cancel_to_none(r).map(|o| o.map(|picked| picked.into_iter().map(|c| c.name).collect()))
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
/// 返回 None = 用户 ESC/Ctrl+C 取消。
pub fn pick_scope(agent_ids: &[String]) -> Result<Option<Scope>> {
    let ids: Vec<&str> = agent_ids.iter().map(|s| s.as_str()).collect();
    if !needs_scope_prompt(&ids) {
        return Ok(Some(Scope::Project(std::env::current_dir()?)));
    }
    let r = inquire::Select::new("安装范围", vec![ScopeChoice::Project, ScopeChoice::Global])
        .with_starting_cursor(0)
        .prompt();
    match cancel_to_none(r)? {
        Some(ScopeChoice::Project) => Ok(Some(Scope::Project(std::env::current_dir()?))),
        Some(ScopeChoice::Global) => Ok(Some(Scope::Global)),
        None => Ok(None),
    }
}

/// 安装方式单选：符号链接（默认，推荐）/ 独立副本。
/// 返回 None = 用户 ESC/Ctrl+C 取消。
pub fn pick_method() -> Result<Option<Method>> {
    let r = inquire::Select::new("安装方式", vec![MethodChoice::Symlink, MethodChoice::Copy])
        .with_starting_cursor(0)
        .prompt();
    cancel_to_none(r).map(|o| {
        o.map(|c| match c {
            MethodChoice::Symlink => Method::Symlink,
            MethodChoice::Copy => Method::Copy,
        })
    })
}

/// 选中的 agent × scope 展开为 Target 列表：按解析后的 install_dir 去重（无论 scope——
/// 项目级多个 universal agent 同目录只装一份；全局级 agents/cline/zed 等共享
/// ~/.agents/skills，不去重会触发同目录重复安装 Conflict；记录保留排序最前的 id）；
/// 跳过 scope 不支持的 agent（如 eve 无全局目录）。
/// 先排序再展开去重，保证结果确定性。
pub fn expand_targets(agent_ids: &[String], scope: &Scope, cfg: &Config) -> Result<Vec<Target>> {
    let mut ids = agent_ids.to_vec();
    ids.sort();
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
        if seen.insert(dir) {
            out.push(t);
        }
    }
    Ok(out)
}

/// 是否需要弹 scope：至少一个选中 agent 支持全局。
/// 用空 overrides 判断：仅 config 自定义 target 的 id 在此视为不支持全局——这是已知
/// 可接受边界：不弹窗的路径（-g/-t/-y/非交互）都显式给了 scope，自定义 agent（本就
/// 只支持全局）的用户可用 -t 或 -a + -g 到达全局安装。
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
    fn single_skill_skips_prompt() {
        let skills = vec![SkillChoice {
            name: "alpha".to_string(),
            description: "A 技能".to_string(),
        }];
        assert_eq!(
            pick_skills(&skills).unwrap(),
            Some(vec!["alpha".to_string()])
        );
    }

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
        // 全局 scope 同样去重：agents 与 cline 共享 ~/.agents/skills，只留一个
        let ts = expand_targets(
            &["agents".to_string(), "cline".to_string()],
            &Scope::Global,
            &cfg,
        )
        .unwrap();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].agent, "agents");
        // 全局目录互不相同的 agent 不去重
        let ids = vec![
            "claude-code".to_string(),
            "codex".to_string(),
            "cursor".to_string(),
        ];
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
