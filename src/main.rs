mod cli;
mod core;
mod tui;
mod web;

fn main() {
    let cli = <cli::Cli as clap::Parser>::parse();
    if let Err(e) = cli::commands::run(cli) {
        eprintln!("错误: {e}");
        std::process::exit(1);
    }
}
