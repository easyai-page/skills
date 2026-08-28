use clap::{Parser, Subcommand, ValueEnum};

pub mod commands;

#[derive(Parser)]
#[command(name = "skills", about = "技能包管理器：下载、安装、分类、更新")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum MethodArg {
    Symlink,
    Copy,
}

#[derive(Subcommand)]
pub enum Cmd {
    /// 下载并安装技能（source 已缓存则复用）
    Add {
        source: String,
        #[arg(short, long)]
        skill: Vec<String>,
        #[arg(short, long)]
        target: Vec<String>, // global:<name> | project:<abs路径>
        #[arg(short = 'g', long)]
        global: bool, // 等价 --target global:agents
        #[arg(long, value_enum)]
        method: Option<MethodArg>,
        #[arg(short = 'y', long)]
        yes: bool,
    },
    /// 列出已安装技能
    #[command(alias = "ls")]
    List {
        #[arg(long)]
        tag: Option<String>,
        #[arg(short, long)]
        target: Option<String>,
        #[arg(short = 'g', long)]
        global: bool,
    },
    /// 删除已安装技能（先查记录再核实磁盘）
    Remove {
        skills: Vec<String>,
        #[arg(short, long)]
        target: Vec<String>,
        #[arg(long)]
        tag: Option<String>,
    },
    /// 按两级策略更新；显式指定技能时强制更新该副本
    Update {
        skills: Vec<String>,
        #[arg(short, long)]
        target: Option<String>,
        #[arg(long)]
        all: bool,
        #[arg(long)]
        dry_run: bool,
        /// 覆盖 copy 副本的本地修改，跳过确认提示
        #[arg(long)]
        force: bool,
    },
    /// 分类管理（只写 registry.json）
    Tag {
        skill: String,
        tags: Vec<String>,
        #[arg(short, long)]
        target: String,
        #[arg(long)]
        remove: bool,
    },
    /// 升级策略（只写 registry.json）；--on/--off/--inherit 必须且只能给一个
    #[command(group = clap::ArgGroup::new("policy").required(true).multiple(false).args(["on", "off", "inherit"]))]
    AutoUpdate {
        skill: Option<String>,
        #[arg(short, long)]
        target: Option<String>,
        #[arg(short, long)]
        source: Option<String>,
        /// 开启自动更新
        #[arg(long)]
        on: bool,
        /// 关闭自动更新
        #[arg(long)]
        off: bool,
        /// 清除副本级覆盖，跟随包级
        #[arg(long)]
        inherit: bool,
    },
    /// 全局配置（只写 config.toml）
    Config {
        #[command(subcommand)]
        sub: ConfigCmd,
    },
    /// 进入 TUI
    Tui,
    /// 收藏技能（只记录地址与功能，不安装）；无参数时列出收藏
    #[command(args_conflicts_with_subcommands = true)]
    Fav {
        source: Option<String>,
        #[arg(short, long)]
        skill: Vec<String>,
        #[command(subcommand)]
        sub: Option<FavSub>,
    },
    /// 启动 Web 管理页
    Ui {
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        no_open: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    Get {
        key: String,
    },
    Set {
        key: String,
        value: String,
    },
    Targets {
        #[command(subcommand)]
        sub: TargetsCmd,
    },
}

#[derive(Subcommand)]
pub enum TargetsCmd {
    Add { name: String, path: String },
    Remove { name: String },
}

#[derive(Subcommand)]
pub enum FavSub {
    /// 删除收藏（--skill 删指定技能，否则删整包；不动缓存与已安装副本）
    Rm {
        source: String,
        #[arg(short, long)]
        skill: Vec<String>,
    },
    /// 从收藏安装（--skill 装指定技能，多技能收藏缺省时交互选择）
    Install {
        source: String,
        #[arg(short, long)]
        skill: Vec<String>,
        #[arg(short, long)]
        target: Vec<String>,
        #[arg(short = 'g', long)]
        global: bool,
        #[arg(long, value_enum)]
        method: Option<MethodArg>,
        #[arg(short = 'y', long)]
        yes: bool,
    },
}
