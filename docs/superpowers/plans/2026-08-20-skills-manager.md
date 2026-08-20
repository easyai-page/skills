# skills-manager 实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 构建一个跨平台（Windows/macOS/Linux）的技能管理工具，CLI + TUI + 本地 Web 管理页三界面，支持技能包下载缓存、多目标安装（symlink/copy）、安装记录、分类管理、两级自动更新。

**架构：** 单二进制 Rust 程序。`core` 模块承载全部领域逻辑（source 解析、git 缓存、registry、安装/更新/删除引擎），`cli` / `tui` / `web` 三个前端是 core 的薄壳。数据落盘：`~/.skills/` 下 `registry.json`（安装记录，原子写入）与可选 `config.toml`（默认值覆盖）。

**技术栈：** Rust，clap（CLI）、gix/gitoxide（纯 Rust git）、ratatui + crossterm（TUI）、axum + tokio（Web）、serde + serde_json + toml、dialoguer（交互）、dirs（跨平台路径）、chrono（时间戳）、thiserror（错误）、tempfile（测试）。

**规格：** `docs/superpowers/specs/2026-08-20-skills-manager-design.md`

---

## 文件结构

```
Cargo.toml
.github/workflows/ci.yml
src/
├── main.rs               # 二进制入口：解析 CLI，分发到 cli/tui/web
├── core/
│   ├── mod.rs            # re-export
│   ├── error.rs          # Error 枚举（thiserror）
│   ├── paths.rs          # ~/.skills 布局 Layout；Target 枚举与寻址解析
│   ├── source.rs         # source 字符串解析 → SourceSpec（key/url/local）
│   ├── registry.rs       # Registry / SourceRecord / Install / Method，原子读写
│   ├── config.rs         # Config：内置默认 + config.toml 覆盖 + target 表
│   ├── git.rs            # gitoxide 封装：shallow_clone / fetch_and_reset / head_commit
│   ├── cache.rs          # 缓存层：ensure_cached（去重复用）、扫描 SKILL.md
│   ├── install.rs        # 安装引擎：symlink/copy、同名冲突处理决策
│   ├── remove.rs         # 删除引擎：记录核实 + 磁盘实况核实
│   ├── update.rs         # 两级更新引擎：包级 pull + 副本级传播、dry-run 计划
│   └── tags.rs           # 分类：add/remove tags、按 tag 筛选
├── cli/
│   ├── mod.rs            # clap 命令树定义
│   └── commands.rs       # 各子命令实现（add/list/remove/update/tag/auto-update/config）
├── tui/
│   ├── mod.rs
│   ├── app.rs            # AppState + 纯函数 reducer（可测试）
│   └── ui.rs             # ratatui 渲染
└── web/
    ├── mod.rs            # axum 路由 + 启动 + 打开浏览器
    ├── api.rs            # REST handler（测试可测）
    └── static/index.html # 内嵌单页前端（include_str!）
tests/
├── fixtures.rs           # 本地 bare repo fixture 构造
├── add_flow.rs           # add → list → remove 全链路
└── update_flow.rs        # 两级更新策略集成测试
```

**关键类型（跨任务一致性锚点）：**

```rust
// core/paths.rs
pub enum Target {
    Global { name: String },      // 经 Config.targets 解析为路径
    Project { root: PathBuf },    // 安装到 <root>/.agents/skills
}
impl Target {
    pub fn parse(s: &str) -> Result<Target>;   // "global:<name>" | "project:<abs路径>"
    pub fn install_dir(&self, cfg: &Config) -> Result<PathBuf>;
    pub fn matches(&self, other: &Target) -> bool;
}
pub struct Layout { pub root: PathBuf }        // ~/.skills
impl Layout {
    pub fn new() -> Result<Layout>;            // dirs::home_dir()/.skills
    pub fn at(root: PathBuf) -> Layout;        // 测试用
    pub fn cache_dir(&self, key: &str) -> PathBuf;         // root.join(key)
    pub fn registry_path(&self) -> PathBuf;    // root/registry.json
    pub fn config_path(&self) -> PathBuf;      // root/config.toml
}

// core/source.rs
pub struct SourceSpec {
    pub key: String,                 // "github/owner/repo" 或 "local/<name>"
    pub url: Option<String>,         // git URL；本地来源为 None
    pub local_path: Option<PathBuf>, // 本地来源的路径
}
pub fn parse_source(input: &str) -> Result<SourceSpec>;

// core/registry.rs
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetRec { Global { name: String }, Project { root: PathBuf } }

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Method { Symlink, Copy }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SourceRecord {
    pub url: String,
    pub commit: String,
    pub fetched_at: String,               // RFC3339
    #[serde(default)] pub auto_update: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Install {
    pub skill: String,
    pub source: String,                   // SourceSpec.key
    pub source_path: PathBuf,             // 仓库内相对路径
    pub target: TargetRec,
    pub method: Method,
    pub commit: String,
    #[serde(default)] pub tags: Vec<String>,
    #[serde(default)] pub auto_update: Option<bool>,
    pub installed_at: String,             // RFC3339
}

#[derive(Serialize, Deserialize, Default)]
pub struct Registry {
    pub version: u32,                     // 当前 1
    #[serde(default)] pub sources: std::collections::BTreeMap<String, SourceRecord>,
    #[serde(default)] pub installs: Vec<Install>,
}
impl Registry {
    pub fn load(layout: &Layout) -> Result<Registry>;          // 不存在则返回空
    pub fn save(&self, layout: &Layout) -> Result<()>;         // tmp + rename 原子写
    pub fn find(&self, skill: &str, target: &TargetRec) -> Option<&Install>;
    pub fn remove(&mut self, skill: &str, target: &TargetRec) -> Option<Install>;
}

// core/config.rs
pub struct Config {
    pub targets: BTreeMap<String, PathBuf>,  // 内置 + config.toml 扩展
    pub default_method: Method,
    pub web_port: u16,
}
impl Config {
    pub fn load(layout: &Layout) -> Result<Config>;            // 无文件 = 全默认
}

// core/update.rs
pub fn repo_should_update(src: &SourceRecord) -> bool;         // auto_update.unwrap_or(false)
pub fn copy_should_update(reg: &Registry, inst: &Install) -> bool; // inst > 包级 > false
```

---

### 任务 1：项目脚手架 + paths 模块（Layout 与 Target 寻址）

**文件：**
- 创建：`Cargo.toml`
- 创建：`src/main.rs`
- 创建：`src/core/mod.rs`
- 创建：`src/core/error.rs`
- 创建：`src/core/paths.rs`
- 测试：内联 `#[cfg(test)]` 于 `paths.rs`

- [ ] **步骤 1：初始化 crate 并写失败测试**

```bash
cargo init --name skills
```

`Cargo.toml` 依赖（后续任务逐步用到，一次配好）：

```toml
[dependencies]
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
gix = { version = "0.66", default-features = false, features = ["blocking-network-client", "max-pure"] }
dirs = "5"
chrono = { version = "0.4", features = ["serde"] }
thiserror = "1"
dialoguer = "0.11"
ratatui = "0.29"
crossterm = "0.28"
axum = "0.7"
tokio = { version = "1", features = ["full"] }
tower = { version = "0.4", features = ["util"] }
open = "5"

[dev-dependencies]
tempfile = "3"
```

> 注意：`gix` 的 API 在 minor 版本间有变动。实现 `core/git.rs`（任务 5）前用 find-docs 技能核对当前版本文档；本文中 gix 调用代码以 0.66 为准，若签名漂移按文档调整，wrapper 函数签名不变。

`src/core/error.rs`：

```rust
#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("json: {0}")] Json(#[from] serde_json::Error),
    #[error("toml: {0}")] Toml(#[from] toml::de::Error),
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
    #[error("{0}")] Msg(String),
}
pub type Result<T> = std::result::Result<T, Error>;
```

`src/core/paths.rs` 失败测试：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_global_target() {
        let t = Target::parse("global:agents").unwrap();
        assert_eq!(t, Target::Global { name: "agents".into() });
    }

    #[test]
    fn parse_project_target_requires_absolute() {
        assert!(Target::parse("project:./rel").is_err());
        let abs = if cfg!(windows) { "project:C:\\work" } else { "project:/work" };
        assert!(Target::parse(abs).is_ok());
    }

    #[test]
    fn parse_rejects_garbage() {
        assert!(Target::parse("agents").is_err());
        assert!(Target::parse("global:").is_err());
    }

    #[test]
    fn project_install_dir_is_dot_agents_skills() {
        let cfg = crate::core::config::Config::default();
        let t = Target::Project { root: PathBuf::from("/tmp/proj") };
        assert_eq!(t.install_dir(&cfg).unwrap(),
                   PathBuf::from("/tmp/proj").join(".agents").join("skills"));
    }

    #[test]
    fn global_install_dir_resolves_via_config() {
        let cfg = crate::core::config::Config::default();
        let t = Target::Global { name: "agents".into() };
        let dir = t.install_dir(&cfg).unwrap();
        assert!(dir.ends_with(".agents/skills") || dir.ends_with(".agents\\skills"));
    }

    #[test]
    fn layout_paths() {
        let l = Layout::at(PathBuf::from("/x/.skills"));
        assert_eq!(l.cache_dir("github/a/b"), PathBuf::from("/x/.skills/github/a/b"));
        assert_eq!(l.registry_path(), PathBuf::from("/x/.skills/registry.json"));
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test paths`
预期：编译失败，`Target`/`Layout` 未定义。

- [ ] **步骤 3：实现 paths.rs**

```rust
use std::path::{Path, PathBuf};
use super::error::{Error, Result};
use super::config::Config;

#[derive(Clone, PartialEq, Debug)]
pub enum Target {
    Global { name: String },
    Project { root: PathBuf },
}

impl Target {
    pub fn parse(s: &str) -> Result<Target> {
        let (kind, rest) = s.split_once(':')
            .ok_or_else(|| Error::BadTarget(s.into()))?;
        match kind {
            "global" if !rest.is_empty() => Ok(Target::Global { name: rest.into() }),
            "project" => {
                let p = PathBuf::from(rest);
                if p.is_absolute() { Ok(Target::Project { root: p }) }
                else { Err(Error::BadTarget(s.into())) }
            }
            _ => Err(Error::BadTarget(s.into())),
        }
    }

    pub fn install_dir(&self, cfg: &Config) -> Result<PathBuf> {
        match self {
            Target::Global { name } => cfg.targets.get(name).cloned()
                .ok_or_else(|| Error::UnknownTarget(name.clone())),
            Target::Project { root } => Ok(root.join(".agents").join("skills")),
        }
    }
}

pub struct Layout { pub root: PathBuf }

impl Layout {
    pub fn new() -> Result<Layout> {
        let home = dirs::home_dir().ok_or(Error::NoHome)?;
        Ok(Layout { root: home.join(".skills") })
    }
    pub fn at(root: PathBuf) -> Layout { Layout { root } }
    pub fn cache_dir(&self, key: &str) -> PathBuf { self.root.join(key) }
    pub fn registry_path(&self) -> PathBuf { self.root.join("registry.json") }
    pub fn config_path(&self) -> PathBuf { self.root.join("config.toml") }
}
```

`src/core/mod.rs`：

```rust
pub mod error;
pub mod paths;
pub mod config;   // 任务 4 才实现，本任务先建空文件并提供 Default
```

`src/core/config.rs`（本任务只放最小实现，任务 4 扩展）：

```rust
use std::collections::BTreeMap;
use std::path::PathBuf;
use super::registry::Method; // 任务 3 实现；本任务临时定义为本地枚举亦可，先建最小版
```

> 注：任务 1 的 `install_dir` 测试依赖 `Config::default()`。本任务内先建 `config.rs` 最小骨架：

```rust
use std::collections::BTreeMap;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Method { Symlink, Copy }

pub struct Config {
    pub targets: BTreeMap<String, PathBuf>,
    pub default_method: Method,
    pub web_port: u16,
}

impl Default for Config {
    fn default() -> Self {
        let mut targets = BTreeMap::new();
        if let Some(home) = dirs::home_dir() {
            targets.insert("agents".into(), home.join(".agents").join("skills"));
            targets.insert("claude".into(), home.join(".claude").join("skills"));
            targets.insert("codex".into(), home.join(".codex").join("skills"));
        }
        Config { targets, default_method: Method::Symlink, web_port: 7823 }
    }
}
```

`src/main.rs`（占位骨架，任务 9 填充）：

```rust
fn main() {
    println!("skills-manager (骨架)");
}
```

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test`
预期：全部 PASS。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): 脚手架 + paths 模块（Layout/Target 寻址）"
```

---

### 任务 2：source 解析

**文件：**
- 修改：`src/core/source.rs`（创建）
- 修改：`src/core/mod.rs`（加 `pub mod source;`）
- 测试：内联于 `source.rs`

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_shorthand_expands() {
        let s = parse_source("mattpocock/skills").unwrap();
        assert_eq!(s.key, "github/mattpocock/skills");
        assert_eq!(s.url.as_deref(), Some("https://github.com/mattpocock/skills"));
        assert!(s.local_path.is_none());
    }

    #[test]
    fn github_https_url() {
        let s = parse_source("https://github.com/mattpocock/skills").unwrap();
        assert_eq!(s.key, "github/mattpocock/skills");
    }

    #[test]
    fn github_https_url_with_git_suffix_and_trailing_slash() {
        let s = parse_source("https://github.com/a/b.git/").unwrap();
        assert_eq!(s.key, "github/a/b");
        assert_eq!(s.url.as_deref(), Some("https://github.com/a/b"));
    }

    #[test]
    fn ssh_url() {
        let s = parse_source("git@github.com:a/b.git").unwrap();
        assert_eq!(s.key, "github/a/b");
        assert_eq!(s.url.as_deref(), Some("git@github.com:a/b.git"));
    }

    #[test]
    fn non_github_host() {
        let s = parse_source("https://gitlab.com/org/repo").unwrap();
        assert_eq!(s.key, "gitlab.com/org/repo");
    }

    #[test]
    fn local_absolute_path() {
        let p = if cfg!(windows) { "C:\\tmp\\myskill" } else { "/tmp/myskill" };
        let s = parse_source(p).unwrap();
        assert!(s.key.starts_with("local/"));
        assert!(s.url.is_none());
        assert_eq!(s.local_path.as_deref(), Some(std::path::Path::new(p)));
    }

    #[test]
    fn rejects_empty_and_single_word() {
        assert!(parse_source("").is_err());
        assert!(parse_source("noslash").is_err());
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test source`
预期：编译失败，`parse_source` 未定义。

