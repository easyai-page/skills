# add 安装向导重设计 实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 把 `add`/`fav install` 的安装交互重设计为「选技能 → 选 agent → 选 scope → 选安装方式」四步向导，内置约 70 个 agent 的注册表，项目级安装按 agent 分目录，registry.json 升级 v3（不兼容旧版），CLI/TUI/Web 三前端共用同一套逻辑。

**架构：** 新增 `src/core/agents.rs` 静态注册表（从 vercel-labs/skills 的 `src/agents.ts` 移植）；`Target` 重构为 `agent × scope`；交互步骤的数据与决策收敛到 crate 根的 `src/wizard.rs`（CLI 和 TUI 都用 dialoguer，共用）；Web 全参数驱动（新增 `/api/agents`）。安装机制（symlink/copy、manifest、原子性）不变。

**技术栈：** Rust、clap、dialoguer 0.11、serde/toml、ratatui（TUI）、axum（Web）。

**规格：** `docs/superpowers/specs/2026-09-05-add-wizard-redesign-design.md`（先读）。

**全局约束（每个任务都要遵守）：**
- 代码注释中文，解释为什么；命名英文；`cargo fmt` 后提交。
- 六条安全不变量（见 AGENTS.md）不得破坏；本计划只改变「目标路径怎么算出来」。
- 每个任务结束跑 `cargo test` 保证全绿再 commit。

---

## 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `src/core/agents.rs` | agent 静态注册表：AgentSpec、全量表、detect/resolve、config 覆盖合并 | 新建 |
| `src/core/paths.rs` | `Target { agent, scope }` / `Scope`、`install_dir`、parse（新旧两种语法） | 重构 |
| `src/core/registry.rs` | `TargetRec` 换 `{ agent, scope }` 格式、version 3、拒绝旧版 | 重构 |
| `src/core/config.rs` | `targets` 只存文件里的覆盖/自定义项；内置默认移入 agents 表 | 重构 |
| `src/core/install.rs` / `update.rs` / `remove.rs` / `favorites.rs` | `to_rec` 等 Target 适配 | 修改 |
| `src/wizard.rs` | 四步交互（dialoguer）：pick_skills / pick_agents / pick_scope / pick_method + 纯决策函数 | 新建 |
| `src/cli/mod.rs` | 新增 `-a/--agent`、`--all`；`-s` 支持 `'*'` | 修改 |
| `src/cli/commands.rs` | add 与 fav install 接四步向导；list/remove/update/auto-update 的 target 展示解析适配 | 重构 |
| `src/tui/mod.rs` | install_wizard / fav_install_wizard 改用 `src/wizard.rs`，删 `pick_targets` | 修改 |
| `src/web/api.rs` | 新增 `GET /api/agents`；InstallReq 改收 `agents + scope` 服务端展开 | 修改 |
| `src/web/static/index.html` | 安装面板加 agent 多选 + scope/method 选择 | 修改 |
| `tests/e2e.rs` / `tests/cli_smoke.rs` | fixture 与断言适配 + 新链路 | 修改 |
| `AGENTS.md` | target 模型与交互流程描述更新 | 修改 |

---

## 任务 1：agent 注册表 `src/core/agents.rs`

**文件：**
- 创建：`src/core/agents.rs`
- 修改：`src/core/mod.rs`（加 `pub mod agents;`，先看现有 mod 列表照格式加）

- [ ] **步骤 1：编写失败的测试**

在 `src/core/agents.rs` 末尾先只放测试模块（实现下一节）：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_nonempty() {
        let mut seen = std::collections::HashSet::new();
        for a in AGENTS {
            assert!(!a.id.is_empty() && !a.name.is_empty());
            assert!(seen.insert(a.id), "重复 id: {}", a.id);
            assert!(!a.project_dir.is_empty());
            assert!(!a.project_dir.starts_with('/'));
        }
    }

    #[test]
    fn universal_flag_matches_project_dir() {
        for a in AGENTS {
            assert_eq!(a.universal, a.project_dir == ".agents/skills", "{}", a.id);
        }
    }

    #[test]
    fn registry_is_complete() {
        // 移植自 vercel-labs/skills src/agents.ts，条目数与其一致（允许 ±2 漂移，移植时按实际校准）
        assert!(AGENTS.len() >= 60, "只有 {} 条，移植不完整", AGENTS.len());
        assert!(AGENTS.iter().any(|a| a.id == "claude-code"));
        assert!(AGENTS.iter().any(|a| a.id == "codex"));
        assert!(AGENTS.iter().any(|a| a.id == "cursor"));
        assert!(AGENTS.iter().any(|a| a.id == "opencode"));
        assert!(AGENTS.iter().any(|a| a.id == "agents")); // 我们自加的 universal 共享目录条目
    }

    #[test]
    fn resolve_builtin_and_override() {
        let a = resolve("claude-code", &Default::default()).unwrap();
        assert_eq!(a.project_dir, ".claude/skills");
        assert!(a.global_dir.is_some());
        assert!(resolve("nonexistent-agent", &Default::default()).is_err());
    }

    #[test]
    fn detect_uses_config_dirs() {
        let tmp = tempfile::tempdir().unwrap();
        let claude = AGENTS.iter().find(|a| a.id == "claude-code").unwrap();
        assert!(!is_installed_at(tmp.path(), claude));
        std::fs::create_dir_all(tmp.path().join(".claude")).unwrap();
        assert!(is_installed_at(tmp.path(), claude));
    }

    #[test]
    fn detect_installed_returns_detected_ids() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".codex")).unwrap();
        let ids = detect_installed_at(tmp.path());
        assert!(ids.contains(&"codex"));
        assert!(!ids.contains(&"claude-code"));
    }
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test agents:: 2>&1 | tail -5`
预期：编译失败，`agents` 模块不存在。

- [ ] **步骤 3：实现 agents.rs**

先取参考数据：`curl -sL https://raw.githubusercontent.com/vercel-labs/skills/main/src/agents.ts -o /tmp/ref-agents.ts`（直连失败按全局约定加 `https_proxy=http://127.0.0.1:7897` 重试）。

