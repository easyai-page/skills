pub mod app;
pub mod ui;

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;

use crate::cli::ui as cli_ui;
use crate::core::{
    agents, cache,
    config::Config,
    error::{Error, Result},
    favorites, install,
    paths::Layout,
    registry::{Method, Registry},
    remove,
    source::parse_source,
    update,
};
use crate::wizard;
use app::{Action, AppState, FavRow, View};

/// RAII 终端守卫：进入 raw mode + Alternate Screen，
/// Drop 时无条件恢复（事件循环出错或 panic unwind 均会恢复终端）。
struct TermGuard {
    active: bool,
}

impl TermGuard {
    fn enter() -> Result<TermGuard> {
        crossterm::terminal::enable_raw_mode()?;
        // 半路失败（raw mode 已成但 Alternate Screen 失败）时恢复 raw mode，
        // 避免把用户终端留在 raw 状态。
        if let Err(e) = crossterm::execute!(std::io::stdout(), EnterAlternateScreen) {
            let _ = crossterm::terminal::disable_raw_mode();
            return Err(e.into());
        }
        Ok(TermGuard { active: true })
    }

    /// 暂时恢复正常终端（跑 inquire 向导用）；幂等。
    fn suspend(&mut self) {
        if self.active {
            let _ = crossterm::terminal::disable_raw_mode();
            let _ = crossterm::execute!(std::io::stdout(), LeaveAlternateScreen);
            self.active = false;
        }
    }

    /// 从 suspend 状态重新进入 TUI 终端模式；幂等。
    fn resume(&mut self) -> Result<()> {
        if !self.active {
            crossterm::terminal::enable_raw_mode()?;
            crossterm::execute!(std::io::stdout(), EnterAlternateScreen)?;
            self.active = true;
        }
        Ok(())
    }
}

impl Drop for TermGuard {
    fn drop(&mut self) {
        self.suspend();
    }
}

pub fn run(layout: &Layout, cfg: &Config) -> Result<()> {
    let reg = Registry::load(layout)?;
    let mut app = AppState::new(reg);
    let mut guard = TermGuard::enter()?;
    let backend = CrosstermBackend::new(std::io::stdout());
    let mut term = ratatui::Terminal::new(backend)?;
    let res = event_loop(&mut term, &mut guard, layout, cfg, &mut app);
    // 正常/错误路径都先显式恢复终端（panic 路径由 Drop 兜底），再传播结果。
    guard.suspend();
    // 错误路径（res? 提前返回）有意跳过退出落盘：此时事件循环已异常，
    // 内存 registry 状态可信度低，保留磁盘上最后一次一致快照更安全。
    res?;
    app.registry.save(layout)?; // 退出时落盘（auto_update 切换等）
    Ok(())
}

fn event_loop(
    term: &mut ratatui::Terminal<CrosstermBackend<std::io::Stdout>>,
    guard: &mut TermGuard,
    layout: &Layout,
    cfg: &Config,
    app: &mut AppState,
) -> Result<()> {
    loop {
        term.draw(|f| ui::draw(f, app))?;
        if let Event::Key(k) = event::read()? {
            if k.code == KeyCode::Char('i') {
                // i：Installed 等视图=从仓库装；Favorites=从收藏装。向导需正常终端（inquire）。
                guard.suspend();
                let r = if app.view == View::Favorites {
                    fav_install_wizard(layout, cfg, app)
                } else {
                    install_wizard(layout, cfg, app)
                };
                if let Err(e) = r {
                    eprintln!("安装失败: {e}");
                    let _ = inquire::Text::new("按回车返回 TUI").prompt();
                }
                guard.resume()?;
                term.clear()?;
                continue;
            }
            if k.code == KeyCode::Char('f') && app.view == View::Favorites {
                guard.suspend();
                let r = fav_wizard(layout, app);
                if let Err(e) = r {
                    eprintln!("收藏失败: {e}");
                    let _ = inquire::Text::new("按回车返回 TUI").prompt();
                }
                guard.resume()?;
                term.clear()?;
                continue;
            }
            if app.view == View::Installed {
                if k.code == KeyCode::Enter {
                    guard.suspend();
                    let r = installed_action_menu(layout, cfg, app);
                    if let Err(e) = r {
                        eprintln!("操作异常: {e}");
                        let _ = inquire::Text::new("按回车返回 TUI").prompt();
                    }
                    guard.resume()?;
                    term.clear()?;
                    continue;
                }
                if k.code == KeyCode::Char('d') {
                    guard.suspend();
                    let r = confirm_delete_installed(layout, cfg, app);
                    if let Err(e) = r {
                        eprintln!("删除失败: {e}");
                        let _ = inquire::Text::new("按回车返回 TUI").prompt();
                    }
                    guard.resume()?;
                    term.clear()?;
                    continue;
                }
            }
            let action = match k.code {
                KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
                KeyCode::Up | KeyCode::Char('k') => Action::Up,
                KeyCode::Down | KeyCode::Char('j') => Action::Down,
                KeyCode::Tab => Action::NextView,
                KeyCode::BackTab => Action::PrevView,
                KeyCode::Char('a') => Action::ToggleAutoUpdate,
                KeyCode::Char('p') => Action::ToggleShowAll,
                KeyCode::Char('d') => Action::DeleteFav,
                _ => continue,
            };
            if action == Action::Quit {
                break;
            }
            app.reduce(action);
        }
    }
    Ok(())
}

