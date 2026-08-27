pub mod app;
pub mod ui;

use crossterm::event::{self, Event, KeyCode};
use crossterm::terminal::{EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::backend::CrosstermBackend;

use crate::core::{
    cache,
    config::Config,
    error::{Error, Result},
    favorites, install,
    paths::{Layout, Target},
    registry::Registry,
    remove,
    source::parse_source,
};
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

    /// 暂时恢复正常终端（跑 dialoguer 向导用）；幂等。
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
                // i：Installed 等视图=从仓库装；Favorites=从收藏装。向导需正常终端（dialoguer）。
                guard.suspend();
                let r = if app.view == View::Favorites {
                    fav_install_wizard(layout, cfg, app)
                } else {
                    install_wizard(layout, cfg, app)
                };
                if let Err(e) = r {
                    eprintln!("安装失败: {e}");
                    let _ = dialoguer::Input::<String>::new()
                        .with_prompt("按回车返回 TUI")
                        .allow_empty(true)
                        .interact_text();
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
                    let _ = dialoguer::Input::<String>::new()
                        .with_prompt("按回车返回 TUI")
                        .allow_empty(true)
                        .interact_text();
                }
                guard.resume()?;
                term.clear()?;
                continue;
            }
            let action = match k.code {
                KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
                KeyCode::Up | KeyCode::Char('k') => Action::Up,
                KeyCode::Down | KeyCode::Char('j') => Action::Down,
                KeyCode::Tab => Action::NextView,
                KeyCode::BackTab => Action::PrevView,
                KeyCode::Char('a') => Action::ToggleAutoUpdate,
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

/// 安装向导（已在正常终端模式下运行）：选 source → 选技能 → 选目标 → 走 core 安装。
fn install_wizard(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let source: String = dialoguer::Input::new()
        .with_prompt("source（github:owner/repo、URL 或本地路径）")
        .interact_text()
        .map_err(|e| Error::Msg(e.to_string()))?;
    let spec = parse_source(&source)?;
    let cached = cache::ensure_cached(layout, &spec)?;
    if !cached.fresh {
        println!("已缓存 {}，复用（skills update 可更新）", spec.key);
    }
    let all = cache::scan_skills(&cached.path)?;
    let names: Vec<String> = all.iter().map(|s| s.name.clone()).collect();
    let idx = dialoguer::MultiSelect::new()
        .with_prompt("选择要安装的技能")
        .items(&names)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    if idx.is_empty() {
        println!("未选择技能，取消安装");
        return Ok(());
    }
    let target = pick_target(cfg)?;
    let method = cfg.default_method;

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
    for i in idx {
        let entry = &all[i];
        match install::install_skill(
            layout,
            cfg,
            reg,
            &spec.key,
            &entry.name,
            &entry.rel_path.to_string_lossy(),
            &target,
            method,
            &cached.commit,
        ) {
            Ok(_) => println!("已安装 {} → {target:?} ({method:?})", entry.name),
            Err(Error::Conflict(p)) => {
                let overwrite = dialoguer::Confirm::new()
                    .with_prompt(format!("{p:?} 已存在，覆盖？"))
                    .interact()
                    .map_err(|e| Error::Msg(e.to_string()))?;
                if overwrite {
                    let rec = install::to_rec(&target);
                    let _ = remove::remove_install(layout, cfg, reg, &entry.name, &rec);
                    install::install_skill(
                        layout,
                        cfg,
                        reg,
                        &spec.key,
                        &entry.name,
                        &entry.rel_path.to_string_lossy(),
                        &target,
                        method,
                        &cached.commit,
                    )?;
                    println!("已覆盖安装 {} → {target:?} ({method:?})", entry.name);
                } else {
                    println!("跳过 {}", entry.name);
                }
            }
            Err(e) => return Err(e),
        }
    }
    reg.save(layout)?;
    Ok(())
}

/// 安装向导共用的目标选择：配置的 global targets + 当前项目（project:<cwd>）。
fn pick_target(cfg: &Config) -> Result<Target> {
    let mut targets: Vec<(String, Target)> = cfg
        .targets
        .keys()
        .map(|n| (format!("global:{n}"), Target::Global { name: n.clone() }))
        .collect();
    targets.push((
        format!("project:{}", std::env::current_dir()?.display()),
        Target::Project {
            root: std::env::current_dir()?,
        },
    ));
    let labels: Vec<&str> = targets.iter().map(|(l, _)| l.as_str()).collect();
    let ti = dialoguer::Select::new()
        .with_prompt("安装到目标")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    Ok(targets[ti].1.clone())
}

/// 收藏向导（已在正常终端模式下运行）：输 source → 扫描 → 多选（默认全选）→ 走 core 收藏。
/// 直接操作并落盘内存 registry（与 install_wizard 同因：TUI 内未落盘的 auto_update 切换不得丢失）。
fn fav_wizard(layout: &Layout, app: &mut AppState) -> Result<()> {
    let source: String = dialoguer::Input::new()
        .with_prompt("source（github:owner/repo、URL 或本地路径）")
        .interact_text()
        .map_err(|e| Error::Msg(e.to_string()))?;
    let spec = parse_source(&source)?;
    let cached = cache::ensure_cached(layout, &spec)?;
    if !cached.fresh {
        println!("已缓存 {}，复用", spec.key);
    }
    let all = cache::scan_skills(&cached.path)?;
    let names: Vec<String> = all.iter().map(|s| s.name.clone()).collect();
    let defaults = vec![true; names.len()];
    let idx = dialoguer::MultiSelect::new()
        .with_prompt("选择要收藏的技能")
        .items(&names)
        .defaults(&defaults)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    if idx.is_empty() {
        println!("未选择技能，取消收藏");
        return Ok(());
    }
    // 全选 = 整仓收藏（传空切片走全量覆盖语义，快照随仓库收缩也能刷新干净）；
    // 部分选 = 只收藏勾选项（upsert）
    let picked: Vec<String> = if idx.len() == names.len() {
        Vec::new()
    } else {
        idx.into_iter().map(|i| names[i].clone()).collect()
    };
    let (key, n) = favorites::bookmark(layout, &mut app.registry, &spec, &picked)?;
    let n = if picked.is_empty() { all.len() } else { n };
    app.registry.save(layout)?;
    println!("已收藏 {key}（{n} 个技能）");
    Ok(())
}

/// 从收藏安装（已在正常终端模式下运行）：当前行定技能集 → 选目标 → 走 core 安装。
fn fav_install_wizard(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let rows = app.fav_rows();
    let Some(row) = rows.get(app.selected) else {
        println!("（无收藏）");
        return Ok(());
    };
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
                let names: Vec<String> = fav.skills.iter().map(|s| s.name.clone()).collect();
                let idx = dialoguer::MultiSelect::new()
                    .with_prompt("选择要安装的技能")
                    .items(&names)
                    .interact()
                    .map_err(|e| Error::Msg(e.to_string()))?;
                if idx.is_empty() {
                    println!("未选择技能，取消安装");
                    return Ok(());
                }
                (
                    key.clone(),
                    idx.into_iter().map(|i| names[i].clone()).collect(),
                )
            }
        }
    };
    let target = pick_target(cfg)?;
    let method = cfg.default_method;
    for s in &picked {
        match favorites::fav_install(layout, cfg, &mut app.registry, &key, s, &target, method) {
            Ok(_) => println!("已安装 {s} → {target:?} ({method:?})"),
            Err(Error::Conflict(p)) => {
                let overwrite = dialoguer::Confirm::new()
                    .with_prompt(format!("{p:?} 已存在，覆盖？"))
                    .interact()
                    .map_err(|e| Error::Msg(e.to_string()))?;
                if overwrite {
                    let rec = install::to_rec(&target);
                    let _ = remove::remove_install(layout, cfg, &mut app.registry, s, &rec);
                    favorites::fav_install(
                        layout,
                        cfg,
                        &mut app.registry,
                        &key,
                        s,
                        &target,
                        method,
                    )?;
                    println!("已覆盖安装 {s} → {target:?} ({method:?})");
                } else {
                    println!("跳过 {s}");
                }
            }
            Err(e) => return Err(e),
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
            version: 1,
            ..Default::default()
        };
        reg.installs.push(Install {
            skill: "s0".into(),
            source: "github/o/r".into(),
            source_path: "skills/s0".into(),
            target: TargetRec::Global {
                name: "agents".into(),
            },
            method: Method::Copy,
            commit: "c1".into(),
            tags: vec![],
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
}
