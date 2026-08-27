//! TUI 应用状态与纯函数 reducer（业务变更全部走 core 类型）。

use crate::core::favorites;
use crate::core::registry::{Install, Method, Registry};

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum View {
    Installed,
    Install,
    Sources,
    Favorites,
}

#[derive(PartialEq, Debug)]
pub enum Action {
    Up,
    Down,
    NextView,
    PrevView,
    ToggleAutoUpdate,
    // 事件循环的 'd' 键接线在下一任务落地，目前仅测试构造此变体
    // （与 git.rs FailurePoint 的 allow(dead_code) 同一过渡模式）。
    #[allow(dead_code)]
    DeleteFav,
    Quit,
}

/// 收藏视图的扁平行：source 标题行或其中的技能行。
#[derive(PartialEq, Debug)]
pub enum FavRow {
    Source(String),       // source key
    Skill(String, usize), // source key + skills 下标
}

pub struct AppState {
    pub registry: Registry,
    pub view: View,
    pub selected: usize,
}

impl AppState {
    pub fn new(registry: Registry) -> Self {
        AppState {
            registry,
            view: View::Installed,
            selected: 0,
        }
    }

    pub fn visible_rows(&self) -> Vec<&Install> {
        self.registry.installs.iter().collect()
    }

    /// 收藏视图展平行：多技能仓库 = 标题行 + 每技能一行；
    /// 单技能仓库只出标题行（二级留空，用途挂在一级）。
    pub fn fav_rows(&self) -> Vec<FavRow> {
        let mut rows = Vec::new();
        for (key, fav) in &self.registry.favorites {
            rows.push(FavRow::Source(key.clone()));
            if !favorites::is_single_skill_repo(fav) {
                for i in 0..fav.skills.len() {
                    rows.push(FavRow::Skill(key.clone(), i));
                }
            }
        }
        rows
    }