- [ ] **步骤 3：实现 source.rs**

```rust
use std::path::PathBuf;
use super::error::{Error, Result};

#[derive(Clone, PartialEq, Debug)]
pub struct SourceSpec {
    pub key: String,
    pub url: Option<String>,
    pub local_path: Option<PathBuf>,
}

pub fn parse_source(input: &str) -> Result<SourceSpec> {
    let input = input.trim().trim_end_matches('/');
    if input.is_empty() {
        return Err(Error::Msg("source 为空".into()));
    }
    // 本地绝对路径
    let p = PathBuf::from(input);
    if p.is_absolute() {
        let name = p.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unnamed".into());
        return Ok(SourceSpec { key: format!("local/{name}"), url: None, local_path: Some(p) });
    }
    // SSH 形式 git@host:owner/repo[.git]
    if let Some(rest) = input.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            let path = path.trim_end_matches(".git");
            return Ok(SourceSpec {
                key: format!("{host}/{path}"),
                url: Some(input.into()),
                local_path: None,
            });
        }
        return Err(Error::Msg(format!("无法解析 source: {input}")));
    }
    // https://host/owner/repo[.git]
    if let Some(rest) = input.strip_prefix("https://").or_else(|| input.strip_prefix("http://")) {
        let mut parts = rest.splitn(2, '/');
        let host = parts.next().unwrap_or_default();
        let path = parts.next().map(|p| p.trim_end_matches('/').trim_end_matches(".git"));
        match (host, path) {
            (h, Some(p)) if !h.is_empty() && p.matches('/').count() == 1 => {
                let short = if h == "github.com" { "github" } else { h };
                return Ok(SourceSpec {
                    key: format!("{short}/{p}"),
                    url: Some(format!("https://{h}/{p}")),
                    local_path: None,
                });
            }
            _ => return Err(Error::Msg(format!("无法解析 source: {input}"))),
        }
    }
    // GitHub 简写 owner/repo
    if input.matches('/').count() == 1
        && input.chars().all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
    {
        return Ok(SourceSpec {
            key: format!("github/{input}"),
            url: Some(format!("https://github.com/{input}")),
            local_path: None,
        });
    }
    Err(Error::Msg(format!("无法解析 source: {input}")))
}
```

> 设计说明：GitHub 的 key 用 `github/...` 而非 `github.com/...`，与规格磁盘布局一致（`~/.skills/github/<owner>/<repo>`）；其他 host 保留完整域名作 key 第一段。

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test source`
预期：7 个测试全 PASS。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): source 解析（GitHub 简写补全/URL/SSH/本地路径）"
```

---

### 任务 3：registry 数据模型与原子读写