结构定义：

```rust
//! agent 注册表：内置表移植自 vercel-labs/skills 的 src/agents.ts（数据同步，非代码依赖）。
//! 「装到哪」的唯一权威来源；config.toml [targets] 只做全局路径覆盖与自定义扩展。

use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct AgentSpec {
    pub id: &'static str,
    pub name: &'static str,
    /// 项目内相对路径（项目级安装 = <项目根>/<project_dir>）
    pub project_dir: &'static str,
    /// 全局路径（含 ~ 前缀，未展开）；None = 该 agent 不支持全局安装
    pub global_dir: Option<&'static str>,
    /// 本机检测用的 home 下配置目录（不含 ~ 前缀，如 ".claude"）
    pub detect_dirs: &'static [&'static str],
    /// project_dir == ".agents/skills" 的一批，共享同一目录
    pub universal: bool,
}

/// 运行形态：内置条目经 config 覆盖后的解析结果
#[derive(Clone, Debug)]
pub struct ResolvedAgent {
    pub id: String,
    pub name: String,
    pub project_dir: Option<PathBuf>, // 自定义 agent（仅 config 定义）没有项目路径
    pub global_dir: Option<PathBuf>,  // 已展开 ~；已被 [targets] 同名覆盖
}
```

移植规则（对 `/tmp/ref-agents.ts` 的 `agents` 记录逐条转换）：
- 键名 → `id`；`displayName` → `name`；`skillsDir` → `project_dir`；`globalSkillsDir` → `global_dir`（`undefined` → `None`）。
- `detectInstalled` 函数体里 `existsSync` 的 home 配置目录 → `detect_dirs`（取目录部分，如 `~/.claude` → `".claude"`；用 env 变量的取默认值目录，如 `CODEX_HOME` → `".codex"`；用 cwd 相对目录的忽略）。
- `universal` = `skillsDir === '.agents/skills'`。
- 表首自加一条：`AgentSpec { id: "agents", name: "Universal (.agents/skills)", project_dir: ".agents/skills", global_dir: Some("~/.agents/skills"), detect_dirs: &[".agents"], universal: true }`。
- `AGENTS: &[AgentSpec]` 按 id 字典序排列（展示稳定）。

函数：

```rust
pub fn all() -> &'static [AgentSpec] { AGENTS }

/// 解析 agent：内置表命中 → 应用 config 覆盖；否则 config 自定义（仅全局）；否则 None。
/// overrides 即 Config::targets（文件里的 [targets] 条目，已展开 ~）。
pub fn resolve(id: &str, overrides: &std::collections::BTreeMap<String, PathBuf>) -> Option<ResolvedAgent>;

pub fn is_installed_at(home: &Path, spec: &AgentSpec) -> bool {
    spec.detect_dirs.iter().any(|d| home.join(d).is_dir())
}

/// 返回检测到的内置 agent id（含 "agents" 若 ~/.agents 存在）
pub fn detect_installed_at(home: &Path) -> Vec<&'static str>;

/// 生产入口：home 取 dirs::home_dir()，取不到返回空
pub fn detect_installed() -> Vec<&'static str>;
```

