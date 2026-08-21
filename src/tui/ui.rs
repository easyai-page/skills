//! ratatui 渲染层：无状态纯绘制，业务数据全部来自 AppState。

use ratatui::{
    Frame,
    layout::{Constraint, Layout as RLayout},
    style::{Color, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs},
};

use super::app::{AppState, View};

pub fn draw(f: &mut Frame, app: &AppState) {
    let chunks = RLayout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(f.area());
    let titles = ["已安装", "安装向导", "仓库缓存"];
    let idx = match app.view {
        View::Installed => 0,
        View::Install => 1,
        View::Sources => 2,
    };
    f.render_widget(
        Tabs::new(titles.iter().map(|t| *t).collect::<Vec<_>>())
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
                        &s.commit[..7.min(s.commit.len())],
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::{Install, Method, Registry, SourceRecord, TargetRec};
    use ratatui::{Terminal, backend::TestBackend};

    /// 冒烟测试：三个视图都能完整渲染不 panic（不做像素级断言）。
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
        let mut app = AppState::new(reg);
        let backend = TestBackend::new(80, 24);
        let mut term = Terminal::new(backend).unwrap();
        for view in [View::Installed, View::Install, View::Sources] {
            app.view = view;
            term.draw(|f| draw(f, &app)).unwrap();
        }
    }
}