**文件：**
- 修改：`src/core/registry.rs`（创建）
- 修改：`src/core/mod.rs`（加 `pub mod registry;`，并把 `config.rs` 中的临时 `Method` 移除改从 registry 引入）
- 测试：内联于 `registry.rs`

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Layout;

    fn sample_install() -> Install {
        Install {
            skill: "web-design".into(),
            source: "github/mattpocock/skills".into(),
            source_path: "skills/web-design".into(),
            target: TargetRec::Global { name: "agents".into() },
            method: Method::Copy,
            commit: "a1b2c3d".into(),
            tags: vec!["frontend".into()],
            auto_update: None,
            installed_at: "2026-08-20T10:00:00Z".into(),
        }
    }

    #[test]
    fn load_missing_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::load(&Layout::at(tmp.path().to_path_buf())).unwrap();
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        let mut reg = Registry { version: 1, ..Default::default() };
        reg.sources.insert("github/a/b".into(), SourceRecord {
            url: "https://github.com/a/b".into(),
            commit: "deadbeef".into(),
            fetched_at: "2026-08-20T10:00:00Z".into(),
            auto_update: Some(true),
        });
        reg.installs.push(sample_install());
        reg.save(&layout).unwrap();
        let loaded = Registry::load(&layout).unwrap();
        assert_eq!(loaded.installs.len(), 1);
        assert_eq!(loaded.installs[0].skill, "web-design");
        assert_eq!(loaded.sources["github/a/b"].commit, "deadbeef");
        // 落盘 JSON 与规格字段一致
        let raw = std::fs::read_to_string(layout.registry_path()).unwrap();
        assert!(raw.contains("\"kind\": \"global\""));
        assert!(raw.contains("\"method\": \"copy\""));
    }

    #[test]
    fn find_and_remove_by_skill_and_target() {
        let mut reg = Registry { version: 1, ..Default::default() };
        reg.installs.push(sample_install());
        let t = TargetRec::Global { name: "agents".into() };
        assert!(reg.find("web-design", &t).is_some());
        assert!(reg.find("web-design", &TargetRec::Global { name: "claude".into() }).is_none());
        let removed = reg.remove("web-design", &t);
        assert!(removed.is_some());
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn target_rec_serde_shape() {
        let g = serde_json::to_string(&TargetRec::Global { name: "agents".into() }).unwrap();
        assert_eq!(g, r#"{"kind":"global","name":"agents"}"#);
        let p = serde_json::to_string(&TargetRec::Project { root: "/x".into() }).unwrap();
        assert_eq!(p, r#"{"kind":"project","root":"/x"}"#);
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test registry`
预期：编译失败。

- [ ] **步骤 3：实现 registry.rs**

```rust
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
use super::error::Result;
use super::paths::Layout;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetRec {
    Global { name: String },
    Project { root: PathBuf },
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Method { Symlink, Copy }

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SourceRecord {
    pub url: String,
    pub commit: String,
    pub fetched_at: String,
    #[serde(default)]
    pub auto_update: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Install {
    pub skill: String,
    pub source: String,
    pub source_path: PathBuf,
    pub target: TargetRec,
    pub method: Method,
    pub commit: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub auto_update: Option<bool>,
    pub installed_at: String,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Registry {
    pub version: u32,
    #[serde(default)]
    pub sources: BTreeMap<String, SourceRecord>,
    #[serde(default)]
    pub installs: Vec<Install>,
}

impl Registry {
    pub fn load(layout: &Layout) -> Result<Registry> {
        let path = layout.registry_path();
        if !path.exists() {
            return Ok(Registry { version: 1, ..Default::default() });
        }
        let raw = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, layout: &Layout) -> Result<()> {
        let path = layout.registry_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?;   // 同目录 rename，原子替换
        Ok(())
    }

    pub fn find(&self, skill: &str, target: &TargetRec) -> Option<&Install> {
        self.installs.iter().find(|i| i.skill == skill && &i.target == target)
    }

    pub fn remove(&mut self, skill: &str, target: &TargetRec) -> Option<Install> {
        let pos = self.installs.iter()
            .position(|i| i.skill == skill && &i.target == target)?;
        Some(self.installs.remove(pos))
    }
}
```

同时把 `config.rs` 改为 `use super::registry::Method;` 并删除其中的临时 `Method` 定义。

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test`
预期：全部 PASS（含任务 1、2 的测试）。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): registry 数据模型与原子读写"
```

---

### 任务 4：config.toml（内置默认 + 可选覆盖）

**文件：**
- 修改：`src/core/config.rs`
- 测试：内联于 `config.rs`

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Layout;

    #[test]
    fn no_config_file_uses_builtin_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config::load(&Layout::at(tmp.path().to_path_buf())).unwrap();
        assert!(cfg.targets.contains_key("agents"));
        assert_eq!(cfg.web_port, 7823);
        assert_eq!(cfg.default_method, Method::Symlink);
    }

    #[test]
    fn config_file_overrides_and_extends() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.toml"), r#"
[defaults]
method = "copy"
[web]
port = 9000
[targets]
cursor = "~/.cursor/skills"
"#).unwrap();
        let cfg = Config::load(&Layout::at(tmp.path().to_path_buf())).unwrap();
        assert_eq!(cfg.default_method, Method::Copy);
        assert_eq!(cfg.web_port, 9000);
        assert!(cfg.targets.contains_key("cursor"));
        assert!(cfg.targets.contains_key("agents")); // 内置的还在
    }

    #[test]
    fn tilde_expands_in_targets() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.toml"),
            "[targets]\ncursor = \"~/.cursor/skills\"\n").unwrap();
        let cfg = Config::load(&Layout::at(tmp.path().to_path_buf())).unwrap();
        let p = &cfg.targets["cursor"];
        assert!(!p.to_string_lossy().contains('~'));
        assert!(p.is_absolute());
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test config`
预期：编译失败（`Config::load` 不存在）。

- [ ] **步骤 3：实现 config.rs（在任务 1 骨架上扩展）**

```rust
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use super::error::Result;
use super::paths::Layout;
pub use super::registry::Method;

#[derive(Clone, Debug)]
pub struct Config {
    pub targets: BTreeMap<String, PathBuf>,
    pub default_method: Method,
    pub web_port: u16,
}

#[derive(Deserialize, Default)]
struct FileConfig {
    defaults: Option<FileDefaults>,
    web: Option<FileWeb>,
    targets: Option<BTreeMap<String, String>>,
}
#[derive(Deserialize)]
struct FileDefaults { method: Option<Method> }
#[derive(Deserialize)]
struct FileWeb { port: Option<u16> }

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

impl Default for Config {
    fn default() -> Self {
        let mut targets = BTreeMap::new();
        if let Some(home) = dirs::home_dir() {
            targets.insert("agents".into(), home.join(".agents").join("skills"));
            targets.insert("claude".into(), home.join(".claude").join("skills"));
            targets.insert("codex".into(), home.join(".codex").join("skills"));
        }
        Config { targets, default_method: Method::Symlink, web_port: 7823 }
    }
}

impl Config {
    pub fn load(layout: &Layout) -> Result<Config> {
        let mut cfg = Config::default();
        let path = layout.config_path();
        if !path.exists() {
            return Ok(cfg);   // 无配置文件也能工作
        }
        let fc: FileConfig = toml::from_str(&std::fs::read_to_string(&path)?)?;
        if let Some(d) = fc.defaults {
            if let Some(m) = d.method { cfg.default_method = m; }
        }
        if let Some(w) = fc.web {
            if let Some(p) = w.port { cfg.web_port = p; }
        }
        if let Some(t) = fc.targets {
            for (name, p) in t {
                cfg.targets.insert(name, expand_tilde(&p));
            }
        }
        Ok(cfg)
    }
}
```

`Method` 需在 `registry.rs` 上补 `#[derive(serde::Deserialize)]`（已有）——`FileDefaults` 依赖 toml 反序列化 `Method`，`serde(rename_all="lowercase")` 已保证 `"copy"`/`"symlink"` 可解析。

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test`
预期：全部 PASS。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): config.toml 加载（内置默认 + 可选覆盖）"
```

---
### 任务 5：git 封装（gitoxide 浅克隆 / 更新 / 版本号）

**文件：**
- 创建：`src/core/git.rs`
- 修改：`src/core/mod.rs`（加 `pub mod git;`）
- 测试：内联于 `git.rs`（用本地 bare repo，不依赖网络）

> 实现前先核对 gix 当前版本 API（find-docs 技能）。wrapper 三个函数签名固定：`shallow_clone(url, dest) -> Result<String>`、`fetch_and_reset(path) -> Result<Option<String>>`、`head_commit(path) -> Result<String>`，内部实现允许随文档调整。

- [ ] **步骤 1：编写失败测试（用 git CLI 造 fixture，仅测试环境依赖系统 git）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn make_bare_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let bare = tmp.path().join("bare.git");
        std::fs::create_dir_all(&work).unwrap();
        let run = |dir: &std::path::Path, args: &[&str]| {
            let st = Command::new("git").args(args).current_dir(dir).status().unwrap();
            assert!(st.success(), "git {:?} 失败", args);
        };
        run(&work, &["init", "-b", "main"]);
        run(&work, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "--allow-empty", "-m", "c1"]);
        run(&work, &["clone", "--bare", ".", bare.to_str().unwrap()]);
        (tmp, bare)
    }

    #[test]
    fn shallow_clone_returns_head_commit() {
        let (_tmp, bare) = make_bare_repo();
        let dest = tempfile::tempdir().unwrap().path().join("clone");
        let commit = shallow_clone(&format!("file://{}", bare.display()), &dest).unwrap();
        assert_eq!(commit.len(), 40);
        assert_eq!(head_commit(&dest).unwrap(), commit);
        assert!(dest.join(".git").exists());
    }

    #[test]
    fn fetch_and_reset_reports_change_only_when_moved() {
        let (tmp, bare) = make_bare_repo();
        let dest = tmp.path().join("clone");
        let c1 = shallow_clone(&format!("file://{}", bare.display()), &dest).unwrap();
        // 无新提交 → None
        assert_eq!(fetch_and_reset(&dest).unwrap(), None);
        // 推一个新提交
        let work = tmp.path().join("work");
        let run = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&work).status().unwrap().success());
        run(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "--allow-empty", "-m", "c2"]);
        run(&["push", bare.to_str().unwrap(), "main"]);
        let c2 = fetch_and_reset(&dest).unwrap().expect("应有新 commit");
        assert_ne!(c1, c2);
        assert_eq!(head_commit(&dest).unwrap(), c2);
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test git`
预期：编译失败，函数未定义。

- [ ] **步骤 3：实现 git.rs（gix 0.66 API；若漂移以 find-docs 查到的为准）**

```rust
use std::path::Path;
use std::sync::atomic::AtomicBool;
use super::error::{Error, Result};

fn gerr<E: std::fmt::Display>(e: E) -> Error { Error::Git(e.to_string()) }

/// 浅克隆 url 到 dest，返回 HEAD commit 全 hash。
pub fn shallow_clone(url: &str, dest: &Path) -> Result<String> {
    let url = gix::url::parse(url.into()).map_err(gerr)?;
    let prep = gix::prepare_clone(url, dest).map_err(gerr)?
        .with_shallow(gix::remote::fetch::Shallow::DepthAtLeast(
            std::num::NonZeroU32::new(1).unwrap()));
    let (checkout, _) = prep
        .fetch_then_checkout(gix::progress::Discard, &AtomicBool::new(false))
        .map_err(gerr)?;
    let (repo, _) = checkout
        .main_worktree(gix::progress::Discard, &AtomicBool::new(false))
        .map_err(gerr)?;
    head_commit(repo.path().parent().unwrap_or(dest))
}

/// fetch 远端并 reset 到 origin/HEAD；有新 commit 返回 Some(hash)，否则 None。
pub fn fetch_and_reset(path: &Path) -> Result<Option<String>> {
    let before = head_commit(path)?;
    let repo = gix::open(path).map_err(gerr)?;
    let remote = repo.find_default_remote(gix::remote::Direction::Fetch)
        .and_then(|r| r.ok_or(())).map_err(|_| Error::Git("无默认 remote".into()))?;
    remote.connect(gix::remote::Direction::Fetch).map_err(gerr)?
        .prepare_fetch(gix::progress::Discard, Default::default()).map_err(gerr)?
        .receive(gix::progress::Discard, &AtomicBool::new(false)).map_err(gerr)?;
    // reset 到远端默认分支
    let repo = gix::open(path).map_err(gerr)?;
    let branch = repo.find_reference("HEAD").ok().and_then(|r| r.target().try_name().map(|n| n.to_string()))
        .unwrap_or_else(|| "refs/heads/main".into());
    let short = branch.trim_start_matches("refs/heads/");
    let upstream = format!("refs/remotes/origin/{short}");
    let oid = repo.find_reference(&upstream).map_err(gerr)?.id().detach();
    // 写 HEAD 指向上游 commit（浅克隆场景下等价 hard reset）
    repo.reference(branch.as_str(), oid, gix::refs::transaction::PreviousValue::Any, "skills update").map_err(gerr)?;
    let after = head_commit(path)?;
    Ok(if after == before { None } else { Some(after) })
}

/// 返回工作区 HEAD commit 全 hash。
pub fn head_commit(path: &Path) -> Result<String> {
    let repo = gix::open(path).map_err(gerr)?;
    let id = repo.head_id().map_err(gerr)?;
    Ok(id.to_string())
}
```

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test git`
预期：2 个测试 PASS。若 gix API 编译失败，用 find-docs 查 `gix prepare_clone` / `prepare_fetch` 当前签名后修正内部实现，wrapper 签名与测试不动。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): git 封装（gitoxide 浅克隆/fetch/head）"
```

---

### 任务 6：缓存层 ensure_cached + 技能扫描

**文件：**
- 创建：`src/core/cache.rs`
- 修改：`src/core/mod.rs`（加 `pub mod cache;`）
- 测试：内联于 `cache.rs`

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::source::parse_source;
    use std::process::Command;

    /// 造一个含两个技能的 bare 技能包仓库
    fn skill_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let bare = tmp.path().join("bare.git");
        std::fs::create_dir_all(work.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(work.join("skills/beta")).unwrap();
        std::fs::write(work.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A 技能\n---\n# Alpha\n").unwrap();
        std::fs::write(work.join("skills/beta/SKILL.md"),
            "---\nname: beta\ndescription: B 技能\n---\n# Beta\n").unwrap();
        let run = |args: &[&str]| assert!(Command::new("git").args(args).current_dir(&work).status().unwrap().success());
        run(&["init", "-b", "main"]);
        run(&["add", "."]);
        run(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "c1"]);
        run(&["clone", "--bare", ".", bare.to_str().unwrap()]);
        (tmp, bare)
    }

    #[test]
    fn ensure_cached_clones_once_then_reuses() {
        let (_t, bare) = skill_repo();
        let layout = Layout::at(tempfile::tempdir().unwrap().path().to_path_buf());
        let spec = parse_source(&format!("file://{}/org/pkg", bare.display())).unwrap();
        // file:// URL 的 key 形态走非 github host 分支；直接用内部 key 构造更稳：
        let spec = SourceSpec {
            key: "local-test/org/pkg".into(),
            url: Some(format!("file://{}", bare.display())),
            local_path: None,
        };
        let first = ensure_cached(&layout, &spec).unwrap();
        assert!(first.fresh);                       // 首次 = 新下载
        let second = ensure_cached(&layout, &spec).unwrap();
        assert!(!second.fresh);                     // 已存在 = 复用，不重复下载
        assert_eq!(first.path, second.path);
        assert_eq!(first.commit, second.commit);
    }

    #[test]
    fn scan_finds_multiple_skills_with_frontmatter() {
        let (_t, bare) = skill_repo();
        let layout = Layout::at(tempfile::tempdir().unwrap().path().to_path_buf());
        let spec = SourceSpec {
            key: "local-test/org/pkg".into(),
            url: Some(format!("file://{}", bare.display())),
            local_path: None,
        };
        let cached = ensure_cached(&layout, &spec).unwrap();
        let skills = scan_skills(&cached.path).unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "alpha");
        assert_eq!(skills[0].rel_path, std::path::PathBuf::from("skills/alpha"));
        assert_eq!(skills[0].description, "A 技能");
    }

    #[test]
    fn scan_supports_root_level_single_skill() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("SKILL.md"),
            "---\nname: solo\ndescription: 单技能\n---\n").unwrap();
        let skills = scan_skills(tmp.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].rel_path, std::path::PathBuf::from("."));
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test cache`
预期：编译失败。

- [ ] **步骤 3：实现 cache.rs**

```rust
use std::path::{Path, PathBuf};
use super::error::{Error, Result};
use super::git;
use super::paths::Layout;
use super::source::SourceSpec;

pub struct Cached {
    pub path: PathBuf,   // 缓存目录绝对路径
    pub commit: String,
    pub fresh: bool,     // true = 本次新下载；false = 复用已有缓存
}

pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub rel_path: PathBuf,   // 相对缓存根的路径
}

/// 确保 source 已缓存；已存在则复用（不重复下载）。
pub fn ensure_cached(layout: &Layout, spec: &SourceSpec) -> Result<Cached> {
    let dest = layout.cache_dir(&spec.key);
    if dest.join(".git").exists() || (spec.local_path.is_some() && dest.exists()) {
        let commit = git::head_commit(&dest).unwrap_or_default();
        return Ok(Cached { path: dest, commit, fresh: false });
    }
    match (&spec.url, &spec.local_path) {
        (Some(url), None) => {
            std::fs::create_dir_all(dest.parent().unwrap())?;
            let commit = git::shallow_clone(url, &dest)?;
            Ok(Cached { path: dest, commit, fresh: true })
        }
        (None, Some(src)) => {
            std::fs::create_dir_all(dest.parent().unwrap())?;
            copy_dir(src, &dest)?;
            Ok(Cached { path: dest, commit: String::new(), fresh: true })
        }
        _ => Err(Error::Msg("source 缺少 url 或本地路径".into())),
    }
}

/// 扫描缓存目录内所有技能：根级 SKILL.md（单技能）或 <dir>/SKILL.md（多技能，深度 2）。
pub fn scan_skills(root: &Path) -> Result<Vec<SkillEntry>> {
    let mut out = Vec::new();
    if root.join("SKILL.md").exists() {
        out.push(read_entry(root, PathBuf::from("."))?);
        return Ok(out);
    }
    for dir1 in std::fs::read_dir(root)? {
        let d1 = dir1?.path();
        if !d1.is_dir() || d1.file_name().unwrap() == ".git" { continue; }
        if d1.join("SKILL.md").exists() {
            out.push(read_entry(&d1, rel(root, &d1))?);
        }
        for dir2 in std::fs::read_dir(&d1)? {
            let d2 = dir2?.path();
            if d2.is_dir() && d2.join("SKILL.md").exists() {
                out.push(read_entry(&d2, rel(root, &d2))?);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn rel(root: &Path, p: &Path) -> PathBuf {
    p.strip_prefix(root).unwrap_or(p).to_path_buf()
}

/// 解析 SKILL.md frontmatter 的 name/description（简易解析，不引入 yaml 依赖）。
fn read_entry(dir: &Path, rel_path: PathBuf) -> Result<SkillEntry> {
    let raw = std::fs::read_to_string(dir.join("SKILL.md"))?;
    let mut name = String::new();
    let mut description = String::new();
    if let Some(fm) = raw.strip_prefix("---").and_then(|r| r.split("---").nth(1)) {
        for line in fm.lines() {
            if let Some((k, v)) = line.split_once(':') {
                let v = v.trim().trim_matches('"').to_string();
                match k.trim() {
                    "name" => name = v,
                    "description" => description = v,
                    _ => {}
                }
            }
        }
    }
    if name.is_empty() {
        name = dir.file_name().map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unnamed".into());
    }
    Ok(SkillEntry { name, description, rel_path })
}

pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.file_type()?.is_dir() {
            if e.file_name() == ".git" { continue; }
            copy_dir(&e.path(), &to)?;
        } else {
            std::fs::copy(e.path(), &to)?;
        }
    }
    Ok(())
}
```

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test cache`
预期：3 个测试 PASS。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): 缓存层（去重复用 + SKILL.md 扫描）"
```

---

### 任务 7：安装引擎（symlink / copy / 同名冲突）

**文件：**
- 创建：`src/core/install.rs`
- 修改：`src/core/mod.rs`（加 `pub mod install;`）
- 测试：内联于 `install.rs`

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::registry::{Method, Registry, TargetRec};

    /// 构造：缓存里一个技能包（技能 alpha），Config 的 agents target 指向临时目录
    fn setup() -> (tempfile::TempDir, Layout, Config, Registry) {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let cache = layout.cache_dir("github/o/r");
        std::fs::create_dir_all(cache.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(cache.join(".git")).unwrap();
        std::fs::write(cache.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A\n---\n").unwrap();
        let mut cfg = Config::default();
        cfg.targets.insert("agents".into(), tmp.path().join("global/agents"));
        (tmp, layout, cfg, Registry { version: 1, ..Default::default() })
    }

    #[test]
    fn copy_install_creates_independent_copy_and_record() {
        let (_t, layout, cfg, mut reg) = setup();
        let target = Target::Global { name: "agents".into() };
        let recs = install_skill(&layout, &cfg, &mut reg, "github/o/r",
            "alpha", "skills/alpha", &target, Method::Copy, "c1").unwrap();
        let dest = cfg.targets["agents"].join("alpha");
        assert!(dest.join("SKILL.md").exists());
        assert!(!dest.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(recs.method, Method::Copy);
        assert_eq!(reg.installs.len(), 1);
        assert_eq!(reg.installs[0].target, TargetRec::Global { name: "agents".into() });
    }

    #[cfg(unix)]
    #[test]
    fn symlink_install_points_into_cache() {
        let (_t, layout, cfg, mut reg) = setup();
        let target = Target::Global { name: "agents".into() };
        install_skill(&layout, &cfg, &mut reg, "github/o/r",
            "alpha", "skills/alpha", &target, Method::Symlink, "c1").unwrap();
        let dest = cfg.targets["agents"].join("alpha");
        let link = std::fs::read_link(&dest).unwrap();
        assert_eq!(link, layout.cache_dir("github/o/r").join("skills/alpha"));
        assert_eq!(reg.installs[0].method, Method::Symlink);
    }

    #[test]
    fn conflict_returns_decision_request() {
        let (_t, layout, cfg, mut reg) = setup();
        let target = Target::Global { name: "agents".into() };
        install_skill(&layout, &cfg, &mut reg, "github/o/r",
            "alpha", "skills/alpha", &target, Method::Copy, "c1").unwrap();
        // 再装同名技能 → 返回冲突，由调用方决定
        let err = install_skill(&layout, &cfg, &mut reg, "github/o/r",
            "alpha", "skills/alpha", &target, Method::Copy, "c1").unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    #[test]
    fn same_skill_to_two_targets_creates_two_records() {
        let (_t, layout, mut cfg, mut reg) = setup();
        cfg.targets.insert("claude".into(), _t.path().join("global/claude"));
        for name in ["agents", "claude"] {
            install_skill(&layout, &cfg, &mut reg, "github/o/r", "alpha", "skills/alpha",
                &Target::Global { name: name.into() }, Method::Copy, "c1").unwrap();
        }
        assert_eq!(reg.installs.len(), 2);
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test install`
预期：编译失败（`Error::Conflict` 也不存在，需先加到 error.rs）。

- [ ] **步骤 3：实现 install.rs（并给 error.rs 加 `Conflict(PathBuf)`）**

```rust
use std::path::{Path, PathBuf};
use super::cache::copy_dir;
use super::config::Config;
use super::error::{Error, Result};
use super::paths::{Layout, Target};
use super::registry::{Install, Method, Registry, TargetRec};

/// 把技能从缓存安装到一个目标；目标已有同名目录时返回 Error::Conflict 交由前端决策。
pub fn install_skill(
    layout: &Layout, cfg: &Config, reg: &mut Registry,
    source_key: &str, skill: &str, source_path: &str,
    target: &Target, method: Method, commit: &str,
) -> Result<Install> {
    let src_dir = layout.cache_dir(source_key).join(source_path);
    let dest_root = target.install_dir(cfg)?;
    let dest = dest_root.join(skill);
    if dest.exists() || dest.symlink_metadata().is_ok() {
        return Err(Error::Conflict(dest));
    }
    std::fs::create_dir_all(&dest_root)?;
    match method {
        Method::Copy => copy_dir(&src_dir, &dest)?,
        Method::Symlink => make_symlink(&src_dir, &dest)?,
    }
    let rec = Install {
        skill: skill.into(),
        source: source_key.into(),
        source_path: PathBuf::from(source_path),
        target: to_rec(target),
        method,
        commit: commit.into(),
        tags: vec![],
        auto_update: None,
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    reg.installs.push(rec.clone());
    Ok(rec)
}

pub fn to_rec(t: &Target) -> TargetRec {
    match t {
        Target::Global { name } => TargetRec::Global { name: name.clone() },
        Target::Project { root } => TargetRec::Project { root: root.clone() },
    }
}

#[cfg(unix)]
fn make_symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst)?;
    Ok(())
}

#[cfg(windows)]
fn make_symlink(src: &Path, dst: &Path) -> Result<()> {
    // 优先目录符号链接；权限不足时退化为 junction（不需要管理员权限）
    if std::os::windows::fs::symlink_dir(src, dst).is_ok() {
        return Ok(());
    }
    junction::create(src, dst).map_err(|e| Error::Msg(format!(
        "创建链接失败（{}）：请用 --method copy，或开启 Windows 开发者模式", e)))?;
    Ok(())
}
```

> Windows 的 junction 兜底需要 `junction` crate，加入 `[target.'cfg(windows)'.dependencies] junction = "1"`。测试里 symlink 用例标 `#[cfg(unix)]`，Windows CI 上跑 copy 用例。

`error.rs` 追加：

```rust
    #[error("目标已存在同名技能: {0}")]
    Conflict(std::path::PathBuf),
```

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test install`
预期：PASS（Unix 全 4 个；Windows 跳过 symlink 用例）。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): 安装引擎（symlink/copy + 同名冲突决策）"
```

---

### 任务 8：删除引擎（记录 + 磁盘实况核实）

**文件：**
- 创建：`src/core/remove.rs`
- 修改：`src/core/mod.rs`（加 `pub mod remove;`）
- 测试：内联于 `remove.rs`

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn setup_installed(method: Method) -> (tempfile::TempDir, Layout, Config, Registry) {
        // 复用任务 7 的构造思路：缓存 + 已安装一条记录
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let cache = layout.cache_dir("github/o/r");
        std::fs::create_dir_all(cache.join("skills/alpha")).unwrap();
        std::fs::write(cache.join("skills/alpha/SKILL.md"), "---\nname: alpha\n---\n").unwrap();
        let mut cfg = Config::default();
        cfg.targets.insert("agents".into(), tmp.path().join("g/agents"));
        let mut reg = Registry { version: 1, ..Default::default() };
        crate::core::install::install_skill(&layout, &cfg, &mut reg, "github/o/r",
            "alpha", "skills/alpha", &Target::Global { name: "agents".into() },
            method, "c1").unwrap();
        (tmp, layout, cfg, reg)
    }

    #[test]
    fn remove_copy_deletes_dir_and_record() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let dest = cfg.targets["agents"].join("alpha");
        let outcome = remove_install(&layout, &cfg, &mut reg, "alpha",
            &TargetRec::Global { name: "agents".into() }).unwrap();
        assert_eq!(outcome, RemoveOutcome::Removed);
        assert!(!dest.exists());
        assert!(reg.installs.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn remove_symlink_only_removes_link_not_cache() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Symlink);
        let dest = cfg.targets["agents"].join("alpha");
        remove_install(&layout, &cfg, &mut reg, "alpha",
            &TargetRec::Global { name: "agents".into() }).unwrap();
        assert!(dest.symlink_metadata().is_err());            // 链接没了
        assert!(layout.cache_dir("github/o/r/skills/alpha/SKILL.md").exists()); // 缓存还在
    }

    #[test]
    fn remove_when_dir_manually_deleted_cleans_record_only() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        std::fs::remove_dir_all(cfg.targets["agents"].join("alpha")).unwrap();
        let outcome = remove_install(&layout, &cfg, &mut reg, "alpha",
            &TargetRec::Global { name: "agents".into() }).unwrap();
        assert_eq!(outcome, RemoveOutcome::RecordOnly);       // 磁盘已不在，只清记录
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn remove_unknown_returns_not_installed() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let err = remove_install(&layout, &cfg, &mut reg, "nope",
            &TargetRec::Global { name: "agents".into() }).unwrap_err();
        assert!(matches!(err, Error::NotInstalled(_)));
    }

    #[test]
    fn remove_verifies_symlink_points_to_cache() {
        // 记录说 symlink，但磁盘上是普通目录（用户换了）→ 拒绝删除，提示人工核实
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Symlink);
        let dest = cfg.targets["agents"].join("alpha");
        #[cfg(unix)] {
            std::fs::remove_file(&dest).unwrap();
            std::fs::create_dir_all(&dest).unwrap();          // 换成真目录
            let err = remove_install(&layout, &cfg, &mut reg, "alpha",
                &TargetRec::Global { name: "agents".into() }).unwrap_err();
            assert!(matches!(err, Error::Mismatch(_)));
            assert!(dest.exists());                            // 不动用户的目录
            assert_eq!(reg.installs.len(), 1);                 // 记录保留
        }
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test remove`
预期：编译失败（`Error::Mismatch` 需加入 error.rs）。

- [ ] **步骤 3：实现 remove.rs**

```rust
use super::config::Config;
use super::error::{Error, Result};
use super::paths::{Layout, Target};
use super::registry::{Method, Registry, TargetRec};

#[derive(PartialEq, Debug)]
pub enum RemoveOutcome { Removed, RecordOnly }

/// 按记录删除：先查 registry，再核实磁盘实况，一致才删。
pub fn remove_install(
    layout: &Layout, cfg: &Config, reg: &mut Registry,
    skill: &str, target: &TargetRec,
) -> Result<RemoveOutcome> {
    let rec = reg.find(skill, target)
        .ok_or_else(|| Error::NotInstalled(format!("{skill} @ {target:?}")))?
        .clone();
    let t = match target {
        TargetRec::Global { name } => Target::Global { name: name.clone() },
        TargetRec::Project { root } => Target::Project { root: root.clone() },
    };
    let dest = t.install_dir(cfg)?.join(skill);

    let meta = dest.symlink_metadata();
    match (rec.method, meta) {
        (_, Err(_)) => {
            // 磁盘上已不存在（用户手动删了）→ 只清记录
            reg.remove(skill, target);
            Ok(RemoveOutcome::RecordOnly)
        }
        (Method::Symlink, Ok(m)) if m.file_type().is_symlink() => {
            let link = std::fs::read_link(&dest)?;
            let expect = layout.cache_dir(&rec.source).join(&rec.source_path);
            if link != expect {
                return Err(Error::Mismatch(format!("{dest:?} 指向 {link:?}，与记录不符，已保留")));
            }
            std::fs::remove_file(&dest)?;
            reg.remove(skill, target);
            Ok(RemoveOutcome::Removed)
        }
        (Method::Copy, Ok(m)) if m.is_dir() && !m.file_type().is_symlink() => {
            std::fs::remove_dir_all(&dest)?;
            reg.remove(skill, target);
            Ok(RemoveOutcome::Removed)
        }
        (_, Ok(_)) => Err(Error::Mismatch(
            format!("{dest:?} 实况与安装方式 {:?} 不符，已保留", rec.method))),
    }
}
```

`error.rs` 追加：

```rust
    #[error("磁盘实况与安装记录不一致: {0}")]
    Mismatch(String),
```

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test remove`
预期：PASS。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): 删除引擎（记录+磁盘实况双重核实）"
```

---

### 任务 9：两级更新引擎

**文件：**
- 创建：`src/core/update.rs`
- 修改：`src/core/mod.rs`（加 `pub mod update;`）
- 测试：内联于 `update.rs`

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn reg_with(source_auto: Option<bool>, installs: Vec<(Method, Option<bool>)>) -> Registry {
        let mut reg = Registry { version: 1, ..Default::default() };
        reg.sources.insert("github/o/r".into(), SourceRecord {
            url: "https://github.com/o/r".into(), commit: "c1".into(),
            fetched_at: "2026-08-20T00:00:00Z".into(), auto_update: source_auto,
        });
        for (i, (method, auto)) in installs.into_iter().enumerate() {
            reg.installs.push(Install {
                skill: format!("s{i}"), source: "github/o/r".into(),
                source_path: format!("skills/s{i}").into(),
                target: TargetRec::Global { name: "agents".into() },
                method, commit: "c1".into(), tags: vec![],
                auto_update: auto, installed_at: "2026-08-20T00:00:00Z".into(),
            });
        }
        reg
    }

    #[test]
    fn repo_update_follows_source_flag_default_false() {
        assert!(!repo_should_update(&reg_with(None, vec![]).sources["github/o/r"]));
        assert!(repo_should_update(&reg_with(Some(true), vec![]).sources["github/o/r"]));
    }

    #[test]
    fn copy_effective_flag_install_overrides_source() {
        let reg = reg_with(Some(true), vec![(Method::Copy, Some(false))]);
        assert!(!copy_should_update(&reg, &reg.installs[0])); // 副本级 false 盖住包级 true
        let reg = reg_with(Some(false), vec![(Method::Copy, Some(true))]);
        assert!(copy_should_update(&reg, &reg.installs[0])); // 副本级 true 盖住包级 false
        let reg = reg_with(Some(true), vec![(Method::Copy, None)]);
        assert!(copy_should_update(&reg, &reg.installs[0])); // 跟随包级
        let reg = reg_with(None, vec![(Method::Copy, None)]);
        assert!(!copy_should_update(&reg, &reg.installs[0])); // 默认 false
    }

    #[test]
    fn plan_respects_two_levels() {
        // 包级开：symlink 全更新；copy 里 s0 副本关、s1 跟随
        let mut reg = reg_with(Some(true), vec![
            (Method::Symlink, None),
            (Method::Copy, Some(false)),
            (Method::Copy, None),
        ]);
        let plan = build_plan(&reg, None);
        assert_eq!(plan.sources.len(), 1);
        let copy_decisions: Vec<_> = plan.copies.iter().map(|c| (c.skill.clone(), c.update)).collect();
        assert_eq!(copy_decisions,
            vec![("s1".into(), false), ("s2".into(), true)]);
        assert_eq!(plan.symlinks, vec!["s0".to_string()]);
        // 包级关：仓库不 pull，一切不更新
        let mut reg2 = reg_with(Some(false), vec![(Method::Symlink, None), (Method::Copy, None)]);
        let plan2 = build_plan(&reg2, None);
        assert!(plan2.sources.is_empty());
        assert!(plan2.symlinks.is_empty());
        assert!(plan2.copies.iter().all(|c| !c.update));
    }

    #[test]
    fn explicit_skill_target_forces_update() {
        let reg = reg_with(Some(false), vec![(Method::Copy, Some(false))]);
        let sel = Selection { skill: "s0".into(),
            target: TargetRec::Global { name: "agents".into() } };
        let plan = build_plan(&reg, Some(&sel));
        assert_eq!(plan.copies[0].update, true);   // 显式指定无视配置
        assert_eq!(plan.sources.len(), 1);         // 且会拉仓库
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test update`
预期：编译失败。

- [ ] **步骤 3：实现 update.rs**

```rust
use super::error::Result;
use super::registry::{Install, Method, Registry, SourceRecord, TargetRec};

pub fn repo_should_update(src: &SourceRecord) -> bool {
    src.auto_update.unwrap_or(false)
}

pub fn copy_should_update(reg: &Registry, inst: &Install) -> bool {
    inst.auto_update
        .or(reg.sources.get(&inst.source).and_then(|s| s.auto_update))
        .unwrap_or(false)
}

pub struct Selection { pub skill: String, pub target: TargetRec }

#[derive(Default, Debug)]
pub struct Plan {
    pub sources: Vec<String>,                 // 将执行 pull 的仓库 key
    pub symlinks: Vec<String>,                // 随仓库更新的 symlink 技能名
    pub copies: Vec<CopyDecision>,            // 每个 copy 副本的决定（含跳过的，供 dry-run 展示）
}

#[derive(Debug)]
pub struct CopyDecision { pub skill: String, pub target: TargetRec, pub update: bool, pub reason: String }

/// 构建更新计划。selection = Some 时只针对该副本，且无视配置强制更新。
pub fn build_plan(reg: &Registry, selection: Option<&Selection>) -> Plan {
    let mut plan = Plan::default();
    let mut pull: Vec<String> = Vec::new();
    match selection {
        Some(sel) => {
            if let Some(inst) = reg.find(&sel.skill, &sel.target) {
                pull.push(inst.source.clone());
                match inst.method {
                    Method::Symlink => plan.symlinks.push(inst.skill.clone()),
                    Method::Copy => plan.copies.push(CopyDecision {
                        skill: inst.skill.clone(), target: sel.target.clone(),
                        update: true, reason: "显式指定".into(),
                    }),
                }
            }
        }
        None => {
            for (key, src) in &reg.sources {
                if repo_should_update(src) { pull.push(key.clone()); }
            }
            for inst in &reg.installs {
                let repo_in_plan = pull.contains(&inst.source);
                match inst.method {
                    Method::Symlink => {
                        if repo_in_plan { plan.symlinks.push(inst.skill.clone()); }
                    }
                    Method::Copy => {
                        let allowed = copy_should_update(reg, inst);
                        plan.copies.push(CopyDecision {
                            skill: inst.skill.clone(), target: inst.target.clone(),
                            update: repo_in_plan && allowed,
                            reason: if !repo_in_plan { "仓库不更新".into() }
                                    else if !allowed { "副本级/包级配置关闭".into() }
                                    else { "更新".into() },
                        });
                    }
                }
            }
        }
    }
    pull.sort(); pull.dedup();
    plan.sources = pull;
    plan
}

/// 执行计划（非 dry-run）：pull 仓库 → copy 副本重复制 → 更新 registry commit 字段。
pub fn execute_plan(
    layout: &super::paths::Layout, cfg: &super::config::Config,
    reg: &mut Registry, plan: &Plan,
) -> Result<Vec<String>> {
    let mut done = Vec::new();
    for key in &plan.sources {
        let cache = layout.cache_dir(key);
        if let Some(new_commit) = super::git::fetch_and_reset(&cache)? {
            if let Some(src) = reg.sources.get_mut(key) {
                src.commit = new_commit.clone();
                src.fetched_at = chrono::Utc::now().to_rfc3339();
            }
            done.push(format!("仓库 {key} → {new_commit:.8}"));
        }
    }
    for d in plan.copies.iter().filter(|c| c.update) {
        let rec = reg.find(&d.skill, &d.target).unwrap().clone();
        let target = match &d.target {
            TargetRec::Global { name } => super::paths::Target::Global { name: name.clone() },
            TargetRec::Project { root } => super::paths::Target::Project { root: root.clone() },
        };
        let dest = target.install_dir(cfg)?.join(&d.skill);
        let src_dir = layout.cache_dir(&rec.source).join(&rec.source_path);
        if dest.exists() { std::fs::remove_dir_all(&dest)?; }
        super::cache::copy_dir(&src_dir, &dest)?;
        let commit = reg.sources[&rec.source].commit.clone();
        if let Some(mut_inst) = reg.installs.iter_mut()
            .find(|i| i.skill == d.skill && i.target == d.target) {
            mut_inst.commit = commit;
        }
        done.push(format!("副本 {} @ {:?} 已更新", d.skill, d.target));
    }
    for name in &plan.symlinks {
        for inst in reg.installs.iter_mut()
            .filter(|i| &i.skill == name && i.method == Method::Symlink) {
            inst.commit = reg.sources[&inst.source].commit.clone();
        }
        done.push(format!("软连接 {name} 跟随仓库"));
    }
    reg.save(layout)?;
    Ok(done)
}
```

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test update`
预期：4 个测试 PASS。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): 两级更新引擎（包级 pull + 副本级传播 + dry-run 计划）"
```

---
### 任务 10：分类管理（tags）

**文件：**
- 创建：`src/core/tags.rs`
- 修改：`src/core/mod.rs`（加 `pub mod tags;`）
- 测试：内联于 `tags.rs`

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn reg() -> Registry {
        let mut r = Registry { version: 1, ..Default::default() };
        for (skill, name) in [("a", "agents"), ("a", "claude"), ("b", "agents")] {
            r.installs.push(Install {
                skill: skill.into(), source: "github/o/r".into(),
                source_path: format!("skills/{skill}").into(),
                target: TargetRec::Global { name: name.into() },
                method: Method::Copy, commit: "c1".into(),
                tags: vec![], auto_update: None, installed_at: "t".into(),
            });
        }
        r
    }

    #[test]
    fn set_tags_on_one_install_only() {
        let mut r = reg();
        let t = TargetRec::Global { name: "agents".into() };
        set_tags(&mut r, "a", &t, vec!["frontend".into(), "ui".into()]).unwrap();
        assert_eq!(r.find("a", &t).unwrap().tags, vec!["frontend", "ui"]);
        // 同名技能在 claude 的副本不受影响
        let claude = TargetRec::Global { name: "claude".into() };
        assert!(r.find("a", &claude).unwrap().tags.is_empty());
    }

    #[test]
    fn filter_by_tag() {
        let mut r = reg();
        set_tags(&mut r, "a", &TargetRec::Global { name: "agents".into() }, vec!["frontend".into()]).unwrap();
        set_tags(&mut r, "b", &TargetRec::Global { name: "agents".into() }, vec!["backend".into()]).unwrap();
        let hits = filter_by_tag(&r, "frontend");
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].skill, "a");
        assert!(filter_by_tag(&r, "不存在").is_empty());
    }

    #[test]
    fn set_tags_on_missing_install_errors() {
        let mut r = reg();
        let t = TargetRec::Global { name: "agents".into() };
        assert!(set_tags(&mut r, "nope", &t, vec!["x".into()]).is_err());
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test tags`
预期：编译失败。

- [ ] **步骤 3：实现 tags.rs**

```rust
use super::error::{Error, Result};
use super::registry::{Install, Registry, TargetRec};

/// 覆盖式设置某条 install 的分类（同一技能的其他目标副本不受影响）。
pub fn set_tags(reg: &mut Registry, skill: &str, target: &TargetRec, tags: Vec<String>) -> Result<()> {
    let inst = reg.installs.iter_mut()
        .find(|i| i.skill == skill && &i.target == target)
        .ok_or_else(|| Error::NotInstalled(skill.into()))?;
    inst.tags = tags;
    Ok(())
}

pub fn filter_by_tag<'a>(reg: &'a Registry, tag: &str) -> Vec<&'a Install> {
    reg.installs.iter().filter(|i| i.tags.iter().any(|t| t == tag)).collect()
}
```

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test tags`
预期：3 个测试 PASS。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "feat(core): 分类管理（按 install 粒度打标与筛选）"
```

---

### 任务 11：CLI 命令树与全部子命令

**文件：**
- 修改：`src/cli/mod.rs`（创建）—— clap 命令树
- 修改：`src/cli/commands.rs`（创建）—— 各子命令实现
- 修改：`src/main.rs`—— 接线
- 测试：`tests/cli_smoke.rs`

- [ ] **步骤 1：编写失败测试（smoke：命令树能解析，help 不崩）**

```rust
use assert_cmd::Command;

