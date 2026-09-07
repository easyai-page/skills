//! CLI 终端输出辅助：阶段 spinner（indicatif）+ 染色状态行（console）。
//! 仅服务普通终端输出（CLI 全程、TUI 向导的弹窗间隙）；TUI 主界面渲染走 ratatui，不用本模块。

use console::style;
use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;

use crate::core::registry::Method;

/// 阶段级不确定进度条。stderr 非 TTY（管道/CI）时返回隐藏 bar，调用方无需分支。
pub fn spinner(msg: impl Into<String>) -> ProgressBar {
    let pb = if std::io::stderr().is_terminal() {
        ProgressBar::new_spinner()
    } else {
        ProgressBar::hidden()
    };
    if let Ok(st) = ProgressStyle::with_template("{spinner:.cyan} {msg}") {
        pb.set_style(st);
    }
    pb.set_message(msg.into());
    pb.enable_steady_tick(std::time::Duration::from_millis(100));
    pb
}

/// 安装循环里打状态行：TTY 时打到 spinner 上方（不糊屏），隐藏 bar（非 TTY）时直接 println，
/// 保证 CI 日志不丢行。
pub fn progress_or_print(pb: &ProgressBar, line: String) {
    if pb.is_hidden() {
        println!("{line}");
    } else {
        pb.println(line);
    }
}

pub fn ok(msg: impl std::fmt::Display) -> String {
    format!("{} {}", style("✓").green().bold(), msg)
}

pub fn skip(msg: impl std::fmt::Display) -> String {
    format!("{} {}", style("⊘").yellow(), msg)
}

pub fn err(msg: impl std::fmt::Display) -> String {
    format!("{} {}", style("✗").red().bold(), msg)
}

pub fn warn(msg: impl std::fmt::Display) -> String {
    format!("{} {}", style("!").yellow(), msg)
}

/// Method 的用户向文案（不再对用户暴露 Rust 枚举名）。
pub fn method_label(m: Method) -> &'static str {
    match m {
        Method::Symlink => "符号链接",
        Method::Copy => "独立副本",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_label_is_user_facing() {
        assert_eq!(method_label(Method::Symlink), "符号链接");
        assert_eq!(method_label(Method::Copy), "独立副本");
    }
}