`resolve` 实现要点：内置命中时 `global_dir = overrides.get(id).cloned().or(内置 global_dir 展开 ~)`；未命中内置但 `overrides` 有该 key → `ResolvedAgent { id, name: id, project_dir: None, global_dir: Some(覆盖值) }`。

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test agents::`
预期：6 个测试全 PASS。

- [ ] **步骤 5：Commit**

```bash
cargo fmt && git add src/core/agents.rs src/core/mod.rs
git commit -m "feat(core): 内置 agent 注册表（移植 vercel-labs/skills agents 表）"
```

---

## 任务 2：Target 模型重构（paths.rs + registry.rs v3）

**文件：**
- 修改：`src/core/paths.rs`（整体重构 Target，文件头部的 Layout 不动）
- 修改：`src/core/registry.rs:8-13`（TargetRec）、`src/core/registry.rs:63-74`（to_target）、`Registry::load`（version 检查）
- 测试：`src/core/paths.rs` tests 模块、`src/core/registry.rs` tests 模块

**本任务只改模型层与两者的单元测试；调用点（install/update/CLI 等）在任务 3 适配，本任务结束时不求全项目编译通过，只要求 `cargo test --lib paths` 与 `cargo test --lib registry` 绿。**

- [ ] **步骤 1：编写失败的测试**

`src/core/paths.rs` tests 替换为：

```rust
#[test]
fn parse_new_syntax() {
    assert_eq!(
        Target::parse("claude-code@global").unwrap(),
        Target { agent: "claude-code".into(), scope: Scope::Global }
    );
    let abs = if cfg!(windows) { "codex@project:C:\\work" } else { "codex@project:/work" };
    assert!(matches!(Target::parse(abs).unwrap().scope, Scope::Project(_)));
}

#[test]
fn parse_legacy_syntax_maps_to_universal() {
    // 旧 -t 写法兼容：project:<abs> = universal agent + 项目根
    assert_eq!(
        Target::parse("global:claude-code").unwrap(),
        Target { agent: "claude-code".into(), scope: Scope::Global }
    );
    let t = Target::parse(if cfg!(windows) { "project:C:\\work" } else { "project:/work" }).unwrap();
    assert_eq!(t.agent, "agents");
    assert!(matches!(t.scope, Scope::Project(_)));
}

#[test]
fn parse_rejects_garbage() {
    assert!(Target::parse("agents").is_err());
    assert!(Target::parse("global:").is_err());
    assert!(Target::parse("@global").is_err());
    assert!(Target::parse("claude-code@project:./rel").is_err());
}

#[test]
fn display_roundtrip() {
    let t = Target { agent: "codex".into(), scope: Scope::Global };
    assert_eq!(Target::parse(&t.to_string()).unwrap(), t);
}

#[test]
fn install_dir_project_per_agent() {
    let cfg = crate::core::config::Config::default();
    let t = Target { agent: "claude-code".into(), scope: Scope::Project(PathBuf::from("/tmp/proj")) };
    assert_eq!(t.install_dir(&cfg).unwrap(), PathBuf::from("/tmp/proj/.claude/skills"));
    let u = Target { agent: "agents".into(), scope: Scope::Project(PathBuf::from("/tmp/proj")) };
    assert_eq!(u.install_dir(&cfg).unwrap(), PathBuf::from("/tmp/proj/.agents/skills"));
}

#[test]
fn install_dir_global_and_errors() {
    let cfg = crate::core::config::Config::default();
    let g = Target { agent: "codex".into(), scope: Scope::Global };
    let dir = g.install_dir(&cfg).unwrap();
    assert!(dir.ends_with(".codex/skills") || dir.ends_with(".codex\\skills"));
    // 不支持全局的 agent（eve 无 global_dir）
    let eve = Target { agent: "eve".into(), scope: Scope::Global };
    assert!(eve.install_dir(&cfg).is_err());
    // 未知 agent
    assert!(Target { agent: "nope".into(), scope: Scope::Global }.install_dir(&cfg).is_err());
}
```

`src/core/registry.rs` tests 新增：

```rust
#[test]
fn target_rec_v3_roundtrip() {
    let rec = TargetRec::Global { agent: "claude-code".into() };
    let v = serde_json::to_value(&rec).unwrap();
    assert_eq!(v, serde_json::json!({"scope": "global", "agent": "claude-code"}));
    let back: TargetRec = serde_json::from_value(v).unwrap();
    assert_eq!(back, rec);
    let p = TargetRec::Project { agent: "codex".into(), root: PathBuf::from("/x") };
    let back: TargetRec = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
    assert_eq!(back, p);
}

#[test]
fn load_rejects_legacy_version() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = Layout::at(tmp.path().to_path_buf());
    // v2 旧格式（target 带 kind 标签）必须被拒绝，不得静默吞掉
    std::fs::write(layout.registry_path(), r#"{"version":2,"sources":{},"installs":[{"skill":"a","source":"s","source_path":"s","target":{"kind":"global","name":"agents"},"method":"copy","commit":"c","installed_at":"t"}]}"#).unwrap();
    let err = Registry::load(&layout).unwrap_err();
    assert!(err.to_string().contains("registry.json"), "{err}");
}
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test --lib paths 2>&1 | tail -3`
预期：编译失败（`Scope` 未定义）。

- [ ] **步骤 3：实现**

`src/core/paths.rs` 的 `Target` 整体替换：

```rust
use super::agents;
use super::config::Config;
use super::error::{Error, Result};
use std::fmt;
use std::path::PathBuf;

#[derive(Clone, PartialEq, Debug)]
pub enum Scope {
    /// 项目级：值为项目根，最终目录 = <root>/<agent.project_dir>
    Project(PathBuf),
    Global,
}