/// 确认并删除选中的已安装技能副本。
fn confirm_delete_installed(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let visible = app.visible_rows();
    let Some(row) = visible.get(app.selected) else {
        return Ok(());
    };
    let skill = row.skill.clone();
    let target = row.target.clone();
    let prompt_text = format!("确认删除 {} @ {}？", skill, target.to_target());
    if !inquire::Confirm::new(&prompt_text)
        .with_default(false)
        .prompt()
        .unwrap_or(false)
    {
        return Ok(());
    }

    match remove::remove_install(layout, cfg, &mut app.registry, &skill, &target) {
        Ok(_) => {
            app.registry.save(layout)?;
            println!("已删除 {skill} @ {}", target.to_target());
            let max = app.visible_rows().len().saturating_sub(1);
            app.selected = app.selected.min(max);
        }
        Err(e) => {
            eprintln!("删除失败: {e}");
        }
    }
    let _ = inquire::Text::new("按回车返回 TUI").prompt();
    Ok(())
}

/// 已安装技能管理动作菜单（回车弹出）。
fn installed_action_menu(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let visible = app.visible_rows();
    let Some(row) = visible.get(app.selected) else {
        return Ok(());
    };
    let skill = row.skill.clone();
    let target = row.target.clone();
    let method = row.method;
    let auto_up = row.auto_update;
    let source = row.source.clone();

    println!("\n▶ 管理技能: {} @ {}", skill, target.to_target());
    println!(
        "  当前方式: {:?} | 自动更新: {} | 来源: {}",
        method,
        match auto_up {
            Some(true) => "开",
            Some(false) => "关",
            None => "跟随包级",
        },
        source
    );

    let options = vec![
        "1. 手动更新 (普通检查更新)",
        "2. 强制覆盖更新 (忽略本地修改)",
        "3. 配置自动更新策略",
        "4. 转换安装方式 (symlink ⇄ copy)",
        "5. 删除该技能副本",
        "0. 返回",
    ];

    let choice = inquire::Select::new("选择操作", options)
        .prompt()
        .unwrap_or("0. 返回");

    match choice {
        s if s.starts_with("1.") => {
            match update::update_single(layout, cfg, &mut app.registry, &skill, &target, false) {
                Ok(done) => {
                    if done.is_empty() {
                        println!("当前已是最新版本");
                    } else {
                        for l in done {
                            println!("{l}");
                        }
                    }
                }
                Err(Error::Mismatch(msg)) => {
                    eprintln!("检测到本地修改:\n{msg}");
                    if inquire::Confirm::new("是否放弃本地修改强制覆盖？")
                        .with_default(false)
                        .prompt()
                        .unwrap_or(false)
                    {
                        match update::update_single(
                            layout,
                            cfg,
                            &mut app.registry,
                            &skill,
                            &target,
                            true,
                        ) {
                            Ok(done) => {
                                for l in done {
                                    println!("{l}");
                                }
                            }
                            Err(e) => eprintln!("强制更新失败: {e}"),
                        }
                    }
                }
                Err(e) => eprintln!("更新失败: {e}"),
            }
            let _ = inquire::Text::new("按回车返回 TUI").prompt();
        }
        s if s.starts_with("2.") => {
            if inquire::Confirm::new("确认强制更新并覆盖可能的本地修改？")
                .with_default(false)
                .prompt()
                .unwrap_or(false)
            {
                match update::update_single(layout, cfg, &mut app.registry, &skill, &target, true) {
                    Ok(done) => {
                        for l in done {
                            println!("{l}");
                        }
                    }
                    Err(e) => eprintln!("强制更新失败: {e}"),
                }
                let _ = inquire::Text::new("按回车返回 TUI").prompt();
            }
        }
        s if s.starts_with("3.") => {
            if method == Method::Copy {
                let policy_opts = vec!["跟随包级", "开启", "关闭"];
                if let Ok(p) =
                    inquire::Select::new("选择该副本的自动更新策略", policy_opts).prompt()
                {
                    let val = match p {
                        "开启" => Some(true),
                        "关闭" => Some(false),
                        _ => None,
                    };
                    if let Some(inst) = app.registry.find_mut(&skill, &target) {
                        inst.auto_update = val;
                    }
                    app.registry.save(layout)?;
                    println!("已保存自动更新策略");
                }
            } else {
                let src_au = app
                    .registry
                    .sources
                    .get(&source)
                    .and_then(|s| s.auto_update)
                    .unwrap_or(false);
                println!(
                    "提示：软链接始终跟随所属仓库 [{source}]（当前包级: {}）",
                    if src_au { "开" } else { "关" }
                );
                if inquire::Confirm::new(&format!("是否切换所属仓库 [{source}] 的自动更新开关？"))
                    .prompt()
                    .unwrap_or(false)
                {
                    if let Some(s) = app.registry.sources.get_mut(&source) {
                        s.auto_update = Some(!src_au);
                    }
                    app.registry.save(layout)?;
                    println!(
                        "已更新仓库自动更新策略为: {}",
                        if !src_au { "开" } else { "关" }
                    );
                }
            }
            let _ = inquire::Text::new("按回车返回 TUI").prompt();
        }
        s if s.starts_with("4.") => {
            let new_m = match method {
                Method::Symlink => Method::Copy,
                Method::Copy => Method::Symlink,
            };
            let prompt = format!("确认从 {:?} 转换为 {:?}？", method, new_m);
            if inquire::Confirm::new(&prompt).prompt().unwrap_or(false) {
                match install::switch_install_method(
                    layout,
                    cfg,
                    &mut app.registry,
                    &skill,
                    &target,
                    new_m,
                    false,
                ) {
                    Ok(_) => println!("已成功切换为 {:?}", new_m),
                    Err(Error::Mismatch(msg)) => {
                        eprintln!("检测到本地修改:\n{msg}");
                        if inquire::Confirm::new("转换为软连接将丢弃本地修改，是否强制转换？")
                            .with_default(false)
                            .prompt()
                            .unwrap_or(false)
                        {
                            match install::switch_install_method(
                                layout,
                                cfg,
                                &mut app.registry,
                                &skill,
                                &target,
                                new_m,
                                true,
                            ) {
                                Ok(_) => println!("已强制切换为 {:?}", new_m),
                                Err(e) => eprintln!("强制切换失败: {e}"),
                            }
                        }
                    }
                    Err(e) => eprintln!("切换失败: {e}"),
                }
                let _ = inquire::Text::new("按回车返回 TUI").prompt();
            }
        }
        s if s.starts_with("5.") => {
            confirm_delete_installed(layout, cfg, app)?;
        }
        _ => {}
    }

    Ok(())
}

