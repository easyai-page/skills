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

/// agent 选项：行内显示 id（名称）→ 项目目录，检测到的带标记。
pub struct AgentChoice {
    pub id: &'static str,
    pub name: &'static str,
    pub project_dir: &'static str,
    pub detected: bool,
}

impl std::fmt::Display for AgentChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mark = if self.detected { "  ✓已检测" } else { "" };
        write!(
            f,
            "{}（{}）→ {}{}",
            self.id, self.name, self.project_dir, mark
        )
    }
}

/// 预选决策：上次选择的记忆（过滤已失效 id）> 检测到的 > ["agents"]。
pub fn agent_defaults(detected: &[&str], last_selected: &[String]) -> Vec<String> {
    let remembered: Vec<String> = last_selected
        .iter()
        .filter(|id| agents::all().iter().any(|a| a.id == id.as_str()))
        .cloned()
        .collect();
    if !remembered.is_empty() {
        return remembered;
    }
    if !detected.is_empty() {
        return detected.iter().map(|s| s.to_string()).collect();
    }
    vec!["agents".to_string()]
}

/// 列表排序：检测到的置顶（两组各自保持注册表的 id 字典序）。
pub fn detected_first(detected: &[&str]) -> Vec<&'static agents::AgentSpec> {
    let mut out: Vec<_> = agents::all()
        .iter()
        .filter(|a| detected.contains(&a.id))
        .collect();
    out.extend(agents::all().iter().filter(|a| !detected.contains(&a.id)));
    out
}

/// agent 多选：恰好检测到 1 个时不弹窗直接用；否则搜索式多选，
/// 预选 = agent_defaults（记忆 > 检测 > agents）。返回 None = 取消。
/// 只列内置 agent（config 自定义 target 无法检测本机安装，走 -a 参数指定）。
pub fn pick_agents(
    detected: &[&'static str],
    last_selected: &[String],
) -> Result<Option<Vec<String>>> {
    if let [only] = detected {
        let name = agents::all()
            .iter()
            .find(|a| a.id == *only)
            .map(|a| a.name)
            .unwrap_or(only);
        println!("安装到：{name}（已检测）");
        return Ok(Some(vec![only.to_string()]));
    }
    let specs = detected_first(detected);
    let want = agent_defaults(detected, last_selected);
    let default_idx: Vec<usize> = specs
        .iter()
        .enumerate()
        .filter(|(_, a)| want.iter().any(|w| w == a.id))
        .map(|(i, _)| i)
        .collect();
    let options: Vec<AgentChoice> = specs
        .iter()
        .map(|a| AgentChoice {
            id: a.id,
            name: a.name,
            project_dir: a.project_dir,
            detected: detected.contains(&a.id),
        })
        .collect();
    let r = inquire::MultiSelect::new("安装到哪些 agent", options)
        .with_default(&default_idx)
        .with_help_message("输入过滤，空格勾选，→ 全选，← 全清，回车确认，ESC 取消")
        .with_page_size(20)
        .with_validator(inquire::min_length!(1, "至少选择一个 agent"))
        .prompt();
    cancel_to_none(r).map(|o| o.map(|cs| cs.into_iter().map(|c| c.id.to_string()).collect()))
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

    #[test]
    fn agent_defaults_priority() {
        // 记忆优先，且过滤已失效 id
        assert_eq!(
            agent_defaults(
                &["codex"],
                &["claude-code".to_string(), "ghost".to_string()]
            ),
            vec!["claude-code".to_string()]
        );
        // 无有效记忆 → 检测到的
        assert_eq!(agent_defaults(&["codex"], &[]), vec!["codex".to_string()]);
        // 全无 → universal 兜底
        assert_eq!(agent_defaults(&[], &[]), vec!["agents".to_string()]);
    }

    #[test]
    fn detected_first_ordering() {
        let specs = detected_first(&["zed"]);
        assert_eq!(specs[0].id, "zed", "检测到的必须置顶");
        assert!(
            specs[1..].windows(2).all(|w| w[0].id < w[1].id),
            "未检测组保持 id 字典序"
        );
    }
}