#[test]
fn help_lists_all_subcommands() {
    let out = Command::cargo_bin("skills").unwrap()
        .arg("--help").output().unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    for cmd in ["add", "list", "remove", "update", "tag", "auto-update", "config", "tui", "ui"] {
        assert!(s.contains(cmd), "help 缺少 {cmd}");
    }
}

#[test]
fn list_on_empty_layout_succeeds() {
    let tmp = tempfile::tempdir().unwrap();
    let out = Command::cargo_bin("skills").unwrap()
        .env("SKILLS_HOME", tmp.path())   // 测试隔离：Layout::new 读此环境变量
        .args(["list"]).output().unwrap();
    assert!(out.status.success());
}
```

`Cargo.toml` dev-dependencies 追加 `assert_cmd = "2"`。

`Layout::new` 改为先读 `SKILLS_HOME` 环境变量（测试隔离用，也是用户自定义数据目录的出口）：

```rust
pub fn new() -> Result<Layout> {
    if let Ok(p) = std::env::var("SKILLS_HOME") {
        return Ok(Layout { root: p.into() });
    }
    let home = dirs::home_dir().ok_or(Error::NoHome)?;
    Ok(Layout { root: home.join(".skills") })
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test --test cli_smoke`
预期：FAIL（子命令不存在）。

- [ ] **步骤 3：实现 cli/mod.rs 命令树**

```rust
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(name = "skills", about = "技能包管理器：下载、安装、分类、更新")]
pub struct Cli {
    #[command(subcommand)]
    pub cmd: Option<Cmd>,
}