/// 安装目标 = agent × scope。agent 为内置注册表 id 或 config 自定义名。
#[derive(Clone, PartialEq, Debug)]
pub struct Target {
    pub agent: String,
    pub scope: Scope,
}

impl Target {
    /// 支持新语法 `<agent>@global` / `<agent>@project:<abs路径>`，
    /// 兼容旧 -t 写法 `global:<agent>` 与 `project:<abs>`（= universal agents + 项目根）。
    pub fn parse(s: &str) -> Result<Target> {
        if let Some(rest) = s.strip_prefix("global:") {
            if rest.is_empty() { return Err(Error::BadTarget(s.into())); }
            return Ok(Target { agent: rest.into(), scope: Scope::Global });
        }
        if let Some(rest) = s.strip_prefix("project:") {
            let p = PathBuf::from(rest);
            if !p.is_absolute() { return Err(Error::BadTarget(s.into())); }
            return Ok(Target { agent: "agents".into(), scope: Scope::Project(p) });
        }
        let (agent, rest) = s.split_once('@').ok_or_else(|| Error::BadTarget(s.into()))?;
        if agent.is_empty() { return Err(Error::BadTarget(s.into())); }
        match rest {
            "global" => Ok(Target { agent: agent.into(), scope: Scope::Global }),
            _ => {
                let p = PathBuf::from(rest.strip_prefix("project:").ok_or_else(|| Error::BadTarget(s.into()))?);
                if !p.is_absolute() { return Err(Error::BadTarget(s.into())); }
                Ok(Target { agent: agent.into(), scope: Scope::Project(p) })
            }
        }
    }

