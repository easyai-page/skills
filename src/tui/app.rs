//! TUI 应用状态与纯函数 reducer（业务变更全部走 core 类型）。

use crate::core::registry::{Install, Method, Registry};

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum View {
    Installed,
    Install,
    Sources,
}

#[derive(PartialEq, Debug)]
pub enum Action {
    Up,
    Down,
    NextView,
    PrevView,
    ToggleAutoUpdate,
    Select,
    Quit,
}

pub struct AppState {
    pub registry: Registry,
    pub view: View,
    pub selected: usize,
    pub tag_filter: Option<String>,
}

impl AppState {
    pub fn new(registry: Registry) -> Self {
        AppState {
            registry,
            view: View::Installed,
            selected: 0,
            tag_filter: None,
        }
    }

    pub fn visible_rows(&self) -> Vec<&Install> {
        self.registry
            .installs
            .iter()
            .filter(|i| {
                self.tag_filter
                    .as_ref()
                    .map(|t| i.tags.contains(t))
                    .unwrap_or(true)
            })
            .collect()
    }

    pub fn reduce(&mut self, action: Action) {
        match action {
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => {
                let max = self.visible_rows().len().saturating_sub(1);
                self.selected = (self.selected + 1).min(max);
            }
            Action::NextView => {
                self.view = match self.view {
                    View::Installed => View::Install,
                    View::Install => View::Sources,
                    View::Sources => View::Installed,
                };
                self.selected = 0;
            }
            Action::PrevView => {
                self.view = match self.view {
                    View::Installed => View::Sources,
                    View::Install => View::Installed,
                    View::Sources => View::Install,
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
            Action::Select | Action::Quit => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::{Install, Method, Registry, TargetRec};

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

    #[test]
    fn tab_switches_view() {
        let mut app = app_with(1);
        assert_eq!(app.view, View::Installed);
        app.reduce(Action::NextView);
        assert_eq!(app.view, View::Install);
        app.reduce(Action::NextView);
        assert_eq!(app.view, View::Sources);
        app.reduce(Action::NextView);
        assert_eq!(app.view, View::Installed);
    }

    #[test]
    fn filter_by_tag_narrows_rows() {
        let mut app = app_with(3);
        app.registry.installs[1].tags = vec!["frontend".into()];
        app.tag_filter = Some("frontend".into());
        let rows = app.visible_rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].skill, "s1");
    }
}
