//! ratatui 渲染层：无状态纯绘制，业务数据全部来自 AppState。

use ratatui::{
    Frame,
    layout::{Constraint, Layout as RLayout},
    style::{Color, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs},
};

use super::app::FavRow;
use super::app::{AppState, View};

pub fn draw(f: &mut Frame, app: &AppState) {
    let chunks = RLayout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(f.area());
    let titles = ["已安装", "安装向导", "仓库缓存", "收藏"];
    let idx = match app.view {
        View::Installed => 0,
        View::Install => 1,
        View::Sources => 2,
        View::Favorites => 3,
    };
    f.render_widget(
        Tabs::new(titles.to_vec())
            .select(idx)
            .block(Block::default().borders(Borders::ALL).title("skills")),
        chunks[0],
    );
    match app.view {
        View::Installed => {
            let visible = app.visible_rows();
            let rows = visible.iter().enumerate().map(|(i, r)| {
                let style = if i == app.selected {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };
                Row::new(vec![
                    Cell::from(r.skill.clone()),
                    Cell::from(format!("{:?}", r.method)),
                    Cell::from(format!("{:?}", r.target)),
                    Cell::from(r.tags.join(",")),
                    Cell::from(match r.auto_update {
                        Some(true) => "开",
                        Some(false) => "关",
                        None => "跟随包级",
                    }),
                ])
                .style(style)
            });
            f.render_widget(
                Table::new(
                    rows,
                    [
                        Constraint::Percentage(25),
                        Constraint::Percentage(10),
                        Constraint::Percentage(35),
                        Constraint::Percentage(15),
                        Constraint::Percentage(15),
                    ],
                )
                .header(
                    Row::new(vec!["技能", "方式", "目标", "分类", "自动更新"])
                        .style(Style::default().fg(Color::Cyan)),
                ),
                chunks[1],
            );
        }
        View::Install => {
            f.render_widget(
                Paragraph::new("安装向导：按 i 输入 source（本视图在事件循环中处理）")
                    .block(Block::default().borders(Borders::ALL)),
                chunks[1],
            );
        }
        View::Sources => {
            let text: String = app
                .registry
                .sources
                .iter()
                .map(|(k, s)| {
                    format!(
                        "{k}\t{}\tauto_update={:?}\n",
                        // chars().take(7)：按字符截断，避免多字节 UTF-8
                        // 在字节边界切片导致 panic。
                        s.commit.chars().take(7).collect::<String>(),
                        s.auto_update
                    )
                })
                .collect();
            f.render_widget(
                Paragraph::new(text)
                    .block(Block::default().borders(Borders::ALL).title("仓库缓存")),
                chunks[1],
            );
        }
        View::Favorites => {
            let rows = app.fav_rows();
            let table_rows = rows.iter().enumerate().map(|(i, row)| {
                let style = if i == app.selected {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };
                match row {
                    FavRow::Source(key) => {
                        let fav = &app.registry.favorites[key];
                        let date: String = fav.bookmarked_at.chars().take(10).collect();
                        if crate::core::favorites::is_single_skill_repo(fav) {
                            // 单技能仓库：二级留空，用途挂一级行
                            Row::new(vec![
                                Cell::from(key.clone()),
                                Cell::from(fav.skills[0].description.clone()),
                                Cell::from(date),
                            ])
                        } else {
                            Row::new(vec![
                                Cell::from(key.clone()),
                                Cell::from(format!("{} 个技能", fav.skills.len())),
                                Cell::from(date),
                            ])
                        }
                    }
                    FavRow::Skill(key, idx) => {
                        let s = &app.registry.favorites[key].skills[*idx];
                        Row::new(vec![
                            Cell::from(format!("  {}", s.name)),
                            Cell::from(s.description.clone()),
                            Cell::from(String::new()),
                        ])
                    }
                }
                .style(style)
            });
            f.render_widget(
                Table::new(
                    table_rows,
                    [
                        Constraint::Percentage(35),
                        Constraint::Percentage(50),
                        Constraint::Percentage(15),
                    ],
                )
                .header(
                    Row::new(vec!["收藏", "用途", "收藏时间"])
                        .style(Style::default().fg(Color::Cyan)),
                )
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("f=收藏 d=删除 i=安装"),
                ),
                chunks[1],
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::{
        FavSkill, Favorite, Install, Method, Registry, SourceRecord, TargetRec,
    };
    use ratatui::{Terminal, backend::TestBackend};

    /// 冒烟测试：四个视图都能完整渲染不 panic（不做像素级断言）。
    #[test]
    fn draw_all_views_smoke() {
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.installs.push(Install {
            skill: "web-design".into(),
            source: "github/o/r".into(),
            source_path: "skills/web-design".into(),
            target: TargetRec::Global {
                name: "agents".into(),
            },
            method: Method::Copy,
            commit: "c1".into(),
            tags: vec!["frontend".into()],
            auto_update: Some(true),
            installed_at: "t".into(),
        });
        reg.sources.insert(
            "github/o/r".into(),
            SourceRecord {
                url: "https://github.com/o/r".into(),
                commit: "deadbeefcafe".into(),
                fetched_at: "t".into(),
                auto_update: None,
            },
        );
        // 收藏 fixture：一个多技能仓库（标题行 + 技能行）+ 一个单技能仓库（仅标题行），
        // 两种渲染分支都要走到
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
        let mut app = AppState::new(reg);
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        for view in [
            View::Installed,
            View::Install,
            View::Sources,
            View::Favorites,
        ] {
            app.view = view;
            term.draw(|f| draw(f, &app)).unwrap();
        }
    }
}