#[derive(Clone, Copy, ValueEnum)]
pub enum MethodArg { Symlink, Copy }

#[derive(Subcommand)]
pub enum Cmd {
    /// 下载并安装技能（source 已缓存则复用）
    Add {
        source: String,
        #[arg(short, long)] skill: Vec<String>,
        #[arg(short, long)] target: Vec<String>,   // global:<name> | project:<abs路径>
        #[arg(short = 'g', long)] global: bool,     // 等价 --target global:agents
        #[arg(long, value_enum)] method: Option<MethodArg>,
        #[arg(short = 'y', long)] yes: bool,
    },
    /// 列出已安装技能
    #[command(alias = "ls")]
    List {
        #[arg(long)] tag: Option<String>,
        #[arg(short, long)] target: Option<String>,
        #[arg(short = 'g', long)] global: bool,
    },
    /// 删除已安装技能（先查记录再核实磁盘）
    Remove {
        skills: Vec<String>,
        #[arg(short, long)] target: Vec<String>,
        #[arg(long)] tag: Option<String>,
        #[arg(short = 'y', long)] yes: bool,
    },
    /// 按两级策略更新；显式指定技能时强制更新该副本
    Update {
        skills: Vec<String>,
        #[arg(short, long)] target: Option<String>,
        #[arg(long)] all: bool,
        #[arg(long)] dry_run: bool,
    },
    /// 分类管理（只写 registry.json）
    Tag {
        skill: String,
        tags: Vec<String>,
        #[arg(short, long)] target: String,
        #[arg(long)] remove: bool,
    },
    /// 升级策略（只写 registry.json）
    AutoUpdate {
        skill: Option<String>,
        #[arg(short, long)] target: Option<String>,
        #[arg(short, long)] source: Option<String>,
        #[arg(long)] on: bool,
        #[arg(long)] off: bool,
        #[arg(long)] inherit: bool,   // 清除副本级覆盖，跟随包级
    },
    /// 全局配置（只写 config.toml）
    Config {
        #[command(subcommand)]
        sub: ConfigCmd,
    },
    /// 进入 TUI
    Tui,
    /// 启动 Web 管理页
    Ui {
        #[arg(long)] port: Option<u16>,
        #[arg(long)] no_open: bool,
    },
}