    pub fn install_dir(&self, cfg: &Config) -> Result<PathBuf> {
        let spec = agents::resolve(&self.agent, &cfg.targets)
            .ok_or_else(|| Error::UnknownTarget(self.agent.clone()))?;
        match &self.scope {
            Scope::Global => spec.global_dir.clone().ok_or_else(|| {
                Error::Msg(format!("agent {} 不支持全局安装", self.agent))
            }),
            Scope::Project(root) => spec
                .project_dir
                .map(|rel| root.join(rel))
                .ok_or_else(|| Error::Msg(format!("agent {} 没有项目安装目录（自定义 target 仅支持全局）", self.agent))),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.scope {
            Scope::Global => write!(f, "{}@global", self.agent),
            Scope::Project(root) => write!(f, "{}@project:{}", self.agent, root.display()),
        }
    }
}
```

`src/core/registry.rs`：

```rust
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum TargetRec {
    Global { agent: String },
    Project { agent: String, root: PathBuf },
}

impl TargetRec {
    pub fn to_target(&self) -> crate::core::paths::Target {
        match self {
            TargetRec::Global { agent } => crate::core::paths::Target {
                agent: agent.clone(),
                scope: crate::core::paths::Scope::Global,
            },
            TargetRec::Project { agent, root } => crate::core::paths::Target {
                agent: agent.clone(),
                scope: crate::core::paths::Scope::Project(root.clone()),
            },
        }
    }
}
```

`Registry::load` 在 `serde_json::from_value` 之前加版本闸门：

```rust
let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
if version != 3 {
    return Err(Error::Msg(format!(
        "registry.json 版本过旧（v{version}）：v3 起 target 改为 agent×scope 格式不做迁移，\
         请备份后删除 {} 再重装技能",
        path.display()
    )));
}
```

新建时 `version: 3`；删除 `take_legacy_install_tags` 及其调用（v1 迁移随 v3 一起废弃）；`save` 不变。`reg.version = 3` 固定。

- [ ] **步骤 4：运行测试验证通过**

运行：`cargo test --lib paths && cargo test --lib registry`
预期：PASS（全项目 `cargo build` 此时会红，属预期，任务 3 修复）。

- [ ] **步骤 5：Commit**

```bash
cargo fmt && git add src/core/paths.rs src/core/registry.rs
git commit -m "refactor(core): Target 重构为 agent×scope，registry 升级 v3（不兼容旧版）"
```

---

## 任务 3：core 调用点与 config 适配（恢复全项目编译）

**文件：**
- 修改：`src/core/config.rs`（`Config::default` 不再预填 targets）
- 修改：`src/core/install.rs`（`to_rec` 等 Target 构造点）
- 修改：`src/core/update.rs`、`src/core/remove.rs`、`src/core/favorites.rs`（Target/TargetRec 适配）
- 修改：`src/cli/commands.rs`、`src/tui/mod.rs`、`src/web/api.rs` 中除 add 向导外的所有 Target 使用点（list/remove/update/auto-update 的解析与展示用 `Target::parse` 和 `Display`）

- [ ] **步骤 1：改 config**

`Config::default()` 的 targets 改为空 map（内置默认已进 agents 表；`cfg.targets` 现在只装文件里的覆盖/自定义项）：

```rust
impl Default for Config {
    fn default() -> Self {
        Config { targets: BTreeMap::new(), default_method: Method::Symlink, web_port: 7823 }
    }
}
```

config.rs 两个受影响测试改写：`no_config_file_uses_builtin_defaults` 断言改为 `cfg.targets.is_empty()` 且 `agents::resolve("claude-code", &cfg.targets)` 有值；`config_file_overrides_and_extends` 删 `contains_key("agents")` 断言，改为断言 `agents::resolve("claude-code", &cfg.targets).unwrap().global_dir` 等于覆盖后的 `~/.cursor/skills` 式路径（用 cursor 覆盖例）。

- [ ] **步骤 2：适配 install.rs 的 to_rec**

找到 `pub fn to_rec(t: &Target) -> TargetRec`（install.rs），改为：

```rust
pub fn to_rec(t: &Target) -> TargetRec {
    match &t.scope {
        Scope::Global => TargetRec::Global { agent: t.agent.clone() },
        Scope::Project(root) => TargetRec::Project { agent: t.agent.clone(), root: root.clone() },
    }
}
```

- [ ] **步骤 3：编译驱动适配其余调用点**

反复跑 `cargo build 2>&1 | grep '^error' -A3`，逐个修：所有 `Target::Global { name }` / `Target::Project { root }` 的构造/匹配换成新结构；展示处从 `{t:?}` 换成 `{t}`（Display）；`default_global_target`（commands.rs:437）删除（任务 4 重写 add 时用不到）。list/remove/update/auto-update 的 `-t` 解析仍走 `Target::parse`（新旧语法都收）。

- [ ] **步骤 4：全量测试修复**

运行：`cargo test`
预期：编译通过后，旧断言按新语义修（如 e2e 里 `global:agents` 的断言仍有效——`Target::parse("global:agents")` 映射到 universal 全局）。逐一修到全绿。

- [ ] **步骤 5：Commit**

```bash
cargo fmt && git add -A src tests
git commit -m "refactor: 全项目适配 agent×scope target 模型与 registry v3"
```

---

## 任务 4：共享四步向导 `src/wizard.rs`

**文件：**
- 创建：`src/wizard.rs`
- 修改：`src/main.rs`（加 `mod wizard;`，按现有 mod 声明格式）

- [ ] **步骤 1：编写失败的测试（纯决策函数，不碰 TTY）**

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;

    #[test]
    fn dedup_targets_by_install_dir() {
        // 多个 universal agent 同 scope 解析到同一目录，只留一个
        let cfg = Config::default();
        let ids = vec!["agents".to_string(), "cursor".to_string(), "cline".to_string()];
        let ts = expand_targets(&ids, &Scope::Project(PathBuf::from("/p")), &cfg).unwrap();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].agent, "agents");
        // 全局 scope 下 cursor 与 cline 目录不同，不去重
        let ts = expand_targets(&ids, &Scope::Global, &cfg).unwrap();
        assert_eq!(ts.len(), 3);
    }

    #[test]
    fn expand_skips_global_unsupported() {
        // eve 无 global_dir：expand 时跳过而不是报错（UI 层已过滤，双保险）
        let cfg = Config::default();
        let ts = expand_targets(&["eve".to_string()], &Scope::Global, &cfg).unwrap();
        assert!(ts.is_empty());
    }

    #[test]
    fn needs_scope_prompt_only_when_some_global_capable() {
        assert!(needs_scope_prompt(&["claude-code"]));
        assert!(!needs_scope_prompt(&["eve"])); // 全都不支持全局则不问
    }
}
```

- [ ] **步骤 2：运行验证失败** — `cargo test wizard:: 2>&1 | tail -3`，预期编译失败。

- [ ] **步骤 3：实现**

```rust
//! add / fav install 的四步交互向导：选技能 → 选 agent → 选 scope → 选安装方式。
//! CLI 与 TUI 共用（两者都在普通终端模式下用 dialoguer）；Web 无弹窗，走 /api 参数。
//! 纯决策函数（expand_targets 等）与弹窗函数分离，前者可单测。

use crate::core::agents;
use crate::core::config::Config;
use crate::core::error::{Error, Result};
use crate::core::paths::{Scope, Target};
use crate::core::registry::Method;
use std::path::PathBuf;

/// 技能多选：第一行「全选 (n)」快捷项，默认全不选；空选 = 取消（返回 None）。
/// 单技能直接返回它，不弹窗。
pub fn pick_skills(names: &[String]) -> Result<Option<Vec<String>>>;

/// agent 多选：detected 里的默认勾选；detected 为空时预选 ["agents"]。
/// 返回 None = 空选取消。
pub fn pick_agents(detected: &[&'static str]) -> Result<Option<Vec<String>>>;

/// scope 单选：项目（默认）/ 全局。所有 agent 都不支持全局时直接返回项目，不弹窗。
pub fn pick_scope(agent_ids: &[String]) -> Result<Scope>;

/// 安装方式单选：symlink（默认，推荐）/ copy。
pub fn pick_method() -> Result<Method>;

/// 选中的 agent × scope 展开为 Target 列表：按解析后的 install_dir 去重
/// （多个 universal agent 项目级同目录只装一份，记录保留排序最前的 id）；
/// 跳过 scope 不支持的 agent（如 eve 无全局目录）。
pub fn expand_targets(agent_ids: &[String], scope: &Scope, cfg: &Config) -> Result<Vec<Target>>;

/// 是否需要弹 scope：至少一个选中 agent 支持全局
pub fn needs_scope_prompt(agent_ids: &[&str]) -> bool;
```

`pick_skills` 实现要点：单元素直接 `Ok(Some(vec![names[0]]))`；否则 items = `["全选 (n)"] + names`，dialoguer MultiSelect，若返回索引含 0 → 全选；`interact()` 错误转 `Error::Msg`。

- [ ] **步骤 4：运行验证通过** — `cargo test wizard::`，预期 PASS。

- [ ] **步骤 5：Commit**

```bash
cargo fmt && git add src/wizard.rs src/main.rs
git commit -m "feat: 共享四步安装向导（技能/agent/scope/method）"
```

---

## 任务 5：CLI add / fav install 接四步向导 + 新 flag

**文件：**
- 修改：`src/cli/mod.rs:20-33`（Add flags）、`src/cli/mod.rs:140-160`（FavSub::Install flags）
- 修改：`src/cli/commands.rs:64-136`（Add 分支）、`src/cli/commands.rs:358-412`（FavSub::Install 分支）、删 `resolve_targets`/`pick_targets_interactive`（commands.rs:619-662）

- [ ] **步骤 1：改 flag 定义**

`Cmd::Add` 变为：

```rust
Add {
    source: String,
    /// 指定技能名（可多次）；'*' = 全部
    #[arg(short, long)]
    skill: Vec<String>,
    /// 指定 agent id（可多次）；'*' = 全部
    #[arg(short, long)]
    agent: Vec<String>,
    /// 兼容旧写法：global:<agent> | project:<abs路径>（= universal + 项目根）
    #[arg(short, long)]
    target: Vec<String>,
    /// 全局安装（默认项目级）
    #[arg(short = 'g', long)]
    global: bool,
    #[arg(long, value_enum)]
    method: Option<MethodArg>,
    /// 跳过全部交互：全技能 + 检测到的 agent + 项目 scope + 默认方式
    #[arg(short = 'y', long)]
    yes: bool,
    /// 全技能 + 全 agent + 跳过交互
    #[arg(long)]
    all: bool,
},
```

`FavSub::Install` 同样加 `agent: Vec<String>` 与 `all: bool`（无 `source` 位置参数变化）。

- [ ] **步骤 2：写 add 决策的失败测试**

在 commands.rs tests 加（纯函数，先抽出来）：

```rust
#[test]
fn resolve_skill_picks_star_and_flag() {
    let all = vec!["a".to_string(), "b".to_string()];
    assert_eq!(resolve_skill_flag(&["*".into()], &all).unwrap(), all);
    assert_eq!(resolve_skill_flag(&["a".into()], &all).unwrap(), vec!["a".to_string()]);
    assert!(resolve_skill_flag(&["nope".into()], &all).is_err());
}

#[test]
fn yes_defaults_skills_all_agents_detected_or_universal() {
    // -y 无 -a：检测到的；检测不到 → ["agents"]
    assert_eq!(yes_agents(&["codex"]), vec!["codex".to_string()]);
    assert_eq!(yes_agents(&[]), vec!["agents".to_string()]);
}
```

- [ ] **步骤 3：重写 Add 分支**

流程（替换 commands.rs:86-104 的选择段，`install_loop` 与逐条落盘不动）：

```rust
let all_skills = cache::scan_skills(&cached.path)?;
let all_names: Vec<String> = all_skills.iter().map(|s| s.name.clone()).collect();
// 第 1 步：技能。--all 或 -y 或 -s '*' → 全部；-s 列表 → 校验后用之；
// 否则 TTY 弹 pick_skills，非 TTY → 全部
let picked = if all || yes || skill.iter().any(|s| s == "*") {
    all_names.clone()
} else if !skill.is_empty() {
    resolve_skill_flag(&skill, &all_names)? // 不存在的名字报错（保持预校验语义）
} else if std::io::stdin().is_terminal() {
    match wizard::pick_skills(&all_names)? { Some(p) => p, None => { println!("未选择技能，取消安装"); return Ok(()); } }
} else {
    all_names.clone() // 非交互兜底：全技能（修掉旧版管道下弹窗报错的不对称）
};
// 第 2 步：agent。--all → 全表；-a '*' → 全表；-a 列表 → 校验 id 存在；
// -t 旧写法 → 直接 parse 成 targets 跳到安装；否则 TTY 弹 pick_agents(检测预选)，非 TTY → ["agents"]
// 第 3 步：scope。-g → Global；-t 已给 → 由 -t 决定；否则 TTY 且 needs_scope_prompt → pick_scope，否则 Project(cwd)
// 第 4 步：method。--method → 用之；否则 TTY → pick_method()，非 TTY → cfg.default_method
let targets = wizard::expand_targets(&agent_ids, &scope, &cfg)?;
```

`-a` 列表校验：`agents::resolve(id, &cfg.targets).is_none()` → `Error::Msg(format!("未知 agent {id}"))`，全部先校验再开工（与技能预校验同理）。`-t` 与 `-a` 同时给 → 报错「-t 与 -a 只能给一个」。摘要行：`println!("将安装 {} 个技能 → {} 个目标（{method:?}）", picked.len(), targets.len())`。

`resolve_skill_flag` / `yes_agents` 为纯函数，放 commands.rs。

- [ ] **步骤 4：fav install 接同款流程**

`FavSub::Install` 分支（commands.rs:358-412）：技能来源换成收藏集（现有逻辑保留：单技能直接装、多技能弹选——弹选改用 `wizard::pick_skills`），agent/scope/method 复用与 Add 相同的代码段（抽成一个 `fn resolve_install_plan(agent: &[String], target: &[String], global: bool, all: bool, yes: bool, cfg: &Config) -> Result<(Vec<Target>, Method)>`，Add 与 fav 共用）。

- [ ] **步骤 5：跑测试到全绿 + Commit**

```bash
cargo test
cargo fmt && git add src/cli
git commit -m "feat(cli): add/fav install 四步向导，新增 -a/--agent 与 --all"
```

---

## 任务 6：TUI 向导接共享 wizard

**文件：**
- 修改：`src/tui/mod.rs:143-228`（install_wizard）、`src/tui/mod.rs:293-364`（fav_install_wizard）；删 `pick_targets`（tui/mod.rs:232-251）

- [ ] **步骤 1：install_wizard 改四步**

`scan_skills` 后的技能多选换成 `wizard::pick_skills(&names)?`（None → 打印取消返回）；然后 `wizard::pick_agents(&agents::detect_installed())?` → `wizard::pick_scope(&ids)?` → `wizard::pick_method()?` → `wizard::expand_targets(&ids, &scope, cfg)?` → 摘要行 → 现有双重循环装（`install::install_skill` 调用与 Conflict 确认、内存 `app.registry` 直接操作、结尾 `reg.save` 全部不动）。

- [ ] **步骤 2：fav_install_wizard 同款**

技能集确定逻辑（当前行单技能 / Source 行多选）保留，多选换 `wizard::pick_skills`；后续 agent/scope/method/expand 与 install_wizard 相同（TUI 内可抽一个本地小函数共用，或各自顺序调四个 wizard 函数——两者行数都很少，不过早抽象）。

- [ ] **步骤 3：验证 + Commit**

```bash
cargo test && cargo build
cargo fmt && git add src/tui
git commit -m "feat(tui): 安装向导对齐四步交互"
```

---

## 任务 7：Web API 与前端

**文件：**
- 修改：`src/web/api.rs`（新增 `/api/agents`；`InstallReq`/`FavInstallReq` 换字段）
- 修改：`src/web/static/index.html`（安装面板加 agent 多选、scope/method 单选）

- [ ] **步骤 1：写 /api/agents 的失败测试**

api.rs tests 加：

```rust
#[tokio::test]
async fn list_agents_returns_registry_with_detected_flag() {
    let app = router(test_state());
    let resp = app
        .oneshot(Request::builder().uri("/api/agents").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let agents = v["agents"].as_array().unwrap();
    assert!(agents.len() >= 60);
    let claude = agents.iter().find(|a| a["id"] == "claude-code").unwrap();
    assert_eq!(claude["project_dir"], ".claude/skills");
    assert!(claude.get("detected").is_some());
    // eve 不支持全局
    let eve = agents.iter().find(|a| a["id"] == "eve").unwrap();
    assert_eq!(eve["global_supported"], false);
}
```

- [ ] **步骤 2：实现 /api/agents 与 InstallReq 改造**

```rust
async fn list_agents(State(s): S) -> Json<serde_json::Value> {
    let detected = crate::core::agents::detect_installed();
    Json(serde_json::json!({
        "agents": crate::core::agents::all().iter().map(|a| serde_json::json!({
            "id": a.id, "name": a.name, "project_dir": a.project_dir,
            "global_supported": a.global_dir.is_some(),
            "universal": a.universal,
            "detected": detected.contains(&a.id),
        })).collect::<Vec<_>>()
    }))
}
```

`InstallReq`/`FavInstallReq` 的 `targets: Vec<TargetRec>` 换成：

```rust
agents: Vec<String>,            // agent id 列表
scope: Option<String>,          // "project" | "global"，默认 project
// project 时项目根 = 服务器进程 cwd（web 为本机工具）
```

服务端用 `wizard::expand_targets(&agents, &scope, &cfg)` 展开 + 去重，再 `to_rec` 成 `TargetRec` 走原有安装循环（逐条落盘、409 冲突重试逻辑不动）。`scope` 非法值 → 400。路由表（api.rs:33-49）加 `.route("/api/agents", get(list_agents))`。`list_targets`（api.rs:512）改为返回 agent 注册表视图（或直接被 /api/agents 取代后删除，前端不再引用则删）。

- [ ] **步骤 3：前端面板**

index.html 安装表单：技能勾选下方加「agent 多选」（checkbox 列表，检测到的默认勾 + 「全选」一个快捷 checkbox）、「范围」两个 radio（项目 ./ 默认 / 全局，所选全不支持全局时禁用全局）、「方式」radio（symlink 默认 / copy）。提交时按新字段组装 JSON。纯前端改动，跟着现有表单代码风格走。

- [ ] **步骤 4：验证 + Commit**

```bash
cargo test --lib web 2>/dev/null || cargo test
cargo fmt && git add src/web
git commit -m "feat(web): /api/agents 与四步安装面板"
```

---

## 任务 8：e2e / cli_smoke 适配与新链路

**文件：**
- 修改：`tests/e2e.rs`、`tests/cli_smoke.rs`

- [ ] **步骤 1：适配既有断言**

全量跑 `cargo test --test e2e --test cli_smoke`，失败的按新语义修：target 参数写法（`global:agents` 等旧写法仍兼容，尽量不改 fixture 调用）、registry 断言里的 target JSON 结构（`{"scope":"global","agent":"..."}`）、安装路径断言（项目级 universal 仍是 `.agents/skills`，不变）。

- [ ] **步骤 2：新增链路测试（e2e，本地 bare 仓库 + SKILLS_HOME 隔离，不联网）**

```rust
#[test]
fn add_with_agent_installs_to_agent_project_dir() {
    // add <repo> -s <skill> -a claude-code → <cwd>/.claude/skills/<skill> 存在
    // registry 里 target == {"scope":"project","agent":"claude-code","root":<cwd>}
}

#[test]
fn add_multiple_universal_agents_dedup() {
    // -a agents -a cline（都 universal）→ 磁盘只有一份 .agents/skills/<skill>，registry 只有一条
}

#[test]
fn add_global_agent_installs_to_global_dir() {
    // -g -a codex → $HOME/.codex/skills（SKILLS_HOME 隔离时 home 用测试临时 home 或断言 install_dir 解析）
}

#[test]
fn non_tty_bare_add_defaults_all_skills_to_project_universal() {
    // 管道下不带 -s/-a：全技能装到 <cwd>/.agents/skills（旧版此时报错，此为行为修复回归）
}

#[test]
fn add_yes_and_all_flags() {
    // -y：全技能 + 检测不到 agent 时 universal + 项目；--all：注册表全 agent（仅断言成功与目标数 > 1）
}

#[test]
fn legacy_registry_v2_is_rejected() {
    // 手放一个 version:2 的 registry.json，任意命令报错且信息含「备份后删除」
}
```

- [ ] **步骤 3：全绿 + Commit**

```bash
cargo test
cargo fmt && git add tests
git commit -m "test: e2e/smoke 适配新 target 模型，补四步向导链路用例"
```

---

## 任务 9：文档、验证与收尾

**文件：**
- 修改：`AGENTS.md`（「核心概念」的 target 段、常用命令、`docs/index.md` 索引若有 specs 条目）

- [ ] **步骤 1：更新 AGENTS.md**

- target 段改写为：target = agent × scope；内置 agent 注册表（`src/core/agents.rs`，移植自 vercel-labs/skills）；项目级 = `<root>/<agent.project_dir>`，universal 共享 `.agents/skills`；全局 = agent 全局目录，`config.toml [targets]` 只做覆盖/自定义。
- registry v3、不兼容旧版的说明。
- 交互流程一段：裸 add 四步（技能→agent→scope→方式）。

- [ ] **步骤 2：本地全量验证**

```bash
cargo test && cargo clippy && cargo fmt --check
```

- [ ] **步骤 3：测试机验证（AGENTS.md 既定工作流，自动执行）**

```bash
rsync -a --exclude-from=scripts/rsync-exclude.txt ./ test@192.168.188.210:/mnt/sda3/deploy/skills/tools/
ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && cargo build'
# 真实场景：SKILLS_HOME=$(mktemp -d) 下 add（-s/-a/-g/--method 组合）→ list → tag → update --dry-run → remove 全链路，用本地 bare 仓库 fixture
```

- [ ] **步骤 4：最终 Commit**

```bash
git add AGENTS.md docs && git commit -m "docs: target 模型与四步安装向导文档更新"
```

（不 push，等用户指令。）

---

## 自检记录

- 规格覆盖：四步交互（任务 4/5/6/7）、agent 注册表（任务 1）、target 重构（任务 2/3）、registry v3 不兼容（任务 2/8）、config 覆盖语义（任务 3）、三前端统一（任务 4-7）、非交互兜底（任务 5/8）、安全不变量保持（全局约束 + 任务 3 不改事务代码）、AGENTS.md 更新（任务 9）。
- 明确不做：universal 锁定组、选择记忆、最终确认弹窗、安全审计/telemetry。
