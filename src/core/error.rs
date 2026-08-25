#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("toml: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("无效的 target 语法: {0}（应为 global:<name> 或 project:<绝对路径>）")]
    BadTarget(String),
    #[error("未知的全局 target: {0}")]
    UnknownTarget(String),
    #[error("无法确定用户主目录")]
    NoHome,
    #[error("无效的技能名，必须是单一普通路径组件: {0}")]
    InvalidSkillName(String),
    #[error("源路径不在缓存目录内: {0}")]
    InvalidSourcePath(std::path::PathBuf),
    #[error("源路径不存在或不是目录: {0}")]
    SourceNotDirectory(std::path::PathBuf),
    #[error("目标已存在同名技能: {0}")]
    Conflict(std::path::PathBuf),
    #[error("技能未安装: {0}")]
    NotInstalled(String),
    #[error("未收藏: {0}")]
    NotBookmarked(String),
    #[error("磁盘实况与安装记录不一致: {0}")]
    Mismatch(String),
    #[error("git 操作失败: {0}")]
    Git(String),
    #[error("git 回滚失败，缓存不可用: {0}")]
    GitRecovery(String),
    #[error("{0}")]
    Msg(String),
}
pub type Result<T> = std::result::Result<T, Error>;