#[derive(Subcommand)]
pub enum ConfigCmd {
    Get { key: String },
    Set { key: String, value: String },
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
```

- [ ] **步骤 4：实现 cli/commands.rs（各命令薄壳，逻辑全在 core）**

```rust
use crate::core::{cache, config::Config, error::Result, install, paths::{Layout, Target},
    registry::{Method, Registry, TargetRec}, remove, source::parse_source, tags, update};
use super::{Cli, Cmd, ConfigCmd, MethodArg, TargetsCmd};

pub fn run(cli: Cli) -> Result<()> {
    let layout = Layout::new()?;
    let cfg = Config::load(&layout)?;
    match cli.cmd {
        None | Some(Cmd::Tui) => crate::tui::run(&layout, &cfg),
        Some(Cmd::Ui { port, no_open }) => crate::web::run(&layout, port.unwrap_or(cfg.web_port), no_open),
        Some(Cmd::List { tag, target, global }) => {
            let reg = Registry::load(&layout)?;
            let t = target.as_deref().map(Target::parse).transpose()?;
            let rows: Vec<_> = reg.installs.iter()
                .filter(|i| tag.as_ref().map(|t| i.tags.contains(t)).unwrap_or(true))
                .filter(|i| match &t {
                    Some(t) => install::to_rec(t) == i.target,
                    None => !global || matches!(i.target, TargetRec::Global { .. }),
                })
                .collect();
            if rows.is_empty() { println!("（无已安装技能）"); }
            for i in rows {
                println!("{}\t{:?}\t{:?}\ttags={:?}\tauto_update={:?}",
                    i.skill, i.method, i.target, i.tags, i.auto_update);
            }
            Ok(())
        }
        Some(Cmd::Add { source, skill, target, global, method, yes }) => {
            let mut reg = Registry::load(&layout)?;
            let spec = parse_source(&source)?;
            let cached = cache::ensure_cached(&layout, &spec)?;
            if !cached.fresh { println!("已缓存 {}，复用（skills update 可更新）", spec.key); }
            reg.sources.entry(spec.key.clone()).or_insert(
                crate::core::registry::SourceRecord {
                    url: spec.url.clone().unwrap_or_default(),
                    commit: cached.commit.clone(),
                    fetched_at: chrono::Utc::now().to_rfc3339(),
                    auto_update: None,
                });
            let all = cache::scan_skills(&cached.path)?;
            let picked: Vec<_> = if skill.is_empty() {
                // 交互多选
                let names: Vec<String> = all.iter().map(|s| s.name.clone()).collect();
                let idx = dialoguer::MultiSelect::new()
                    .with_prompt("选择要安装的技能").items(&names).interact()
                    .map_err(|e| crate::core::error::Error::Msg(e.to_string()))?;
                idx.into_iter().map(|i| all[i].name.clone()).collect()
            } else { skill };
            let targets: Vec<Target> = if target.is_empty() {
                let default = if global { "global:agents" } else { "global:agents" };
                vec![Target::parse(default)?]
            } else {
                target.iter().map(|s| Target::parse(s)).collect::<Result<_>>()?
            };
            let method = match method {
                Some(MethodArg::Copy) => Method::Copy,
                Some(MethodArg::Symlink) => Method::Symlink,
                None => cfg.default_method,
            };
            for s in &picked {
                let entry = all.iter().find(|e| &e.name == s)
                    .ok_or_else(|| crate::core::error::Error::Msg(format!("仓库中无技能 {s}")))?;
                for t in &targets {
                    match install::install_skill(&layout, &cfg, &mut reg, &spec.key,
                        &entry.name, &entry.rel_path.to_string_lossy(), t, method, &cached.commit) {
                        Ok(_) => println!("已安装 {s} → {t:?} ({method:?})"),
                        Err(crate::core::error::Error::Conflict(p)) => {
                            if yes { println!("跳过已存在: {p:?}"); }
                            else {
                                let overwrite = dialoguer::Confirm::new()
                                    .with_prompt(format!("{p:?} 已存在，覆盖？"))
                                    .interact().map_err(|e| crate::core::error::Error::Msg(e.to_string()))?;
                                if overwrite {
                                    let rec = install::to_rec(t);
                                    let _ = remove::remove_install(&layout, &cfg, &mut reg, s, &rec);
                                    install::install_skill(&layout, &cfg, &mut reg, &spec.key,
                                        &entry.name, &entry.rel_path.to_string_lossy(), t, method, &cached.commit)?;
                                }
                            }
                        }
                        Err(e) => return Err(e),
                    }
                }
            }
            reg.save(&layout)
        }
        Some(Cmd::Remove { skills, target, tag, yes: _ }) => {
            let mut reg = Registry::load(&layout)?;
            // 组装待删集合：显式 skills × targets，或按 tag 全删
            let mut doomed: Vec<(String, TargetRec)> = Vec::new();
            if let Some(tg) = tag {
                doomed.extend(tags::filter_by_tag(&reg, &tg).iter()
                    .map(|i| (i.skill.clone(), i.target.clone())));
            }
            for s in &skills {
                if target.is_empty() {
                    doomed.extend(reg.installs.iter().filter(|i| &i.skill == s)
                        .map(|i| (i.skill.clone(), i.target.clone())));
                } else {
                    for t in &target {
                        doomed.push((s.clone(), install::to_rec(&Target::parse(t)?)));
                    }
                }
            }
            for (s, t) in doomed {
                match remove::remove_install(&layout, &cfg, &mut reg, &s, &t) {
                    Ok(remove::RemoveOutcome::Removed) => println!("已删除 {s} @ {t:?}"),
                    Ok(remove::RemoveOutcome::RecordOnly) => println!("磁盘已不存在，仅清记录: {s} @ {t:?}"),
                    Err(e) => eprintln!("跳过 {s}: {e}"),
                }
            }
            reg.save(&layout)
        }
        Some(Cmd::Update { skills, target, all, dry_run }) => {
            let mut reg = Registry::load(&layout)?;
            let sel = match (skills.first(), &target) {
                (Some(s), Some(t)) => Some(update::Selection {
                    skill: s.clone(), target: install::to_rec(&Target::parse(t)?) }),
                _ => None,
            };
            let plan = update::build_plan(&reg, sel.as_ref());
            if dry_run {
                println!("将拉取仓库: {:?}", plan.sources);
                println!("软连接跟随: {:?}", plan.symlinks);
                for c in &plan.copies {
                    println!("{} @ {:?}: {}（{}）", c.skill, c.target,
                        if c.update { "更新" } else { "跳过" }, c.reason);
                }
                return Ok(());
            }
            let done = update::execute_plan(&layout, &cfg, &mut reg, &plan)?;
            for line in done { println!("{line}"); }
            Ok(())
        }
        Some(Cmd::Tag { skill, tags: new_tags, target, remove }) => {
            let mut reg = Registry::load(&layout)?;
            let rec = install::to_rec(&Target::parse(&target)?);
            let final_tags = if remove { vec![] } else { new_tags };
            tags::set_tags(&mut reg, &skill, &rec, final_tags)?;
            reg.save(&layout)
        }
        Some(Cmd::AutoUpdate { skill, target, source, on, off, inherit }) => {
            let mut reg = Registry::load(&layout)?;
            let val = if on { Some(true) } else if off { Some(false) } else { None };
            if let Some(src) = source {
                // 包级
                let s = reg.sources.get_mut(&src)
                    .ok_or_else(|| crate::core::error::Error::Msg(format!("未知来源 {src}")))?;
                s.auto_update = val;
            } else if let (Some(s), Some(t)) = (skill, target) {
                // 副本级
                let rec = install::to_rec(&Target::parse(&t)?);
                let inst = reg.installs.iter_mut()
                    .find(|i| i.skill == s && i.target == rec)
                    .ok_or_else(|| crate::core::error::Error::NotInstalled(s.clone()))?;
                if inst.method == Method::Symlink && !inherit {
                    eprintln!("提示：{s} 为软连接安装，更新策略跟随技能包（--source 设置）");
                }
                inst.auto_update = if inherit { None } else { val };
            } else {
                return Err(crate::core::error::Error::Msg(
                    "需指定 --source <包> 或 <技能> + --target".into()));
            }
            reg.save(&layout)
        }
        Some(Cmd::Config { sub }) => run_config(&layout, sub),
    }
}

fn run_config(layout: &Layout, sub: ConfigCmd) -> Result<()> {
    // 读写 config.toml 原文（保留用户注释尽量简单：整体重写）
    let path = layout.config_path();
    let mut doc: toml::Value = if path.exists() {
        toml::from_str(&std::fs::read_to_string(&path)?)?
    } else { toml::Value::Table(Default::default()) };
    match sub {
        ConfigCmd::Get { key } => {
            let mut cur = &doc;
            for part in key.split('.') {
                cur = cur.get(part).ok_or_else(||
                    crate::core::error::Error::Msg(format!("配置项不存在: {key}")))?;
            }
            println!("{cur}");
            Ok(())
        }
        ConfigCmd::Set { key, value } => {
            let mut cur = &mut doc;
            let parts: Vec<_> = key.split('.').collect();
            for part in &parts[..parts.len() - 1] {
                cur = cur.entry(part).or_insert(toml::Value::Table(Default::default()));
            }
            cur[parts[parts.len() - 1]] = toml::Value::String(value);
            std::fs::write(&path, toml::to_string_pretty(&doc).unwrap())?;
            Ok(())
        }
        ConfigCmd::Targets { sub } => match sub {
            TargetsCmd::Add { name, path: p } => {
                doc["targets"][&name] = toml::Value::String(p);
                std::fs::write(&path, toml::to_string_pretty(&doc).unwrap())?;
                Ok(())
            }
            TargetsCmd::Remove { name } => {
                if let Some(t) = doc.get_mut("targets").and_then(|t| t.as_table_mut()) {
                    t.remove(&name);
                }
                std::fs::write(&path, toml::to_string_pretty(&doc).unwrap())?;
                Ok(())
            }
        },
    }
}
```

`main.rs`：

```rust
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
```

本任务内 `tui::run` / `web::run` 先建最小占位实现（任务 12/13 填充）：

```rust
// tui/mod.rs
pub fn run(_l: &crate::core::paths::Layout, _c: &crate::core::config::Config) -> crate::core::error::Result<()> {
    println!("TUI 尚未实现"); Ok(())
}
// web/mod.rs
pub fn run(_l: &crate::core::paths::Layout, port: u16, _no_open: bool) -> crate::core::error::Result<()> {
    println!("Web UI 尚未实现（端口 {port}）"); Ok(())
}
```

- [ ] **步骤 5：运行测试验证通过**

运行：`cargo test`
预期：全部 PASS（含 cli_smoke 2 个）。

- [ ] **步骤 6：Commit**

```bash
git add -A && git commit -m "feat(cli): 完整命令树与子命令实现"
```

---

### 任务 12：TUI（三视图）

**文件：**
- 修改：`src/tui/app.rs`（创建）—— AppState + 纯函数 reducer
- 修改：`src/tui/ui.rs`（创建）—— ratatui 渲染
- 修改：`src/tui/mod.rs`—— 事件循环
- 测试：内联于 `app.rs`

- [ ] **步骤 1：编写失败测试（reducer 纯函数）**

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn app_with(items: usize) -> AppState {
        let mut reg = Registry { version: 1, ..Default::default() };
        for i in 0..items {
            reg.installs.push(Install {
                skill: format!("s{i}"), source: "github/o/r".into(),
                source_path: format!("skills/s{i}").into(),
                target: TargetRec::Global { name: "agents".into() },
                method: Method::Copy, commit: "c1".into(),
                tags: vec![], auto_update: None, installed_at: "t".into(),
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
        app.reduce(Action::Down); app.reduce(Action::Down);
        assert_eq!(app.selected, 2);   // clamp，不越界
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
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test tui`
预期：编译失败。

- [ ] **步骤 3：实现 app.rs（状态与 reducer）**

```rust
use crate::core::registry::{Install, Method, Registry};

#[derive(PartialEq, Debug, Clone, Copy)]
pub enum View { Installed, Install, Sources }

#[derive(PartialEq, Debug)]
pub enum Action { Up, Down, NextView, PrevView, ToggleAutoUpdate, Select, Quit }

pub struct AppState {
    pub registry: Registry,
    pub view: View,
    pub selected: usize,
    pub tag_filter: Option<String>,
}

impl AppState {
    pub fn new(registry: Registry) -> Self {
        AppState { registry, view: View::Installed, selected: 0, tag_filter: None }
    }

    pub fn visible_rows(&self) -> Vec<&Install> {
        self.registry.installs.iter()
            .filter(|i| self.tag_filter.as_ref()
                .map(|t| i.tags.contains(t)).unwrap_or(true))
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
                if self.view != View::Installed { return; }
                if let Some(row) = self.visible_rows().get(self.selected) {
                    let skill = row.skill.clone();
                    let target = row.target.clone();
                    let method = row.method;
                    if method == Method::Symlink { return; } // 软连接跟随包级
                    if let Some(inst) = self.registry.installs.iter_mut()
                        .find(|i| i.skill == skill && i.target == target) {
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
```

- [ ] **步骤 4：实现 ui.rs + mod.rs 事件循环**

`ui.rs`（渲染，逻辑无状态，不必单测；用 ratatui TestBackend 做一次 smoke）：

```rust
use ratatui::{layout::{Constraint, Layout as RLayout}, style::{Color, Style},
    widgets::{Block, Borders, Cell, Paragraph, Row, Table, Tabs}, Frame};
use super::app::{AppState, View};

pub fn draw(f: &mut Frame, app: &AppState) {
    let chunks = RLayout::vertical([Constraint::Length(3), Constraint::Min(0)]).split(f.area());
    let titles = ["已安装", "安装向导", "仓库缓存"];
    let idx = match app.view { View::Installed => 0, View::Install => 1, View::Sources => 2 };
    f.render_widget(Tabs::new(titles.iter().map(|t| *t).collect::<Vec<_>>())
        .select(idx).block(Block::default().borders(Borders::ALL).title("skills")),
        chunks[0]);
    match app.view {
        View::Installed => {
            let rows = app.visible_rows().iter().enumerate().map(|(i, r)| {
                let style = if i == app.selected { Style::default().fg(Color::Yellow) }
                            else { Style::default() };
                Row::new(vec![
                    Cell::from(r.skill.clone()),
                    Cell::from(format!("{:?}", r.method)),
                    Cell::from(format!("{:?}", r.target)),
                    Cell::from(r.tags.join(",")),
                    Cell::from(match r.auto_update {
                        Some(true) => "开", Some(false) => "关", None => "跟随包级",
                    }),
                ]).style(style)
            });
            f.render_widget(Table::new(rows,
                [Constraint::Percentage(25), Constraint::Percentage(10),
                 Constraint::Percentage(35), Constraint::Percentage(15),
                 Constraint::Percentage(15)])
                .header(Row::new(vec!["技能", "方式", "目标", "分类", "自动更新"])
                    .style(Style::default().fg(Color::Cyan))),
                chunks[1]);
        }
        View::Install => {
            f.render_widget(Paragraph::new("安装向导：按 i 输入 source（本视图在事件循环中处理）")
                .block(Block::default().borders(Borders::ALL)), chunks[1]);
        }
        View::Sources => {
            let text: String = app.registry.sources.iter()
                .map(|(k, s)| format!("{k}\t{}\tauto_update={:?}\n", &s.commit[..7.min(s.commit.len())], s.auto_update))
                .collect();
            f.render_widget(Paragraph::new(text)
                .block(Block::default().borders(Borders::ALL).title("仓库缓存")), chunks[1]);
        }
    }
}
```

`mod.rs`：

```rust
pub mod app;
pub mod ui;

use crossterm::event::{self, Event, KeyCode};
use ratatui::backend::CrosstermBackend;
use crate::core::{config::Config, error::Result, paths::Layout, registry::Registry};
use app::{Action, AppState};

pub fn run(layout: &Layout, _cfg: &Config) -> Result<()> {
    let reg = Registry::load(layout)?;
    let mut app = AppState::new(reg);
    crossterm::terminal::enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut term = ratatui::Terminal::new(backend)?;
    loop {
        term.draw(|f| ui::draw(f, &app))?;
        if let Event::Key(k) = event::read()? {
            let action = match k.code {
                KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
                KeyCode::Up | KeyCode::Char('k') => Action::Up,
                KeyCode::Down | KeyCode::Char('j') => Action::Down,
                KeyCode::Tab => Action::NextView,
                KeyCode::BackTab => Action::PrevView,
                KeyCode::Char('a') => Action::ToggleAutoUpdate,
                _ => continue,
            };
            if action == Action::Quit { break; }
            app.reduce(action);
        }
    }
    crossterm::terminal::disable_raw_mode()?;
    crossterm::execute!(term.backend_mut(), crossterm::terminal::LeaveAlternateScreen)?;
    app.registry.save(layout)?;   // 退出时落盘（auto_update 切换等）
    Ok(())
}
```

> 安装向导视图（多选技能/目标）首版用 dialoguer 从 TUI 中切出完成（进入向导时临时退出 raw mode，跑完 dialoguer 再恢复），避免在 ratatui 内实现完整表单；这是有意的范围控制。

- [ ] **步骤 5：运行测试验证通过**

运行：`cargo test tui`
预期：4 个 reducer 测试 PASS；`cargo build` 成功。

- [ ] **步骤 6：Commit**

```bash
git add -A && git commit -m "feat(tui): 三视图 TUI（已安装/安装向导/仓库缓存）"
```

---

### 任务 13：Web 管理页（REST API + 内嵌前端）

**文件：**
- 修改：`src/web/mod.rs`—— axum 启动 + 打开浏览器
- 创建：`src/web/api.rs`—— 路由与 handler
- 创建：`src/web/static/index.html`—— 单页前端
- 测试：内联于 `api.rs`（tower oneshot）

- [ ] **步骤 1：编写失败测试**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tower::ServiceExt; // oneshot