/// 安装向导（已在正常终端模式下运行）：选 source → 共享四步向导（技能 → agent → scope → 方式）→ 走 core 安装。
fn install_wizard(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let source = inquire::Text::new("source（github:owner/repo、URL 或本地路径）")
        .prompt()
        .map_err(|e| Error::Msg(e.to_string()))?;
    let spec = parse_source(&source)?;
    let cached = cache::ensure_cached(layout, &spec)?;
    if !cached.fresh {
        println!("已缓存 {}，复用（skills update 可更新）", spec.key);
    }
    let all = cache::scan_skills(&cached.path)?;
    if all.is_empty() {
        return Err(Error::Msg("仓库中没有可安装的技能".into()));
    }
    // 第 1 步：技能（ESC/Ctrl+C = 取消）
    let choices: Vec<wizard::SkillChoice> = all
        .iter()
        .map(|s| wizard::SkillChoice {
            name: s.name.clone(),
            description: s.description.clone(),
        })
        .collect();
    let Some(picked) = wizard::pick_skills(&choices)? else {
        println!("已取消安装");
        return Ok(());
    };
    // 第 2 步：agent（ESC/Ctrl+C = 取消；确认后记忆本次选择）
    let detected = agents::detect_installed();
    let remembered = crate::core::state::load(layout).last_selected_agents;
    let Some(agent_ids) = wizard::pick_agents(&detected, &remembered)? else {
        println!("已取消安装");
        return Ok(());
    };
    crate::core::state::save_last_agents(layout, &agent_ids);
    // 第 3~4 步：scope → method（ESC/Ctrl+C = 取消），展开去重为 targets
    let Some(scope) = wizard::pick_scope(&agent_ids)? else {
        println!("已取消安装");
        return Ok(());
    };
    let Some(method) = wizard::pick_method()? else {
        println!("已取消安装");
        return Ok(());
    };
    let targets = wizard::expand_targets(&agent_ids, &scope, cfg)?;
    if targets.is_empty() {
        println!("已取消安装");
        return Ok(());
    }
    println!(
        "将安装 {} 个技能 → {} 个目标（{}）",
        picked.len(),
        targets.len(),
        cli_ui::method_label(method)
    );

    // 直接操作 TUI 内存中的 registry：用户在 TUI 内按 a 的 auto_update
    // 切换只存在于 app.registry，若从磁盘 load 全新副本再覆盖回去，
    // 未落盘的切换会被静默丢失。
    let reg = &mut app.registry;
    reg.sources
        .entry(spec.key.clone())
        .or_insert(crate::core::registry::SourceRecord {
            url: spec.url.clone().unwrap_or_default(),
            commit: cached.commit.clone(),
            fetched_at: chrono::Utc::now().to_rfc3339(),
            auto_update: None,
        });
    for s in &picked {
        let entry = all.iter().find(|e| &e.name == s).expect("刚校验过存在");
        for target in &targets {
            match install::install_skill(
                layout,
                cfg,
                reg,
                &spec.key,
                &entry.name,
                &entry.rel_path.to_string_lossy(),
                target,
                method,
                &cached.commit,
            ) {
                Ok(_) => println!(
                    "{}",
                    cli_ui::ok(format!(
                        "已安装 {} → {target}（{}）",
                        entry.name,
                        cli_ui::method_label(method)
                    ))
                ),
                Err(Error::Conflict(p)) => {
                    // ESC/Ctrl+C 视为不覆盖（跳过该条），不中断批次（与 CLI install_loop 同款）
                    let prompt = format!("{} 已存在，覆盖？", p.display());
                    let overwrite =
                        match inquire::Confirm::new(&prompt).with_default(false).prompt() {
                            Ok(v) => v,
                            Err(
                                inquire::error::InquireError::OperationCanceled
                                | inquire::error::InquireError::OperationInterrupted,
                            ) => false,
                            Err(e) => return Err(Error::Msg(e.to_string())),
                        };
                    if overwrite {
                        let rec = install::to_rec(target);
                        let _ = remove::remove_install(layout, cfg, reg, &entry.name, &rec);
                        install::install_skill(
                            layout,
                            cfg,
                            reg,
                            &spec.key,
                            &entry.name,
                            &entry.rel_path.to_string_lossy(),
                            target,
                            method,
                            &cached.commit,
                        )?;
                        println!(
                            "{}",
                            cli_ui::ok(format!(
                                "已覆盖安装 {} → {target}（{}）",
                                entry.name,
                                cli_ui::method_label(method)
                            ))
                        );
                    } else {
                        println!(
                            "{}",
                            cli_ui::skip(format!("跳过 {} → {target}", entry.name))
                        );
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    reg.save(layout)?;
    Ok(())
}

/// 收藏向导（已在正常终端模式下运行）：输 source → 扫描 → 多选（默认全选）→ 走 core 收藏。
/// 直接操作并落盘内存 registry（与 install_wizard 同因：TUI 内未落盘的 auto_update 切换不得丢失）。
fn fav_wizard(layout: &Layout, app: &mut AppState) -> Result<()> {
    let source = inquire::Text::new("source（github:owner/repo、URL 或本地路径）")
        .prompt()
        .map_err(|e| Error::Msg(e.to_string()))?;
    let spec = parse_source(&source)?;
    let cached = cache::ensure_cached(layout, &spec)?;
    if !cached.fresh {
        println!("已缓存 {}，复用", spec.key);
    }
    let all = cache::scan_skills(&cached.path)?;
    let names: Vec<String> = all.iter().map(|s| s.name.clone()).collect();
    let r = inquire::MultiSelect::new("选择要收藏的技能", names.clone())
        .with_all_selected_by_default()
        .with_help_message("输入过滤，空格勾选，→ 全选，← 全清，回车确认，ESC 取消")
        .with_page_size(20)
        .with_validator(inquire::min_length!(1, "至少选择一个技能"))
        .prompt();
    // ESC/Ctrl+C = 取消（与 wizard 各弹窗同款语义）
    let Some(selected) = wizard::cancel_to_none(r)? else {
        println!("已取消收藏");
        return Ok(());
    };
    // 全选 = 整仓收藏（传空切片走全量覆盖语义，快照随仓库收缩也能刷新干净）；
    // 部分选 = 只收藏勾选项（upsert）
    let picked: Vec<String> = if selected.len() == names.len() {
        Vec::new()
    } else {
        selected
    };
    let (key, n) = favorites::bookmark(layout, &mut app.registry, &spec, &picked)?;
    let n = if picked.is_empty() { all.len() } else { n };
    app.registry.save(layout)?;
    println!("已收藏 {key}（{n} 个技能）");
    Ok(())
}

/// 从收藏安装（已在正常终端模式下运行）：当前行定技能集 → 共享四步向导（agent → scope → 方式）→ 走 core 安装。
fn fav_install_wizard(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let rows = app.fav_rows();
    let Some(row) = rows.get(app.selected) else {
        println!("（无收藏）");
        return Ok(());
    };
    // 第 1 步：技能集由当前行决定——技能行单技能；Source 行单技能仓直接取，
    // 多技能仓弹 pick_skills（ESC/Ctrl+C = 取消）
    let (key, picked) = match row {
        FavRow::Skill(key, i) => (
            key.clone(),
            vec![app.registry.favorites[key].skills[*i].name.clone()],
        ),
        FavRow::Source(key) => {
            let fav = &app.registry.favorites[key];
            if favorites::is_single_skill_repo(fav) {
                (key.clone(), vec![fav.skills[0].name.clone()])
            } else {
                let choices: Vec<wizard::SkillChoice> = fav
                    .skills
                    .iter()
                    .map(|s| wizard::SkillChoice {
                        name: s.name.clone(),
                        description: s.description.clone(),
                    })
                    .collect();
                let Some(p) = wizard::pick_skills(&choices)? else {
                    println!("已取消安装");
                    return Ok(());
                };
                (key.clone(), p)
            }
        }
    };
    // 第 2~4 步：agent（ESC/Ctrl+C = 取消；确认后记忆本次选择）→ scope → method，展开去重为 targets
    let detected = agents::detect_installed();
    let remembered = crate::core::state::load(layout).last_selected_agents;
    let Some(agent_ids) = wizard::pick_agents(&detected, &remembered)? else {
        println!("已取消安装");
        return Ok(());
    };
    crate::core::state::save_last_agents(layout, &agent_ids);
    let Some(scope) = wizard::pick_scope(&agent_ids)? else {
        println!("已取消安装");
        return Ok(());
    };
    let Some(method) = wizard::pick_method()? else {
        println!("已取消安装");
        return Ok(());
    };
    let targets = wizard::expand_targets(&agent_ids, &scope, cfg)?;
    if targets.is_empty() {
        println!("已取消安装");
        return Ok(());
    }
    println!(
        "将安装 {} 个技能 → {} 个目标（{}）",
        picked.len(),
        targets.len(),
        cli_ui::method_label(method)
    );
    for s in &picked {
        for target in &targets {
            match favorites::fav_install(layout, cfg, &mut app.registry, &key, s, target, method) {
                Ok(_) => println!(
                    "{}",
                    cli_ui::ok(format!(
                        "已安装 {s} → {target}（{}）",
                        cli_ui::method_label(method)
                    ))
                ),
                Err(Error::Conflict(p)) => {
                    // ESC/Ctrl+C 视为不覆盖（跳过该条），不中断批次（与 CLI install_loop 同款）
                    let prompt = format!("{} 已存在，覆盖？", p.display());
                    let overwrite =
                        match inquire::Confirm::new(&prompt).with_default(false).prompt() {
                            Ok(v) => v,
                            Err(
                                inquire::error::InquireError::OperationCanceled
                                | inquire::error::InquireError::OperationInterrupted,
                            ) => false,
                            Err(e) => return Err(Error::Msg(e.to_string())),
                        };
                    if overwrite {
                        let rec = install::to_rec(target);
                        let _ = remove::remove_install(layout, cfg, &mut app.registry, s, &rec);
                        favorites::fav_install(
                            layout,
                            cfg,
                            &mut app.registry,
                            &key,
                            s,
                            target,
                            method,
                        )?;
                        println!(
                            "{}",
                            cli_ui::ok(format!(
                                "已覆盖安装 {s} → {target}（{}）",
                                cli_ui::method_label(method)
                            ))
                        );
                    } else {
                        println!("{}", cli_ui::skip(format!("跳过 {s} → {target}")));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }
    app.registry.save(layout)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::{Install, Method, TargetRec};

    /// 回归：install_wizard 直接操作 app.registry 并 save，
    /// TUI 内按 a 的 auto_update 切换（仅内存状态）不得被磁盘副本覆盖丢失。
    #[test]
    fn wizard_save_path_preserves_in_memory_auto_update_toggle() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        let mut reg = Registry {
            version: 3,
            ..Default::default()
        };
        reg.installs.push(Install {
            skill: "s0".into(),
            source: "github/o/r".into(),
            source_path: "skills/s0".into(),
            target: TargetRec::Global {
                agent: "agents".into(),
            },
            method: Method::Copy,
            commit: "c1".into(),
            auto_update: None,
            installed_at: "t".into(),
        });
        let mut app = AppState::new(reg);
        app.reduce(Action::ToggleAutoUpdate); // 只改内存，未落盘
        assert_eq!(app.registry.installs[0].auto_update, Some(true));

        // 向导路径：直接 save 内存 registry（不再从磁盘 load 覆盖）。
        app.registry.save(&layout).unwrap();
        let reloaded = Registry::load(&layout).unwrap();
        assert_eq!(reloaded.installs[0].auto_update, Some(true));
        assert_eq!(reloaded.installs[0].skill, "s0");
    }

    #[test]
    fn confirm_delete_and_action_menu_empty_rows_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        let cfg = Config::default();
        let mut app = AppState::new(Registry {
            version: 3,
            ..Default::default()
        });

        // 无可见行时直接 Ok(()) 返回，不 panic，不卡住
        assert!(confirm_delete_installed(&layout, &cfg, &mut app).is_ok());
        assert!(installed_action_menu(&layout, &cfg, &mut app).is_ok());
    }

    #[test]
    fn confirm_delete_installed_noop_when_not_confirmed() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        let cfg = Config::default();
        let mut reg = Registry {
            version: 3,
            ..Default::default()
        };
        reg.installs.push(Install {
            skill: "s0".into(),
            source: "github/o/r".into(),
            source_path: "skills/s0".into(),
            target: TargetRec::Global {
                agent: "agents".into(),
            },
            method: Method::Copy,
            commit: "c1".into(),
            auto_update: None,
            installed_at: "t".into(),
        });
        let mut app = AppState::new(reg);
        // 非交互环境下 inquire prompt 返回错误，unwrap_or(false) 判定为未确认，不删除
        assert!(confirm_delete_installed(&layout, &cfg, &mut app).is_ok());
        assert_eq!(app.registry.installs.len(), 1);
    }
}