    pub fn reduce(&mut self, action: Action) {
        match action {
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => {
                let rows = match self.view {
                    View::Installed => self.visible_rows().len(),
                    View::Favorites => self.fav_rows().len(),
                    View::Install | View::Sources => 0,
                };
                let max = rows.saturating_sub(1);
                self.selected = (self.selected + 1).min(max);
            }
            Action::NextView => {
                self.view = match self.view {
                    View::Installed => View::Install,
                    View::Install => View::Sources,
                    View::Sources => View::Favorites,
                    View::Favorites => View::Installed,
                };
                self.selected = 0;
            }
            Action::PrevView => {
                self.view = match self.view {
                    View::Installed => View::Favorites,
                    View::Favorites => View::Sources,
                    View::Sources => View::Install,
                    View::Install => View::Installed,
                };
                self.selected = 0;
            }
            Action::ToggleAutoUpdate => {
                if self.view != View::Installed {
                    return;
                }
                if let Some(row) = self.visible_rows().get(self.selected) {
                    let skill = row.skill.clone();
                    let target = row.target.clone();
                    let method = row.method;
                    if method == Method::Symlink {
                        return; // 软连接跟随包级
                    }
                    if let Some(inst) = self
                        .registry
                        .installs
                        .iter_mut()
                        .find(|i| i.skill == skill && i.target == target)
                    {
                        inst.auto_update = match inst.auto_update {
                            None => Some(true),
                            Some(true) => Some(false),
                            Some(false) => None,
                        };
                    }
                }
            }
            Action::DeleteFav => {
                if self.view != View::Favorites {
                    return;
                }
                let rows = self.fav_rows();
                if let Some(row) = rows.get(self.selected) {
                    match row {
                        FavRow::Source(key) => {
                            let _ = favorites::unbookmark(&mut self.registry, key, &[]);
                        }
                        FavRow::Skill(key, i) => {
                            let name = self.registry.favorites[key].skills[*i].name.clone();
                            let _ = favorites::unbookmark(&mut self.registry, key, &[name]);
                        }
                    }
                    // 行数已收缩：selected clamp 到合法范围
                    let max = self.fav_rows().len().saturating_sub(1);
                    self.selected = self.selected.min(max);
                }
            }
            Action::Quit => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::{FavSkill, Favorite, Install, Method, Registry, TargetRec};

    fn app_with(items: usize) -> AppState {
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        for i in 0..items {
            reg.installs.push(Install {
                skill: format!("s{i}"),
                source: "github/o/r".into(),
                source_path: format!("skills/s{i}").into(),
                target: TargetRec::Global {
                    name: "agents".into(),
                },
                method: Method::Copy,
                commit: "c1".into(),
                tags: vec![],
                auto_update: None,
                installed_at: "t".into(),
            });
        }
        AppState::new(reg)
    }

    fn app_with_favorites() -> AppState {
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.favorites.insert(
            "github/o/r".into(),
            Favorite {
                url: Some("https://github.com/o/r".into()),
                local_path: None,
                commit: "deadbeef".into(),
                bookmarked_at: "2026-08-25T10:00:00Z".into(),
                skills: vec![
                    FavSkill {
                        name: "a".into(),
                        description: "A".into(),
                        source_path: "skills/a".into(),
                    },
                    FavSkill {
                        name: "b".into(),
                        description: "B".into(),
                        source_path: "skills/b".into(),
                    },
                ],
            },
        );
        reg.favorites.insert(
            "local/solo".into(),
            Favorite {
                url: None,
                local_path: Some("/x/solo".into()),
                commit: String::new(),
                bookmarked_at: "2026-08-25T10:00:00Z".into(),
                skills: vec![FavSkill {
                    name: "solo".into(),
                    description: "单技能".into(),
                    source_path: ".".into(),
                }],
            },
        );
        AppState::new(reg)
    }

    #[test]
    fn tab_cycles_four_views() {
        let mut app = app_with(1);
        assert_eq!(app.view, View::Installed);
        app.reduce(Action::NextView); // Install
        app.reduce(Action::NextView); // Sources
        app.reduce(Action::NextView); // Favorites
        assert_eq!(app.view, View::Favorites);
        app.reduce(Action::NextView);
        assert_eq!(app.view, View::Installed);
        app.reduce(Action::PrevView); // 反向回 Favorites
        assert_eq!(app.view, View::Favorites);
    }

    #[test]
    fn favorites_rows_flatten_two_levels() {
        let app = app_with_favorites();
        let rows = app.fav_rows();
        // BTreeMap 排序：github/o/r 在前（1 标题 + 2 技能），local/solo 单技能仓库只 1 行
        assert_eq!(
            rows,
            vec![
                FavRow::Source("github/o/r".into()),
                FavRow::Skill("github/o/r".into(), 0),
                FavRow::Skill("github/o/r".into(), 1),
                FavRow::Source("local/solo".into()),
            ]
        );
    }

    #[test]
    fn favorites_navigation_clamps_per_view() {
        let mut app = app_with_favorites();
        app.view = View::Favorites;
        for _ in 0..10 {
            app.reduce(Action::Down);
        }
        assert_eq!(app.selected, 3);
        app.reduce(Action::Up);
        assert_eq!(app.selected, 2);
        // 切视图清零选中
        app.reduce(Action::NextView);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn delete_fav_skill_row_then_source_row() {
        let mut app = app_with_favorites();
        app.view = View::Favorites;
        app.selected = 1; // Skill(github/o/r, 0)
        app.reduce(Action::DeleteFav);
        assert_eq!(app.registry.favorites["github/o/r"].skills.len(), 1);
        assert_eq!(app.registry.favorites["github/o/r"].skills[0].name, "b");
        // 删除后行数收缩，selected 被 clamp 到合法范围
        assert!(app.selected < app.fav_rows().len());
        // 标题行：删整包
        app.selected = 0;
        app.reduce(Action::DeleteFav);
        assert!(!app.registry.favorites.contains_key("github/o/r"));
        assert!(app.registry.favorites.contains_key("local/solo"));
        // 非收藏视图不误伤
        app.view = View::Installed;
        app.reduce(Action::DeleteFav);
        assert!(app.registry.favorites.contains_key("local/solo"));
    }

    #[test]
    fn navigation_wraps_and_clamps() {
        let mut app = app_with(3);
        assert_eq!(app.selected, 0);
        app.reduce(Action::Down);
        assert_eq!(app.selected, 1);
        app.reduce(Action::Down);
        app.reduce(Action::Down);
        assert_eq!(app.selected, 2); // clamp，不越界
        app.reduce(Action::Up);
        assert_eq!(app.selected, 1);
    }

    #[test]
    fn toggle_auto_update_flips_selected_copy_install() {
        let mut app = app_with(2);
        app.reduce(Action::ToggleAutoUpdate);
        assert_eq!(app.registry.installs[0].auto_update, Some(true));
        app.reduce(Action::ToggleAutoUpdate);
        assert_eq!(app.registry.installs[0].auto_update, Some(false));
        app.reduce(Action::ToggleAutoUpdate);
        assert_eq!(app.registry.installs[0].auto_update, None); // 三态循环 true→false→跟随
    }
}
