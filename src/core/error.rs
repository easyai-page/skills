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
    #[error("source 已缓存: {0}")]
    AlreadyCached(String),
    #[error("技能未安装: {0}")]
    NotInstalled(String),
    #[error("git 操作失败: {0}")]
    Git(String),
    #[error("git 回滚失败，缓存不可用: {0}")]
    GitRecovery(String),
    #[error("{0}")]
    Msg(String),
}
pub type Result<T> = std::result::Result<T, Error>;