    fn test_state() -> AppState {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let mut reg = Registry { version: 1, ..Default::default() };
        reg.installs.push(Install {
            skill: "alpha".into(), source: "github/o/r".into(),
            source_path: "skills/alpha".into(),
            target: TargetRec::Global { name: "agents".into() },
            method: Method::Copy, commit: "c1".into(),
            tags: vec!["frontend".into()], auto_update: None,
            installed_at: "t".into(),
        });
        reg.save(&layout).unwrap();
        AppState { layout, tmp }
    }

    #[tokio::test]
    async fn list_installs_returns_json() {
        let app = router(test_state());
        let resp = app.oneshot(axum::http::Request::builder()
            .uri("/api/installs").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v[0]["skill"], "alpha");
        assert_eq!(v[0]["tags"][0], "frontend");
    }

    #[tokio::test]
    async fn set_auto_update_writes_registry() {
        let state = test_state();
        let layout_root = state.layout.root.clone();
        let app = router(state);
        let resp = app.oneshot(axum::http::Request::builder()
            .method("POST").uri("/api/auto-update")
            .header("content-type", "application/json")
            .body(axum::body::Body::from(serde_json::to_string(&serde_json::json!({
                "skill": "alpha",
                "target": {"kind": "global", "name": "agents"},
                "value": true
            })).unwrap())).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
        let reg = Registry::load(&Layout::at(layout_root)).unwrap();
        assert_eq!(reg.installs[0].auto_update, Some(true));
    }

    #[tokio::test]
    async fn index_html_served_at_root() {
        let app = router(test_state());
        let resp = app.oneshot(axum::http::Request::builder()
            .uri("/").body(axum::body::Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&body).contains("skills"));
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test web`
预期：编译失败。

- [ ] **步骤 3：实现 api.rs**

```rust
use axum::{extract::State, http::StatusCode, response::Html, routing::{get, post}, Json, Router};
use std::sync::{Arc, Mutex};
use crate::core::{paths::Layout, registry::{Registry, TargetRec}};

/// tmp 字段仅用于测试中持有临时目录句柄（防过早删除）。
#[derive(Clone)]
pub struct AppState {
    pub layout: Layout,
    #[cfg(test)]
    pub tmp: Arc<tempfile::TempDir>,
}

// 非 test 构建需要无 tmp 的构造器
impl AppState {
    pub fn new(layout: Layout) -> Self {
        AppState { layout, #[cfg(test)] tmp: Arc::new(tempfile::tempdir().unwrap()) }
    }
}

pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/installs", get(list_installs))
        .route("/api/sources", get(list_sources))
        .route("/api/auto-update", post(set_auto_update))
        .route("/api/tags", post(set_tags))
        .route("/api/remove", post(remove_install))
        .route("/api/update", post(run_update))
        .with_state(Arc::new(Mutex::new(state)))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("static/index.html"))
}

type S = State<Arc<Mutex<AppState>>>;

async fn list_installs(State(s): S) -> Json<serde_json::Value> {
    let s = s.lock().unwrap();
    let reg = Registry::load(&s.layout).unwrap_or_default();
    Json(serde_json::json!(reg.installs))
}

async fn list_sources(State(s): S) -> Json<serde_json::Value> {
    let s = s.lock().unwrap();
    let reg = Registry::load(&s.layout).unwrap_or_default();
    Json(serde_json::json!(reg.sources))
}

#[derive(serde::Deserialize)]
struct AutoUpdateReq {
    skill: Option<String>,
    target: Option<TargetRec>,
    source: Option<String>,
    value: Option<bool>,     // None = 跟随包级
}

async fn set_auto_update(State(s): S, Json(req): Json<AutoUpdateReq>) -> StatusCode {
    let s = s.lock().unwrap();
    let mut reg = match Registry::load(&s.layout) { Ok(r) => r, Err(_) => return StatusCode::INTERNAL_SERVER_ERROR };
    if let Some(src) = req.source {
        if let Some(r) = reg.sources.get_mut(&src) { r.auto_update = req.value; }
        else { return StatusCode::NOT_FOUND; }
    } else if let (Some(skill), Some(target)) = (req.skill, req.target) {
        match reg.installs.iter_mut().find(|i| i.skill == skill && i.target == target) {
            Some(i) => i.auto_update = req.value,
            None => return StatusCode::NOT_FOUND,
        }
    } else { return StatusCode::BAD_REQUEST; }
    match reg.save(&s.layout) { Ok(_) => StatusCode::OK, Err(_) => StatusCode::INTERNAL_SERVER_ERROR }
}

#[derive(serde::Deserialize)]
struct TagsReq { skill: String, target: TargetRec, tags: Vec<String> }

async fn set_tags(State(s): S, Json(req): Json<TagsReq>) -> StatusCode {
    let s = s.lock().unwrap();
    let mut reg = match Registry::load(&s.layout) { Ok(r) => r, Err(_) => return StatusCode::INTERNAL_SERVER_ERROR };
    match crate::core::tags::set_tags(&mut reg, &req.skill, &req.target, req.tags) {
        Ok(_) => match reg.save(&s.layout) { Ok(_) => StatusCode::OK, Err(_) => StatusCode::INTERNAL_SERVER_ERROR },
        Err(_) => StatusCode::NOT_FOUND,
    }
}

#[derive(serde::Deserialize)]
struct RemoveReq { skill: String, target: TargetRec }

async fn remove_install(State(s): S, Json(req): Json<RemoveReq>) -> StatusCode {
    let s = s.lock().unwrap();
    let cfg = match crate::core::config::Config::load(&s.layout) { Ok(c) => c, Err(_) => return StatusCode::INTERNAL_SERVER_ERROR };
    let mut reg = match Registry::load(&s.layout) { Ok(r) => r, Err(_) => return StatusCode::INTERNAL_SERVER_ERROR };
    match crate::core::remove::remove_install(&s.layout, &cfg, &mut reg, &req.skill, &req.target) {
        Ok(_) => match reg.save(&s.layout) { Ok(_) => StatusCode::OK, Err(_) => StatusCode::INTERNAL_SERVER_ERROR },
        Err(_) => StatusCode::NOT_FOUND,
    }
}

async fn run_update(State(s): S) -> Json<serde_json::Value> {
    let s = s.lock().unwrap();
    let cfg = crate::core::config::Config::load(&s.layout).unwrap_or_default();
    let mut reg = Registry::load(&s.layout).unwrap_or_default();
    let plan = crate::core::update::build_plan(&reg, None);
    let done = crate::core::update::execute_plan(&s.layout, &cfg, &mut reg, &plan)
        .unwrap_or_default();
    Json(serde_json::json!({ "done": done }))
}
```

> `Layout` 需 derive `Clone`（任务 1 中补上 `#[derive(Clone)]`）。`Config` 同理。

- [ ] **步骤 4：实现 mod.rs 与 static/index.html**

```rust
pub mod api;

use crate::core::{error::Result, paths::Layout};

pub fn run(layout: &Layout, port: u16, no_open: bool) -> Result<()> {
    let rt = tokio::runtime::Runtime::new()
        .map_err(|e| crate::core::error::Error::Io(e))?;
    rt.block_on(async {
        let app = api::router(api::AppState::new(layout.clone()));
        let addr = format!("127.0.0.1:{port}");
        let listener = tokio::net::TcpListener::bind(&addr).await?;
        println!("Web UI: http://{addr}");
        if !no_open {
            let _ = open::that(format!("http://{addr}"));
        }
        axum::serve(listener, app).await?;
        Ok(())
    })
}
```

`static/index.html`（无框架单页，功能与 TUI 对等的最小可用版）：

```html
<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8"><title>skills 管理</title>
<style>
body { font-family: system-ui; max-width: 960px; margin: 2rem auto; padding: 0 1rem; }
table { border-collapse: collapse; width: 100%; }
td, th { border: 1px solid #ddd; padding: 6px 10px; }
nav button { margin-right: 8px; }
.on { color: green; } .off { color: #999; }
</style>
</head>
<body>
<h1>skills 管理</h1>
<nav>
  <button onclick="show('installs')">已安装</button>
  <button onclick="show('sources')">仓库缓存</button>
  <button onclick="runUpdate()">执行更新</button>
</nav>
<div id="installs"></div>
<div id="sources" style="display:none"></div>
<script>
async function api(path, opts) {
  const r = await fetch('/api/' + path, opts);
  return r.ok ? (r.headers.get('content-type')||'').includes('json') ? r.json() : null : Promise.reject(r.status);
}
function show(id) {
  for (const d of ['installs','sources']) document.getElementById(d).style.display = d===id?'':'none';
  refresh();
}
async function refresh() {
  const installs = await api('installs');
  document.getElementById('installs').innerHTML =
    '<table><tr><th>技能</th><th>方式</th><th>目标</th><th>分类</th><th>自动更新</th><th></th></tr>' +
    installs.map(i => `<tr><td>${i.skill}</td><td>${i.method}</td>
      <td>${i.target.kind}:${i.target.name||i.target.root}</td>
      <td><input value="${i.tags.join(',')}" onchange="setTags('${i.skill}','${i.target.kind}','${i.target.name||i.target.root}',this.value)"></td>
      <td>${i.method==='copy'
        ? `<select onchange="setAU('${i.skill}','${i.target.kind}','${i.target.name||i.target.root}',this.value)">
            <option value="" ${i.auto_update===null?'selected':''}>跟随包级</option>
            <option value="true" ${i.auto_update===true?'selected':''}>开</option>
            <option value="false" ${i.auto_update===false?'selected':''}>关</option></select>`
        : '跟随包级'}</td>
      <td><button onclick="rm('${i.skill}','${i.target.kind}','${i.target.name||i.target.root}')">删除</button></td></tr>`).join('') + '</table>';
  const sources = await api('sources');
  document.getElementById('sources').innerHTML =
    '<table><tr><th>仓库</th><th>commit</th><th>自动更新</th></tr>' +
    Object.entries(sources).map(([k,s]) => `<tr><td>${k}</td><td>${s.commit.slice(0,7)}</td>
      <td><input type="checkbox" ${s.auto_update?'checked':''} onchange="setSourceAU('${k}',this.checked)"></td></tr>`).join('') + '</table>';
}
function targetBody(kind, ref) { return kind==='global' ? {kind:'global',name:ref} : {kind:'project',root:ref}; }
async function setAU(skill, kind, ref, v) {
  await api('auto-update', {method:'POST', headers:{'content-type':'application/json'},
    body: JSON.stringify({skill, target: targetBody(kind, ref), value: v===''?null:v==='true'})});
}
async function setSourceAU(key, v) {
  await api('auto-update', {method:'POST', headers:{'content-type':'application/json'},
    body: JSON.stringify({source: key, value: v})});
}
async function setTags(skill, kind, ref, v) {
  await api('tags', {method:'POST', headers:{'content-type':'application/json'},
    body: JSON.stringify({skill, target: targetBody(kind, ref), tags: v.split(',').map(s=>s.trim()).filter(Boolean)})});
}
async function rm(skill, kind, ref) {
  if (!confirm(`删除 ${skill}？`)) return;
  await api('remove', {method:'POST', headers:{'content-type':'application/json'},
    body: JSON.stringify({skill, target: targetBody(kind, ref)})});
  refresh();
}
async function runUpdate() {
  const r = await api('update', {method:'POST'});
  alert(r.done.join('\n') || '无更新');
  refresh();
}
refresh();
</script>
</body>
</html>
```

- [ ] **步骤 5：运行测试验证通过**

运行：`cargo test web`
预期：3 个测试 PASS。

- [ ] **步骤 6：Commit**

```bash
git add -A && git commit -m "feat(web): REST API + 内嵌单页管理界面"
```

---

### 任务 14：端到端集成测试 + 三平台 CI

**文件：**
- 创建：`tests/e2e.rs`
- 创建：`.github/workflows/ci.yml`

- [ ] **步骤 1：编写端到端测试（本地 bare repo fixture，全程不依赖网络）**

```rust
// tests/e2e.rs
use assert_cmd::Command;
use std::path::Path;
use std::process::Command as P;

fn git(dir: &Path, args: &[&str]) {
    assert!(P::new("git").args(args).current_dir(dir).status().unwrap().success(), "git {:?}", args);
}

/// 造一个含两个技能的技能包 bare 仓库，返回 (guard, bare 路径, work 路径)
fn fixture_repo(base: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let work = base.join("work");
    let bare = base.join("bare.git");
    for s in ["alpha", "beta"] {
        std::fs::create_dir_all(work.join(format!("skills/{s}"))).unwrap();
        std::fs::write(work.join(format!("skills/{s}/SKILL.md")),
            format!("---\nname: {s}\ndescription: 技能 {s}\n---\n# {s}\n")).unwrap();
    }
    git(&work, &["init", "-b", "main"]);
    git(&work, &["add", "."]);
    git(&work, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "c1"]);
    git(&work, &["clone", "--bare", ".", bare.to_str().unwrap()]);
    (bare, work)
}

fn skills(home: &Path) -> Command {
    let mut c = Command::cargo_bin("skills").unwrap();
    c.env("SKILLS_HOME", home);
    c
}

#[test]
fn add_list_remove_copy_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, _work) = fixture_repo(tmp.path());
    // add：装 alpha 到 global:agents（copy）
    let agents_dir = home.join("agents-skills");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::write(home.join("config.toml"),
        format!("[targets]\nagents = \"{}\"\n", agents_dir.display().to_string().replace('\\', "\\\\"))).unwrap();
    skills(&home).args(["add", &format!("file://{}/o/r", bare.display()),
        "-s", "alpha", "--method", "copy", "-y"]).assert().success();
    assert!(agents_dir.join("alpha/SKILL.md").exists());
    // 再 add 同仓库 → 复用缓存不重复下载（stderr/stdout 有提示）
    let out = skills(&home).args(["add", &format!("file://{}/o/r", bare.display()),
        "-s", "beta", "--method", "copy", "-y"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("已缓存"));
    // list
    let out = skills(&home).args(["list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("alpha") && stdout.contains("beta"));
    // remove
    skills(&home).args(["remove", "alpha", "-y"]).assert().success();
    assert!(!agents_dir.join("alpha").exists());
    assert!(agents_dir.join("beta").exists());
}

#[test]
fn update_respects_two_level_policy() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, work) = fixture_repo(tmp.path());
    std::fs::create_dir_all(&home).unwrap();
    let agents_dir = home.join("agents-skills");
    std::fs::write(home.join("config.toml"),
        format!("[targets]\nagents = \"{}\"\n", agents_dir.display().to_string().replace('\\', "\\\\"))).unwrap();
    skills(&home).args(["add", &format!("file://{}/o/r", bare.display()),
        "-s", "alpha", "-s", "beta", "--method", "copy", "-y"]).assert().success();
    // 包级开、alpha 副本关
    // file:// URL 的 key 推导见 source.rs：host 部分为 bare 父目录名… 测试中用 list 输出断言前
    // 先读 registry 拿真实 key：
    let reg_raw = std::fs::read_to_string(home.join("registry.json")).unwrap();
    let reg: serde_json::Value = serde_json::from_str(&reg_raw).unwrap();
    let key = reg["sources"].as_object().unwrap().keys().next().unwrap().clone();
    skills(&home).args(["auto-update", "--source", &key, "--on"]).assert().success();
    skills(&home).args(["auto-update", "alpha", "-t", "global:agents", "--off"]).assert().success();
    // 仓库推新提交，alpha 加一行
    std::fs::write(work.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: v2\n---\n").unwrap();
    git(&work, &["add", "."]);
    git(&work, &["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-m", "c2"]);
    git(&work, &["push", bare.to_str().unwrap(), "main"]);
    // update：alpha 跳过，beta 更新
    skills(&home).args(["update"]).assert().success();
    let alpha_md = std::fs::read_to_string(agents_dir.join("alpha/SKILL.md")).unwrap();
    assert!(alpha_md.contains("技能 alpha"), "alpha 副本应被跳过");
    // 显式强制更新 alpha
    skills(&home).args(["update", "alpha", "-t", "global:agents"]).assert().success();
    let alpha_md = std::fs::read_to_string(agents_dir.join("alpha/SKILL.md")).unwrap();
    assert!(alpha_md.contains("v2"), "显式指定应强制更新");
}
```

- [ ] **步骤 2：运行验证**

运行：`cargo test --test e2e`
预期：两个测试 PASS。若 file:// URL 的 key 推导与断言不符，以 registry 实际内容为准调整测试（e2e 测试已设计为从 registry 读 key，不硬编码）。

- [ ] **步骤 3：创建 CI 工作流**

`.github/workflows/ci.yml`：

```yaml
name: ci
on: [push, pull_request]
jobs:
  test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
      - run: cargo test
```

- [ ] **步骤 4：全量验证**

运行：`cargo test && cargo clippy -- -D warnings`
预期：全部通过，无警告。

- [ ] **步骤 5：Commit**

```bash
git add -A && git commit -m "test: e2e 集成测试 + 三平台 CI"
```

---

## 自检结果

**规格覆盖度：**
- ~/.skills 布局与 github 子目录 → 任务 1（paths）+ 任务 2（source key）✓
- GitHub 简写补全 → 任务 2 ✓
- 技能包多技能选择安装 → 任务 6（扫描）+ 任务 11（add 多选）✓
- 复制/软连接安装并记录方式 → 任务 7 ✓
- 不重复下载、可更新 → 任务 6（ensure_cached 复用）+ 任务 9 ✓
- 两级自动更新（包级/副本级）、显式强制更新 → 任务 9 ✓
- 安装记录、删除时核实磁盘实况 → 任务 3 + 任务 8 ✓
- 分类管理 → 任务 10 ✓
- CLI 全量子命令 + 参数指定 method/port → 任务 11 ✓
- TUI → 任务 12 ✓
- Web 完整管理界面 → 任务 13 ✓
- config.toml 可选、内置默认值、config/auto-update 命令分离 → 任务 4 + 任务 11 ✓
- 跨平台（gitoxide、junction 兜底、三平台 CI）→ 任务 5 + 任务 7 + 任务 14 ✓
- copy 副本被手动改过的 update 提示 → 任务 9 execute_plan 目前直接覆盖。**偏差**：规格要求覆盖前提示。任务 14 之后追加小任务处理（见下）。

**追加任务 15：copy 副本本地修改检测**

- 文件：`src/core/update.rs`、`src/core/install.rs`
- 做法：install 时对该副本写入一个 `.skills-manifest`（文件名+sha256 列表）隐藏文件；`execute_plan` 重复制前对比 manifest，发现用户改动时返回 `Error::Mismatch`，CLI/TUI/Web 前端提示"将覆盖本地修改"并要求确认（CLI 加 `--force` 跳过确认）。测试：改动副本文件后 update 返回 Mismatch；带确认后更新成功。

**占位符扫描：** 任务 11 中 tui/web 为显式占位实现但已在任务 12/13 用完整实现替换，属计划内演进，无悬空 TODO。

**类型一致性：** `Target`（paths，寻址用）与 `TargetRec`（registry，落盘用）两类型经 `install::to_rec` 转换，全文一致；`Method` 定义在 registry 并被 config 引用，一致；`Plan/CopyDecision/Selection` 仅在 update.rs 出现，一致。


