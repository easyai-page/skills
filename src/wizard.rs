//! add / fav install 的四步交互向导：选技能 → 选 agent → 选 scope → 选安装方式。
//! CLI 与 TUI 共用（两者都在普通终端模式下用 inquire）；Web 无弹窗，走 /api 参数。
//! 纯决策函数（expand_targets 等）与弹窗函数分离，前者可单测。

use crate::core::agents;
use crate::core::config::Config;
use crate::core::error::{Error, Result};
use crate::core::paths::{Scope, Target};
use crate::core::registry::Method;
use crossterm::{
    cursor,
    event::{self, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute, terminal,
};
use inquire::error::InquireError;
use std::io::{IsTerminal, Write as IoWrite, stdout};
// 仅测试用到（生产代码经 Scope::Project 携带 PathBuf，不直接引用）
#[cfg(test)]
use std::path::PathBuf;

/// 技能选项：多选行显示 "name — description"，支持插件/目录分组与已安装标注。
#[derive(Clone, Debug)]
pub struct SkillChoice {
    pub name: String,
    pub description: String,
    pub group: Option<String>,
    pub installed: bool,
}

impl std::fmt::Display for SkillChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let tag = if self.installed { " (已安装)" } else { "" };
        if self.description.is_empty() {
            write!(f, "{}{tag}", self.name)
        } else {
            write!(f, "{}{tag} — {}", self.name, self.description)
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
pub(crate) fn cancel_to_none<T>(r: std::result::Result<T, InquireError>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(Error::Msg(e.to_string())),
    }
}

#[derive(Clone, Debug)]
pub struct TreeItem {
    pub id: String,
    pub label: String,
    pub description: String,
    pub group: Option<String>,
    pub selected: bool,
}

#[derive(Clone, Debug)]
pub struct TreeGroup {
    pub name: String,
    pub collapsed: bool,
    pub item_indices: Vec<usize>,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CheckState {
    None,
    Partial,
    All,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VisibleRow {
    SelectAll,
    Group {
        group_idx: usize,
    },
    Item {
        item_idx: usize,
        is_last_in_group: bool,
    },
}

pub struct TreeSelectState {
    pub items: Vec<TreeItem>,
    pub groups: Vec<TreeGroup>,
    pub cursor: usize,
}

impl TreeSelectState {
    pub fn new(items: Vec<TreeItem>) -> Self {
        let mut group_names = Vec::new();
        for it in &items {
            if let Some(g) = &it.group
                && !group_names.contains(g)
            {
                group_names.push(g.clone());
            }
        }
        if let Some(pos) = group_names
            .iter()
            .position(|g| g == "Other" || g == "其它内置 Agent")
        {
            let o = group_names.remove(pos);
            group_names.push(o);
        }

        let mut groups = Vec::new();
        for g_name in group_names {
            let item_indices: Vec<usize> = items
                .iter()
                .enumerate()
                .filter(|(_, it)| it.group.as_deref() == Some(&g_name))
                .map(|(idx, _)| idx)
                .collect();
            groups.push(TreeGroup {
                name: g_name,
                collapsed: false,
                item_indices,
            });
        }

        Self {
            items,
            groups,
            cursor: 0,
        }
    }

    pub fn has_groups(&self) -> bool {
        !self.groups.is_empty()
    }

    pub fn select_all_state(&self) -> CheckState {
        let total = self.items.len();
        if total == 0 {
            return CheckState::None;
        }
        let sel = self.items.iter().filter(|i| i.selected).count();
        if sel == 0 {
            CheckState::None
        } else if sel == total {
            CheckState::All
        } else {
            CheckState::Partial
        }
    }

    pub fn group_state(&self, group_idx: usize) -> CheckState {
        let group = &self.groups[group_idx];
        if group.item_indices.is_empty() {
            return CheckState::None;
        }
        let sel = group
            .item_indices
            .iter()
            .filter(|&&idx| self.items[idx].selected)
            .count();
        if sel == 0 {
            CheckState::None
        } else if sel == group.item_indices.len() {
            CheckState::All
        } else {
            CheckState::Partial
        }
    }

    pub fn visible_rows(&self) -> Vec<VisibleRow> {
        let mut rows = vec![VisibleRow::SelectAll];
        if self.has_groups() {
            for (g_idx, group) in self.groups.iter().enumerate() {
                rows.push(VisibleRow::Group { group_idx: g_idx });
                if !group.collapsed {
                    let total_in_grp = group.item_indices.len();
                    for (i, &item_idx) in group.item_indices.iter().enumerate() {
                        rows.push(VisibleRow::Item {
                            item_idx,
                            is_last_in_group: i + 1 == total_in_grp,
                        });
                    }
                }
            }
        } else {
            let total = self.items.len();
            for i in 0..total {
                rows.push(VisibleRow::Item {
                    item_idx: i,
                    is_last_in_group: i + 1 == total,
                });
            }
        }
        rows
    }

    pub fn move_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn move_down(&mut self) {
        let max_idx = self.visible_rows().len().saturating_sub(1);
        if self.cursor < max_idx {
            self.cursor += 1;
        }
    }

    pub fn collapse(&mut self) {
        let rows = self.visible_rows();
        if self.cursor >= rows.len() {
            return;
        }
        match rows[self.cursor] {
            VisibleRow::Group { group_idx } => {
                self.groups[group_idx].collapsed = true;
            }
            VisibleRow::Item { item_idx, .. } => {
                if let Some(ref grp_name) = self.items[item_idx].group
                    && let Some(g_idx) = self.groups.iter().position(|g| &g.name == grp_name)
                {
                    self.groups[g_idx].collapsed = true;
                    let new_rows = self.visible_rows();
                    if let Some(pos) = new_rows.iter().position(
                        |r| matches!(r, VisibleRow::Group { group_idx: idx } if *idx == g_idx),
                    ) {
                        self.cursor = pos;
                    }
                }
            }
            VisibleRow::SelectAll => {}
        }
    }

    pub fn expand(&mut self) {
        let rows = self.visible_rows();
        if self.cursor >= rows.len() {
            return;
        }
        if let VisibleRow::Group { group_idx } = rows[self.cursor] {
            self.groups[group_idx].collapsed = false;
        }
    }

    pub fn toggle_current(&mut self) {
        let rows = self.visible_rows();
        if self.cursor >= rows.len() {
            return;
        }
        match rows[self.cursor] {
            VisibleRow::SelectAll => {
                let should_clear = self.select_all_state() == CheckState::All;
                for item in &mut self.items {
                    item.selected = !should_clear;
                }
            }
            VisibleRow::Group { group_idx } => {
                let should_clear = self.group_state(group_idx) == CheckState::All;
                for &idx in &self.groups[group_idx].item_indices {
                    self.items[idx].selected = !should_clear;
                }
            }
            VisibleRow::Item { item_idx, .. } => {
                self.items[item_idx].selected = !self.items[item_idx].selected;
            }
        }
    }

    pub fn current_description(&self) -> String {
        let rows = self.visible_rows();
        if self.cursor >= rows.len() {
            return String::new();
        }
        match rows[self.cursor] {
            VisibleRow::SelectAll => {
                format!("选择或清空全部 {} 个选项。", self.items.len())
            }
            VisibleRow::Group { group_idx } => {
                let g = &self.groups[group_idx];
                format!(
                    "选择或清空「{}」分类下的全部 {} 个条目。",
                    g.name,
                    g.item_indices.len()
                )
            }
            VisibleRow::Item { item_idx, .. } => {
                let it = &self.items[item_idx];
                if it.description.is_empty() {
                    it.label.clone()
                } else {
                    it.description.clone()
                }
            }
        }
    }

    pub fn selected_ids(&self) -> Vec<String> {
        self.items
            .iter()
            .filter(|i| i.selected)
            .map(|i| i.id.clone())
            .collect()
    }
}

struct RawModeGuard {
    active: bool,
}

impl RawModeGuard {
    fn new() -> Result<Self> {
        terminal::enable_raw_mode()
            .map_err(|e| Error::Msg(format!("无法启用终端 raw 模式: {e}")))?;
        let mut out = stdout();
        let _ = execute!(out, cursor::Hide);
        Ok(Self { active: true })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.active {
            let mut out = stdout();
            let _ = execute!(out, cursor::Show);
            let _ = terminal::disable_raw_mode();
        }
    }
}

fn render_tree_prompt(
    title: &str,
    state: &TreeSelectState,
    max_visible: usize,
    warning: Option<&str>,
    term_width: usize,
) -> Vec<String> {
    let mut lines = Vec::new();

    lines.push(format!(
        "{}  {}",
        console::style("◆").green(),
        console::style(title).bold()
    ));

    let rail = console::style("│").dim();
    lines.push(format!("{rail}"));

    let rows = state.visible_rows();
    let is_cur_sel_all = state.cursor == 0;
    let prefix = if is_cur_sel_all {
        console::style("❯").cyan().to_string()
    } else {
        " ".to_string()
    };
    let all_icon = match state.select_all_state() {
        CheckState::All => console::style("●").green().to_string(),
        CheckState::Partial => console::style("◐").yellow().to_string(),
        CheckState::None => console::style("○").dim().to_string(),
    };
    let sel_count = state.items.iter().filter(|i| i.selected).count();
    let sel_all_label = if is_cur_sel_all {
        console::style("全选 (Select All)")
            .underlined()
            .bold()
            .to_string()
    } else {
        console::style("全选 (Select All)").bold().to_string()
    };
    lines.push(format!(
        "{rail} {prefix} {all_icon} {sel_all_label} {}",
        console::style(format!("({}/{})", sel_count, state.items.len())).dim()
    ));
    lines.push(format!("{rail}   {}", console::style("─".repeat(36)).dim()));

    let content_len = rows.len().saturating_sub(1);
    if content_len > 0 {
        let (window_start, window_end) = if content_len <= max_visible {
            (0, content_len)
        } else {
            let cur_item = state.cursor.saturating_sub(1);
            let half = max_visible / 2;
            let start = if cur_item > half {
                (cur_item - half).min(content_len.saturating_sub(max_visible))
            } else {
                0
            };
            let end = (start + max_visible).min(content_len);
            (start, end)
        };

        if window_start > 0 {
            lines.push(format!(
                "{rail}  {}",
                console::style(format!("↑ 还有 {} 项", window_start)).dim()
            ));
        }

        for idx in window_start..window_end {
            let row_idx = idx + 1;
            let is_cur = state.cursor == row_idx;
            let pfx = if is_cur {
                console::style("❯").cyan().to_string()
            } else {
                " ".to_string()
            };

            match rows[row_idx] {
                VisibleRow::SelectAll => unreachable!(),
                VisibleRow::Group { group_idx } => {
                    let grp = &state.groups[group_idx];
                    let arrow = if grp.collapsed {
                        console::style("▸").dim().to_string()
                    } else {
                        console::style("▾").dim().to_string()
                    };
                    let grp_icon = match state.group_state(group_idx) {
                        CheckState::All => console::style("●").green().to_string(),
                        CheckState::Partial => console::style("◐").yellow().to_string(),
                        CheckState::None => console::style("○").dim().to_string(),
                    };
                    let grp_sel = grp
                        .item_indices
                        .iter()
                        .filter(|&&i| state.items[i].selected)
                        .count();
                    let name_styled = if is_cur {
                        console::style(&grp.name).underlined().bold().to_string()
                    } else {
                        console::style(&grp.name).bold().to_string()
                    };
                    lines.push(format!(
                        "{rail} {pfx} {arrow} {grp_icon} {name_styled} {}",
                        console::style(format!("({}/{})", grp_sel, grp.item_indices.len())).dim()
                    ));
                }
                VisibleRow::Item {
                    item_idx,
                    is_last_in_group,
                } => {
                    let it = &state.items[item_idx];
                    let tree = if state.has_groups() {
                        if is_last_in_group {
                            console::style("└─ ").dim().to_string()
                        } else {
                            console::style("├─ ").dim().to_string()
                        }
                    } else {
                        String::new()
                    };
                    let it_icon = if it.selected {
                        console::style("●").green().to_string()
                    } else {
                        console::style("○").dim().to_string()
                    };
                    let label_styled = if is_cur {
                        console::style(&it.label).underlined().to_string()
                    } else {
                        it.label.clone()
                    };
                    lines.push(format!("{rail} {pfx} {tree}{it_icon} {label_styled}"));
                }
            }
        }

        if window_end < content_len {
            lines.push(format!(
                "{rail}  {}",
                console::style(format!("↓ 还有 {} 项", content_len - window_end)).dim()
            ));
        }
    }

    lines.push(format!("{rail}"));
    lines.push(format!("{rail}  {}", console::style("说明").dim()));
    let desc = state.current_description();
    let max_desc_width = term_width.saturating_sub(6).max(20);
    let truncated_desc = console::truncate_str(&desc, max_desc_width, "…");
    lines.push(format!("{rail}  {}", console::style(truncated_desc).dim()));

    lines.push(format!("{rail}"));
    if let Some(w) = warning {
        lines.push(format!(
            "{rail}  {}",
            console::style(format!("⚠️ {w}")).yellow().bold()
        ));
    }
    let help = if state.has_groups() {
        "↑↓ 移动，←→ 折叠/展开，空格 勾选，回车 确认，ESC 取消"
    } else {
        "↑↓ 移动，空格 勾选，回车 确认，ESC 取消"
    };
    lines.push(format!("{rail}  {}", console::style(help).dim()));
    lines.push(format!("{}", console::style("└").dim()));

    lines
}

fn run_tree_select(title: &str, mut state: TreeSelectState) -> Result<Option<Vec<String>>> {
    let _guard = RawModeGuard::new()?;
    let mut last_render_height = 0;
    let mut warning = None;
    let mut stdout = stdout();

    loop {
        let (term_cols, term_rows) = terminal::size().unwrap_or((80, 24));
        let max_visible = (term_rows as usize).saturating_sub(12).clamp(5, 18);

        let lines = render_tree_prompt(title, &state, max_visible, warning, term_cols as usize);
        let height = lines.len();

        let mut buffer = String::new();
        if last_render_height > 0 {
            buffer.push_str(&format!("\x1b[{}A\x1b[J", last_render_height));
        }
        for line in &lines {
            buffer.push_str(line);
            buffer.push('\r');
            buffer.push('\n');
        }
        stdout
            .write_all(buffer.as_bytes())
            .map_err(|e| Error::Msg(e.to_string()))?;
        stdout.flush().map_err(|e| Error::Msg(e.to_string()))?;
        last_render_height = height;

        if let Event::Key(key) = event::read().map_err(|e| Error::Msg(e.to_string()))? {
            if key.kind != KeyEventKind::Press {
                continue;
            }
            warning = None;

            match (key.code, key.modifiers) {
                (KeyCode::Char('c'), KeyModifiers::CONTROL) | (KeyCode::Esc, _) => {
                    if last_render_height > 0 {
                        let clear = format!("\x1b[{}A\x1b[J", last_render_height);
                        let _ = stdout.write_all(clear.as_bytes());
                        let _ = stdout.flush();
                    }
                    return Ok(None);
                }
                (KeyCode::Up, _) => {
                    state.move_up();
                }
                (KeyCode::Down, _) => {
                    state.move_down();
                }
                (KeyCode::Left, _) => {
                    state.collapse();
                }
                (KeyCode::Right, _) => {
                    state.expand();
                }
                (KeyCode::Char(' '), _) => {
                    state.toggle_current();
                }
                (KeyCode::Enter, _) => {
                    let selected = state.selected_ids();
                    if selected.is_empty() {
                        warning = Some("至少选择一项");
                    } else {
                        if last_render_height > 0 {
                            let clear = format!("\x1b[{}A\x1b[J", last_render_height);
                            let _ = stdout.write_all(clear.as_bytes());
                            let _ = stdout.flush();
                        }
                        return Ok(Some(selected));
                    }
                }
                _ => {}
            }
        }
    }
}

/// 将技能选项转换为树状多选条目，并应用智能预选规则：
/// 1. 若存在未安装项：默认勾选所有未安装项，已安装项不勾选；
/// 2. 若全部已安装（或全部未安装）：默认全选；
/// 3. 已安装的项在 label 尾部标注 " (已安装)"。
pub fn build_skill_tree_items(skills: &[SkillChoice]) -> Vec<TreeItem> {
    let has_uninstalled = skills.iter().any(|s| !s.installed);
    skills
        .iter()
        .map(|s| {
            let selected = if has_uninstalled { !s.installed } else { true };
            let label = if s.installed {
                format!("{} (已安装)", s.name)
            } else {
                s.name.clone()
            };
            TreeItem {
                id: s.name.clone(),
                label,
                description: s.description.clone(),
                group: s.group.clone(),
                selected,
            }
        })
        .collect()
}

/// 技能多选：树状层级、折叠展开、Select All、组全选；单技能直接返回它，不弹窗。
/// 返回 None = 用户 ESC/Ctrl+C 取消。
pub fn pick_skills(skills: &[SkillChoice]) -> Result<Option<Vec<String>>> {
    if skills.is_empty() {
        return Ok(Some(Vec::new()));
    }
    if skills.len() == 1 {
        return Ok(Some(vec![skills[0].name.clone()]));
    }
    if !std::io::stdin().is_terminal() {
        return Ok(Some(skills.iter().map(|s| s.name.clone()).collect()));
    }
    let items = build_skill_tree_items(skills);
    let state = TreeSelectState::new(items);
    run_tree_select("选择要安装的技能", state)
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

/// agent 多选：恰好检测到 1 个时不弹窗直接用；否则树状分组多选（已检测展开，其他折叠），
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

    if !std::io::stdin().is_terminal() {
        return Ok(Some(want));
    }

    let items = specs
        .iter()
        .map(|a| {
            let is_detected = detected.contains(&a.id);
            let mark = if is_detected { "  ✓已检测" } else { "" };
            let group_name = if is_detected {
                "已检测到的 Agent".to_string()
            } else {
                "其它内置 Agent".to_string()
            };
            TreeItem {
                id: a.id.to_string(),
                label: format!("{}（{}）→ {}{}", a.id, a.name, a.project_dir, mark),
                description: format!(
                    "项目目录: {} | 默认全局: {}",
                    a.project_dir,
                    a.global_dir.unwrap_or("无全局目录")
                ),
                group: Some(group_name),
                selected: want.iter().any(|w| w == a.id),
            }
        })
        .collect();

    let mut state = TreeSelectState::new(items);
    // 将未检测到的 70+ 个 agent 默认折叠收起，避免刷屏；已检测到的置顶展开
    for g in &mut state.groups {
        if g.name == "其它内置 Agent" {
            g.collapsed = true;
        }
    }
    run_tree_select("安装到哪些 agent", state)
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
            group: None,
            installed: false,
        }];
        assert_eq!(
            pick_skills(&skills).unwrap(),
            Some(vec!["alpha".to_string()])
        );
    }

    #[test]
    fn skill_tree_preselection_rules() {
        // 场景 1：部分已安装，部分未安装 -> 预选所有未安装项，已安装项不预选
        let choices = [
            SkillChoice {
                name: "s1".into(),
                description: "d1".into(),
                group: None,
                installed: true,
            },
            SkillChoice {
                name: "s2".into(),
                description: "d2".into(),
                group: None,
                installed: false,
            },
        ];
        let items = build_skill_tree_items(&choices);
        assert_eq!(items.len(), 2);
        assert!(!items[0].selected);
        assert!(items[0].label.contains("(已安装)"));
        assert!(items[1].selected);
        assert!(!items[1].label.contains("(已安装)"));

        // 场景 2：全部未安装 -> 默认全选
        let all_uninstalled = [
            SkillChoice {
                name: "s1".into(),
                description: "d1".into(),
                group: None,
                installed: false,
            },
            SkillChoice {
                name: "s2".into(),
                description: "d2".into(),
                group: None,
                installed: false,
            },
        ];
        let items2 = build_skill_tree_items(&all_uninstalled);
        assert!(items2.iter().all(|i| i.selected));
        assert!(items2.iter().all(|i| !i.label.contains("(已安装)")));

        // 场景 3：全部已安装 -> 默认全选（供用户整体覆盖更新）
        let all_installed = [
            SkillChoice {
                name: "s1".into(),
                description: "d1".into(),
                group: None,
                installed: true,
            },
            SkillChoice {
                name: "s2".into(),
                description: "d2".into(),
                group: None,
                installed: true,
            },
        ];
        let items3 = build_skill_tree_items(&all_installed);
        assert!(items3.iter().all(|i| i.selected));
        assert!(items3.iter().all(|i| i.label.contains("(已安装)")));
    }

    #[test]
    fn tree_select_all_toggle_and_group_cascade() {
        let choices = [
            SkillChoice {
                name: "s1".into(),
                description: "d1".into(),
                group: Some("G1".into()),
                installed: false,
            },
            SkillChoice {
                name: "s2".into(),
                description: "d2".into(),
                group: Some("G1".into()),
                installed: false,
            },
            SkillChoice {
                name: "s3".into(),
                description: "d3".into(),
                group: Some("G2".into()),
                installed: false,
            },
        ];
        let items = choices
            .iter()
            .map(|c| TreeItem {
                id: c.name.clone(),
                label: c.name.clone(),
                description: c.description.clone(),
                group: c.group.clone(),
                selected: false,
            })
            .collect();
        let mut state = TreeSelectState::new(items);

        assert_eq!(state.select_all_state(), CheckState::None);
        assert_eq!(state.group_state(0), CheckState::None);
        assert_eq!(state.group_state(1), CheckState::None);

        // 光标在 Select All (0)，按空格全选
        assert_eq!(state.cursor, 0);
        state.toggle_current();
        assert_eq!(state.select_all_state(), CheckState::All);
        assert_eq!(state.group_state(0), CheckState::All);
        assert_eq!(state.group_state(1), CheckState::All);
        assert_eq!(state.selected_ids().len(), 3);

        // 再次按空格，全部清空
        state.toggle_current();
        assert_eq!(state.select_all_state(), CheckState::None);
        assert_eq!(state.selected_ids().len(), 0);

        // 移到 G1 分组行 (索引 1)，按空格全选 G1
        state.move_down();
        assert_eq!(state.cursor, 1);
        state.toggle_current();
        assert_eq!(state.group_state(0), CheckState::All);
        assert_eq!(state.group_state(1), CheckState::None);
        assert_eq!(state.select_all_state(), CheckState::Partial);
        assert_eq!(state.selected_ids(), vec!["s1", "s2"]);

        // 移到 s1 (索引 2)，按空格取消勾选
        state.move_down();
        assert_eq!(state.cursor, 2);
        state.toggle_current();
        assert_eq!(state.group_state(0), CheckState::Partial);
        assert_eq!(state.selected_ids(), vec!["s2"]);
    }

    #[test]
    fn tree_collapse_and_expand_from_child() {
        let choices = [
            SkillChoice {
                name: "s1".into(),
                description: "d1".into(),
                group: Some("G1".into()),
                installed: false,
            },
            SkillChoice {
                name: "s2".into(),
                description: "d2".into(),
                group: Some("G2".into()),
                installed: false,
            },
        ];
        let items = choices
            .iter()
            .map(|c| TreeItem {
                id: c.name.clone(),
                label: c.name.clone(),
                description: c.description.clone(),
                group: c.group.clone(),
                selected: false,
            })
            .collect();
        let mut state = TreeSelectState::new(items);

        // 初始展开：SelectAll (0), G1 (1), s1 (2), G2 (3), s2 (4) -> 5 行
        assert_eq!(state.visible_rows().len(), 5);

        // 光标移动到 s1 (2)，按左键折叠其所属组 G1，光标应跳到 G1 (1)
        state.cursor = 2;
        state.collapse();
        assert!(state.groups[0].collapsed);
        assert_eq!(state.cursor, 1);
        // 折叠后可见行：SelectAll (0), G1 (1), G2 (2), s2 (3) -> 4 行
        assert_eq!(state.visible_rows().len(), 4);

        // 在 G1 (1) 上按右键展开
        state.expand();
        assert!(!state.groups[0].collapsed);
        assert_eq!(state.visible_rows().len(), 5);
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
