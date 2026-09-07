# CLI 安装交互体验升级 实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 对标 vercel-labs/skills，把 CLI/TUI 安装向导升级为 inquire 搜索式多选 + spinner 进度 + 染色输出 + 完成摘要 + agent 选择记忆。

**架构：** 只动表现层——`src/wizard.rs`（弹窗）从 dialoguer 迁移到 inquire，新增 `src/cli/ui.rs`（spinner/染色辅助）与 `src/core/state.rs`（`$SKILLS_HOME/state.json` 选择记忆）；core 的安装/git/registry 逻辑与 6 条安全不变量不触碰。

**技术栈：** Rust edition 2024；inquire 0.9.4（交互弹窗，替换 dialoguer）、indicatif 0.18.6（spinner）、console 0.16.4（染色）。

**规格：** `docs/superpowers/specs/2026-09-07-cli-install-ux-design.md`

---

## 文件结构

| 文件 | 职责 | 动作 |
|---|---|---|
| `Cargo.toml` | 依赖：删 dialoguer，加 inquire/indicatif/console | 修改 |
| `src/core/paths.rs` | `Layout` 增加 `state_path()` | 修改 |
| `src/core/state.rs` | UI 状态（state.json）读写：agent 选择记忆 | 新建 |
| `src/core/mod.rs` | 导出 `pub mod state;` | 修改 |
| `src/cli/ui.rs` | spinner（indicatif）、染色状态行（console）、method 用户文案 | 新建 |
| `src/cli/mod.rs` | 导出 `pub mod ui;` | 修改 |
| `src/wizard.rs` | 四步向导重写为 inquire；纯决策函数保留并扩充 | 重写 |
| `src/cli/commands.rs` | add/fav install/update 接线：spinner、摘要、记忆、取消语义 | 修改 |
| `src/tui/mod.rs` | TUI 三处向导迁移 inquire、接记忆 | 修改 |
| `AGENTS.md` | 向导描述与状态文件清单同步新行为 | 修改 |

## 关键 API 事实（已核实 docs.rs，实现时以此为准）

- `inquire::MultiSelect::new(message, options: Vec<T: Display>)`：打字过滤默认开启（substring scorer）；空格勾选、**→ 全选、← 全清**、回车确认；`.with_default(Vec<usize>)` 传**索引**做预选；`.with_page_size(20)`；`.with_validator(inquire::min_length!(1, "msg"))` 禁止空选提交；`.with_help_message(...)`。
- `inquire::Select::new(message, options)`：`.with_starting_cursor(0)` 设默认项。
- `inquire::Confirm::new(msg).with_default(false).prompt()`。
- `inquire::Text::new(msg).prompt()`（空输入默认允许，适合"按回车继续"）。
- 取消：ESC → `Err(inquire::error::InquireError::OperationCanceled)`，Ctrl+C → `Err(InquireError::OperationInterrupted)`（crossterm 后端）。
- `indicatif::ProgressBar`：`ProgressBar::new_spinner()` / `ProgressBar::hidden()`；`set_style(ProgressStyle::with_template(...))`（返回 Result）；`enable_steady_tick(Duration)`；`pb.println(line)` 在 bar 上方打日志；`pb.suspend(|| ...)` 挂起 bar 跑弹窗；`finish_and_clear()` 收尾。
- `console::style("✓").green()`：非 TTY 时自动不输出 ANSI 色码（不影响集成测试的字符串断言）。

---

### 任务 1：依赖切换 + state.json 记忆存储

**文件：**
- 修改：`Cargo.toml`
- 修改：`src/core/paths.rs`（`registry_path` 附近，124-128 行区域）
- 新建：`src/core/state.rs`
- 修改：`src/core/mod.rs`

- [ ] **步骤 1：换依赖**

`Cargo.toml` 删除 `dialoguer = "0.11"`，在 `[dependencies]` 加：

```toml
inquire = "0.9"
indicatif = "0.18"
console = "0.16"
```

- [ ] **步骤 2：先写 state.rs 的失败测试**

新建 `src/core/state.rs`，先只放测试（实现下一步补）：

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Layout;

    #[test]
    fn missing_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Layout::at(tmp.path().to_path_buf());
        assert!(load(&l).last_selected_agents.is_empty());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Layout::at(tmp.path().to_path_buf());
        save_last_agents(&l, &["claude-code".to_string(), "codex".to_string()]);
        assert_eq!(load(&l).last_selected_agents, vec!["claude-code", "codex"]);
    }

    #[test]
    fn corrupted_file_returns_default() {
        let tmp = tempfile::tempdir().unwrap();
        let l = Layout::at(tmp.path().to_path_buf());
        std::fs::write(l.state_path(), "not json {{{").unwrap();
        assert!(load(&l).last_selected_agents.is_empty());
    }
}
```

- [ ] **步骤 3：Layout 加 state_path + 实现 state.rs**

`src/core/paths.rs` 在 `config_path` 后加（与 `registry_path` 同款模式）：

```rust
    pub fn state_path(&self) -> PathBuf {
        self.root.join("state.json")
    }
```

`src/core/state.rs` 实现（写在测试上方）：

```rust
//! UI 状态（$SKILLS_HOME/state.json）：交互向导的记忆数据（当前仅 agent 选择记忆）。
//! 与 registry 无关；缺失/损坏一律降级为默认空状态，绝不影响核心功能。

use serde::{Deserialize, Serialize};

use super::paths::Layout;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct UiState {
    #[serde(default = "one")]
    pub version: u32,
    #[serde(default)]
    pub last_selected_agents: Vec<String>,
}

fn one() -> u32 {
    1
}

/// 读取 UI 状态；文件缺失或损坏都返回默认（记忆只是体验优化，不是权威数据）。
pub fn load(layout: &Layout) -> UiState {
    std::fs::read_to_string(layout.state_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// 记录上次交互确认的 agent 选择。写失败仅警告——记忆丢失不该阻塞安装。
pub fn save_last_agents(layout: &Layout, agents: &[String]) {
    let state = UiState {
        version: 1,
        last_selected_agents: agents.to_vec(),
    };
    if let Err(e) = write_atomic(layout, &state) {
        eprintln!("警告：UI 状态写入失败（{e}），agent 选择记忆未保存");
    }
}

/// tmp + rename 原子写（与 registry.save 同款语义，防半截文件）。
fn write_atomic(layout: &Layout, state: &UiState) -> Result<(), String> {
    let path = layout.state_path();
    let text = serde_json::to_string_pretty(state).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
}
```

`src/core/mod.rs` 按现有模块排列风格加 `pub mod state;`。

- [ ] **步骤 4：跑测试验证通过**

运行：`cargo test state`
预期：3 个测试 PASS。

- [ ] **步骤 5：确认编译无 dialoguer 残留引用前不删干净会报错——本任务先留着引用**

运行：`cargo build 2>&1 | head -5`
预期：编译通过（dialoguer 已从 Cargo.toml 删除，但 wizard.rs/commands.rs/tui 仍引用它，会报 `unresolved import dialoguer` 等错）。**这是预期的红状态**，后续任务逐个修复；若只想本任务绿，可先保留 dialoguer 依赖到任务 6 再删——任选，但任务 6 结束时 dialoguer 必须在 Cargo.toml 与代码中双清零。

- [ ] **步骤 6：Commit**

```bash
cargo fmt
git add Cargo.toml Cargo.lock src/core/paths.rs src/core/state.rs src/core/mod.rs
git commit -m "新增 state.json 选择记忆存储，引入 inquire/indicatif/console 依赖"
```

---

### 任务 2：CLI 输出辅助模块 ui.rs

**文件：**
- 新建：`src/cli/ui.rs`
- 修改：`src/cli/mod.rs`
- 测试：`src/cli/ui.rs` 内 `#[cfg(test)]`

- [ ] **步骤 1：写失败测试**

`src/cli/ui.rs` 先放：

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_label_is_user_facing() {
        assert_eq!(method_label(Method::Symlink), "符号链接");
        assert_eq!(method_label(Method::Copy), "独立副本");
    }
}
```

- [ ] **步骤 2：实现 ui.rs**

```rust
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
```

`src/cli/mod.rs` 在 `mod commands;` 附近加 `pub mod ui;`（pub：TUI 向导也复用染色行）。

- [ ] **步骤 3：跑测试**

运行：`cargo test method_label`
预期：PASS。

- [ ] **步骤 4：Commit**

```bash
cargo fmt
git add src/cli/ui.rs src/cli/mod.rs
git commit -m "新增 CLI 输出辅助：阶段 spinner 与染色状态行"
```

---

### 任务 3：wizard 迁移 inquire——技能/scope/方式三步 + 统一取消映射

**文件：**
- 重写：`src/wizard.rs`（本任务只动 pick_skills/pick_scope/pick_method 与新类型；pick_agents 留给任务 4；expand_targets/needs_scope_prompt 及其测试原样保留）
- 测试：`src/wizard.rs` 内 `#[cfg(test)]`

- [ ] **步骤 1：写失败测试（单技能不弹窗是唯一的非交互可测路径）**

```rust
    #[test]
    fn single_skill_skips_prompt() {
        let skills = vec![SkillChoice {
            name: "alpha".to_string(),
            description: "A 技能".to_string(),
        }];
        assert_eq!(pick_skills(&skills).unwrap(), Some(vec!["alpha".to_string()]));
    }
```

- [ ] **步骤 2：重写 wizard.rs 头部与三个弹窗函数**

模块文档注释改为 `//! add / fav install 的四步交互向导：选技能 → 选 agent → 选 scope → 选安装方式。CLI 与 TUI 共用（两者都在普通终端模式下用 inquire）`。类型与取消映射：

```rust
use inquire::error::InquireError;

/// 技能选项：多选行显示 "name — description"。
#[derive(Clone)]
pub struct SkillChoice {
    pub name: String,
    pub description: String,
}

impl std::fmt::Display for SkillChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.description.is_empty() {
            write!(f, "{}", self.name)
        } else {
            write!(f, "{} — {}", self.name, self.description)
        }
    }
}

/// scope 选项（Display 即说明文案，返回后按枚举匹配，不做字符串解析）。
enum ScopeChoice {
    Project,
    Global,
}

impl std::fmt::Display for ScopeChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ScopeChoice::Project => write!(f, "项目（当前目录，跟随项目提交）"),
            ScopeChoice::Global => write!(f, "全局（用户主目录，跨项目可用）"),
        }
    }
}

enum MethodChoice {
    Symlink,
    Copy,
}

impl std::fmt::Display for MethodChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MethodChoice::Symlink => write!(f, "符号链接（指向缓存，单一来源易更新，推荐）"),
            MethodChoice::Copy => write!(f, "独立副本（脱离缓存，可单独控制更新）"),
        }
    }
}

/// 统一取消语义：ESC/Ctrl+C → Ok(None)（调用方视为「已取消」，退出码 0）；
/// 其余错误（非 TTY、IO 等）照旧向上抛。
fn cancel_to_none<T>(r: std::result::Result<T, InquireError>) -> Result<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(InquireError::OperationCanceled | InquireError::OperationInterrupted) => Ok(None),
        Err(e) => Err(Error::Msg(e.to_string())),
    }
}
```

pick_skills（签名变化：接收 SkillChoice，空选由 validator 拦截，取消走 None）：

```rust
/// 技能多选：打字过滤、→ 全选、← 全清；单技能直接返回它，不弹窗。
/// 返回 None = 用户 ESC/Ctrl+C 取消。
pub fn pick_skills(skills: &[SkillChoice]) -> Result<Option<Vec<String>>> {
    if skills.len() == 1 {
        return Ok(Some(vec![skills[0].name.clone()]));
    }
    let r = inquire::MultiSelect::new("选择要安装的技能", skills.to_vec())
        .with_help_message("输入过滤，空格勾选，→ 全选，← 全清，回车确认，ESC 取消")
        .with_page_size(20)
        .with_validator(inquire::min_length!(1, "至少选择一个技能"))
        .prompt();
    cancel_to_none(r)
        .map(|o| o.map(|picked| picked.into_iter().map(|c| c.name).collect()))
}
```

pick_scope / pick_method（返回 Option，取消 = None）：

```rust
/// scope 单选：项目（默认）/ 全局。所有 agent 都不支持全局时直接返回项目，不弹窗。
pub fn pick_scope(agent_ids: &[String]) -> Result<Option<Scope>> {
    let ids: Vec<&str> = agent_ids.iter().map(|s| s.as_str()).collect();
    if !needs_scope_prompt(&ids) {
        return Ok(Some(Scope::Project(std::env::current_dir()?)));
    }
    let r = inquire::Select::new("安装范围", vec![ScopeChoice::Project, ScopeChoice::Global])
        .with_starting_cursor(0)
        .prompt();
    cancel_to_none(r)?.map(|c| -> Result<Option<Scope>> {
        Ok(Some(match c {
            ScopeChoice::Project => Scope::Project(std::env::current_dir()?),
            ScopeChoice::Global => Scope::Global,
        }))
    }).transpose().map(|x| x.flatten())
}

/// 安装方式单选：符号链接（默认，推荐）/ 独立副本。
pub fn pick_method() -> Result<Option<Method>> {
    let r = inquire::Select::new("安装方式", vec![MethodChoice::Symlink, MethodChoice::Copy])
        .with_starting_cursor(0)
        .prompt();
    cancel_to_none(r).map(|o| {
        o.map(|c| match c {
            MethodChoice::Symlink => Method::Symlink,
            MethodChoice::Copy => Method::Copy,
        })
    })
}
```

（pick_scope 的 Option 嵌套写法若读着别扭，可写成显式 match；语义不变。）

- [ ] **步骤 3：跑测试**

运行：`cargo test wizard`
预期：`single_skill_skips_prompt`、既有 `dedup_targets_by_install_dir`、`expand_skips_global_unsupported`、`needs_scope_prompt_only_when_some_global_capable` 全 PASS（编译错误属于任务 4/5/6 待接线部分时先跳过，以 `cargo test wizard` 编译范围内全绿为准——wizard.rs 自身必须先编过）。

- [ ] **步骤 4：Commit**

```bash
cargo fmt
git add src/wizard.rs
git commit -m "向导技能/范围/方式三步迁移 inquire：搜索过滤、全选键、统一取消语义"
```

---

### 任务 4：pick_agents 升级——检测置顶、目录提示、记忆预选、单检测省略

**文件：**
- 修改：`src/wizard.rs`
- 测试：`src/wizard.rs` 内 `#[cfg(test)]`

- [ ] **步骤 1：写失败测试（纯决策函数全覆盖）**

```rust
    #[test]
    fn agent_defaults_priority() {
        // 记忆优先，且过滤已失效 id
        assert_eq!(
            agent_defaults(&["codex"], &["claude-code".to_string(), "ghost".to_string()]),
            vec!["claude-code".to_string()]
        );
        // 无有效记忆 → 检测到的
        assert_eq!(agent_defaults(&["codex"], &[]), vec!["codex".to_string()]);
        // 全无 → universal 兜底
        assert_eq!(agent_defaults(&[], &[]), vec!["agents".to_string()]);
    }

    #[test]
    fn detected_first_ordering() {
        let specs = detected_first(&["zed"]);
        assert_eq!(specs[0].id, "zed", "检测到的必须置顶");
        assert!(
            specs[1..].windows(2).all(|w| w[0].id < w[1].id),
            "未检测组保持 id 字典序"
        );
    }
```

- [ ] **步骤 2：实现 AgentChoice、两个纯函数与新版 pick_agents**

```rust
/// agent 选项：行内显示 id（名称）→ 项目目录，检测到的带标记。
pub struct AgentChoice {
    pub id: &'static str,
    pub name: &'static str,
    pub project_dir: &'static str,
    pub detected: bool,
}

impl std::fmt::Display for AgentChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let mark = if self.detected { "  ✓已检测" } else { "" };
        write!(f, "{}（{}）→ {}{}", self.id, self.name, self.project_dir, mark)
    }
}

/// 预选决策：上次选择的记忆（过滤已失效 id）> 检测到的 > ["agents"]。
pub fn agent_defaults(detected: &[&str], last_selected: &[String]) -> Vec<String> {
    let remembered: Vec<String> = last_selected
        .iter()
        .filter(|id| agents::all().iter().any(|a| a.id == id.as_str()))
        .cloned()
        .collect();
    if !remembered.is_empty() {
        return remembered;
    }
    if !detected.is_empty() {
        return detected.iter().map(|s| s.to_string()).collect();
    }
    vec!["agents".to_string()]
}

/// 列表排序：检测到的置顶（两组各自保持注册表的 id 字典序）。
pub fn detected_first(detected: &[&str]) -> Vec<&'static agents::AgentSpec> {
    let mut out: Vec<_> = agents::all()
        .iter()
        .filter(|a| detected.contains(&a.id))
        .collect();
    out.extend(agents::all().iter().filter(|a| !detected.contains(&a.id)));
    out
}

/// agent 多选：恰好检测到 1 个时不弹窗直接用；否则搜索式多选，
/// 预选 = agent_defaults（记忆 > 检测 > agents）。返回 None = 取消。
/// 只列内置 agent（config 自定义 target 无法检测本机安装，走 -a 参数指定）。
pub fn pick_agents(detected: &[&'static str], last_selected: &[String]) -> Result<Option<Vec<String>>> {
    if let [only] = detected {
        let name = agents::all()
            .iter()
            .find(|a| a.id == *only)
            .map(|a| a.name)
            .unwrap_or(only);
        println!("安装到：{name}（已检测）");
        return Ok(Some(vec![only.to_string()]));
    }
    let specs = detected_first(detected);
    let want = agent_defaults(detected, last_selected);
    let default_idx: Vec<usize> = specs
        .iter()
        .enumerate()
        .filter(|(_, a)| want.iter().any(|w| w == a.id))
        .map(|(i, _)| i)
        .collect();
    let options: Vec<AgentChoice> = specs
        .iter()
        .map(|a| AgentChoice {
            id: a.id,
            name: a.name,
            project_dir: a.project_dir,
            detected: detected.contains(&a.id),
        })
        .collect();
    let r = inquire::MultiSelect::new("安装到哪些 agent", options)
        .with_default(default_idx)
        .with_help_message("输入过滤，空格勾选，→ 全选，← 全清，回车确认，ESC 取消")
        .with_page_size(20)
        .with_validator(inquire::min_length!(1, "至少选择一个 agent"))
        .prompt();
    cancel_to_none(r)
        .map(|o| o.map(|cs| cs.into_iter().map(|c| c.id.to_string()).collect()))
}
```

- [ ] **步骤 3：跑测试**

运行：`cargo test wizard`
预期：全部 PASS。

- [ ] **步骤 4：Commit**

```bash
cargo fmt
git add src/wizard.rs
git commit -m "agent 选择升级：检测置顶、目录提示、记忆预选、单检测免弹窗"
```

---

### 任务 5：commands.rs 接线——spinner、完成摘要、记忆读写、取消与文案统一

**文件：**
- 修改：`src/cli/commands.rs`

- [ ] **步骤 1：imports 与 add 分支的缓存/扫描阶段加 spinner**

`commands.rs` 顶部加 `use super::ui;` 与 `use crate::core::state;`。add 分支改为：

```rust
            let spec = parse_source(&source)?;
            let sp = ui::spinner("正在获取技能包…");
            let cached = match cache::ensure_cached(&layout, &spec) {
                Ok(c) => {
                    sp.finish_and_clear();
                    c
                }
                Err(e) => {
                    sp.finish_and_clear();
                    return Err(e);
                }
            };
            if !cached.fresh {
                println!("已缓存 {}，复用（skills update 可更新）", spec.key);
            }
            // （registry sources 登记照旧，略）
            let sp = ui::spinner("正在扫描技能…");
            let all_skills = match cache::scan_skills(&cached.path) {
                Ok(s) => {
                    sp.finish_and_clear();
                    s
                }
                Err(e) => {
                    sp.finish_and_clear();
                    return Err(e);
                }
            };
            if all_skills.is_empty() {
                return Err(Error::Msg("仓库中没有可安装的技能".into()));
            }
            println!("{}", ui::ok(format!("发现 {} 个技能", all_skills.len())));
```

- [ ] **步骤 2：add 的技能选择改用 SkillChoice，取消文案统一**

```rust
            let choices: Vec<wizard::SkillChoice> = all_skills
                .iter()
                .map(|e| wizard::SkillChoice {
                    name: e.name.clone(),
                    description: e.description.clone(),
                })
                .collect();
            // （交互分支）
            match wizard::pick_skills(&choices)? {
                Some(p) => p,
                None => {
                    println!("已取消安装");
                    return Ok(());
                }
            }
```

fav install 分支同样构造 choices（`fav.skills` 条目有 `name`/`description` 字段），其两处取消打印（pick_skills 返回 None、picked 为空）统一改为 `已取消安装`。

- [ ] **步骤 3：resolve_install_plan 加 layout 参数，接记忆 + 取消 + 非交互提示**

签名加首参 `layout: &Layout`，两个调用方（add、fav install）传入。函数体改动：

```rust
    let no_prompt = all || yes || !std::io::stdin().is_terminal();
    // 纯兜底路径（非 TTY 且未给 -y/--all）给用户一行引导，行为本身不变
    if !all && !yes && !std::io::stdin().is_terminal() {
        eprintln!("提示：非交互环境可使用 -y / -s / -a / -t / -g 参数控制安装行为");
    }
```

agent 交互分支：

```rust
    } else {
        let detected = agents::detect_installed();
        let remembered = state::load(layout).last_selected_agents;
        match wizard::pick_agents(&detected, &remembered)? {
            Some(ids) => {
                state::save_last_agents(layout, &ids);
                ids
            }
            None => return Ok((vec![], cfg.default_method)), // 取消
        }
    };
```

scope / method 分支适配 Option 返回：

```rust
    let scope = if global {
        Scope::Global
    } else if no_prompt {
        Scope::Project(std::env::current_dir()?)
    } else {
        match wizard::pick_scope(&agent_ids)? {
            Some(s) => s,
            None => return Ok((vec![], cfg.default_method)), // 取消
        }
    };
    let method = match method {
        Some(m) => resolve_method(Some(m), cfg),
        None if no_prompt => cfg.default_method,
        None => match wizard::pick_method()? {
            Some(m) => m,
            None => return Ok((vec![], cfg.default_method)), // 取消
        },
    };
```

- [ ] **步骤 4：计划行与取消文案**

两处（add、fav install）`将安装 {} 个技能 → {} 个目标（{method:?}）` 改为：

```rust
            println!(
                "将安装 {} 个技能 → {} 个目标（{}）",
                picked.len(),
                targets.len(),
                ui::method_label(method)
            );
```

两处 `未选择目标，取消安装` 改为 `已取消安装`。

- [ ] **步骤 5：install_loop 重写——spinner + 逐条染色 + inquire 覆盖确认 + 收尾摘要**

```rust
/// 「逐技能逐目标安装 + Conflict 确认/跳过 + 逐条落盘 + 收尾摘要」循环。
/// 逐条落盘的原因不变：中途失败时 registry 与磁盘已写入的副本保持一致。
/// install_fn 返回 Ok=装成，Err(Conflict)=走确认（yes=跳过），其余 Err 中断。
fn install_loop(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    picked: &[String],
    targets: &[Target],
    method: Method,
    yes: bool,
    install_fn: &dyn Fn(&mut Registry, &str, &Target) -> Result<()>,
) -> Result<()> {
    let total = picked.len() * targets.len();
    let pb = ui::spinner(format!("正在安装（共 {total} 条）…"));
    let mut installed = 0usize;
    let mut skipped: Vec<String> = Vec::new();
    for s in picked {
        for t in targets {
            let installed_now = match install_fn(reg, s, t) {
                Ok(()) => {
                    ui::progress_or_print(
                        &pb,
                        ui::ok(format!("已安装 {s} → {t}（{}）", ui::method_label(method))),
                    );
                    true
                }
                Err(Error::Conflict(p)) => {
                    if yes {
                        ui::progress_or_print(&pb, ui::skip(format!("跳过已存在：{}", p.display())));
                        skipped.push(format!("{s} → {t}"));
                        false
                    } else {
                        // spinner 挂起再弹窗，避免两套终端绘制互相糊屏
                        let prompt = format!("{} 已存在，覆盖？", p.display());
                        let overwrite = match pb.suspend(|| {
                            inquire::Confirm::new(&prompt).with_default(false).prompt()
                        }) {
                            Ok(v) => v,
                            // 覆盖确认的 ESC/Ctrl+C 视为不覆盖（跳过该条），不中断批次
                            Err(
                                inquire::error::InquireError::OperationCanceled
                                | inquire::error::InquireError::OperationInterrupted,
                            ) => false,
                            Err(e) => {
                                pb.finish_and_clear();
                                return Err(Error::Msg(e.to_string()));
                            }
                        };
                        if overwrite {
                            let rec = install::to_rec(t);
                            let _ = remove::remove_install(layout, cfg, reg, s, &rec);
                            install_fn(reg, s, t)?;
                            ui::progress_or_print(&pb, ui::ok(format!("已覆盖安装 {s} → {t}")));
                            true
                        } else {
                            skipped.push(format!("{s} → {t}"));
                            false
                        }
                    }
                }
                Err(e) => {
                    pb.finish_and_clear();
                    return Err(e);
                }
            };
            if installed_now {
                installed += 1;
                // 逐条落盘（tmp+rename 原子写，代价低）：之后任一失败提前返回时，
                // registry 都与磁盘已写入的副本保持一致，不留孤儿副本。
                reg.save(layout)?;
            }
        }
    }
    pb.finish_and_clear();
    println!(
        "共安装 {installed}/{total} 条（{} 个技能 × {} 个目标，{}）",
        picked.len(),
        targets.len(),
        ui::method_label(method)
    );
    if !skipped.is_empty() {
        println!("跳过 {} 条：", skipped.len());
        for s in &skipped {
            println!("  {}", ui::skip(s));
        }
    }
    Ok(())
}
```

（原 `#[allow(clippy::too_many_arguments)]` 注释块保留。）

- [ ] **步骤 6：update 的覆盖确认换 inquire（语义不变：默认否）**

`commands.rs:284` 区域的 `dialoguer::Confirm` 改为：

```rust
                    let confirmed = inquire::Confirm::new("将覆盖上述本地修改，继续？")
                        .with_default(false)
                        .prompt()
                        .map_err(|e| {
                            Error::Msg(format!(
                                "确认交互失败（{e}）；非交互环境请改用 skills update --force 明确覆盖"
                            ))
                        })?;
```

- [ ] **步骤 7：全量测试 + 静态检查**

运行：`cargo fmt && cargo clippy && cargo test`
预期：全绿（既有集成测试走非交互路径，断言不含本次改动的文案；`tests/e2e.rs:243` 断言的 `Symlink` 来自 list 的 `{:?}` 输出，本计划不改 list）。

- [ ] **步骤 8：Commit**

```bash
git add src/cli/commands.rs
git commit -m "add/fav install 接入 spinner、染色、完成摘要与 agent 选择记忆"
```

---

### 任务 6：TUI 向导迁移 inquire

**文件：**
- 修改：`src/tui/mod.rs`（约 39、92、101、115、145、160-181、195-239、245-279、301-325、326-354 行区域）
- 修改：`AGENTS.md`

注意：TUI 的安装循环**刻意不复用** `install_loop`——install_loop 逐条落盘，而 TUI 的不变量是「直接操作内存 registry，退出时才落盘，错误路径不落盘」（AGENTS.md）。本任务只换弹窗与文案，不加摘要块、不改落盘时机。

- [ ] **步骤 1：install_wizard（src/tui/mod.rs:144 起）改造**

- 顶部注释「向导需正常终端（dialoguer）」改为 inquire；39 行注释同样改。
- source 输入：`dialoguer::Input` → `inquire::Text::new("source（github:owner/repo、URL 或本地路径）").prompt().map_err(|e| Error::Msg(e.to_string()))?`。
- `pick_skills(&names)` → 构造 `Vec<wizard::SkillChoice>`（`all` 的 SkillEntry 有 `name`/`description`）后调用；None 分支打印 `已取消安装`。
- `pick_agents` 接记忆（与 commands.rs 同款）：

```rust
    let detected = agents::detect_installed();
    let remembered = crate::core::state::load(layout).last_selected_agents;
    let Some(agent_ids) = wizard::pick_agents(&detected, &remembered)? else {
        println!("已取消安装");
        return Ok(());
    };
    crate::core::state::save_last_agents(layout, &agent_ids);
```

- `pick_scope`/`pick_method` 适配 Option：None → `println!("已取消安装"); return Ok(());`。
- 计划行 `{method:?}` → `ui::method_label(method)`（顶部 `use crate::cli::ui;`）。
- 安装循环内的 `dialoguer::Confirm` → `inquire::Confirm::new(&format!("{} 已存在，覆盖？", p.display())).with_default(false).prompt()`，ESC/Ctrl+C 视为不覆盖（与 install_loop 同款 match）；`已安装/已覆盖安装/跳过` 三行用 `ui::ok`/`ui::skip` 染色，方法名用 `method_label`。

- [ ] **步骤 2：fav_wizard（:244 起）改造**

- `dialoguer::Input` → inquire::Text（同上）。
- 技能多选 → inquire MultiSelect（全部默认勾选用 `.with_all_selected_by_default()`；`min_length!(1, "至少选择一个技能")`；help message 同 wizard）。返回值是 `Vec<String>`（直接以 names 为选项）。取消（ESC/Ctrl+C）→ `println!("已取消收藏"); return Ok(());`——用 wizard 里同款判断（cancel_to_none 是 wizard 私有，可在 TUI 就地 match InquireError，或把它改为 `pub(crate)` 复用，选后者）。
- 「全选 = 整仓收藏（传空切片）」逻辑不变：`picked.len() == names.len()` 时传空。

- [ ] **步骤 3：fav_install_wizard（:282 起）与「按回车返回 TUI」改造**

- pick_skills/pick_agents/pick_scope/pick_method 同步骤 1 的改法（fav.skills 条目有 description）。
- 两处「按回车返回 TUI」（:101、:115）：`dialoguer::Input` → `let _ = inquire::Text::new("按回车返回 TUI").prompt();`。

- [ ] **步骤 4：确认 dialoguer 清零并从 Cargo.toml 移除（若任务 1 未删）**

运行：`grep -rn "dialoguer" src/ Cargo.toml`
预期：无输出。

- [ ] **步骤 5：更新 AGENTS.md 两处描述**

- 「add 常用参数」下方向导段落中「选 agent（多选，检测到的默认勾，未检测到预选 agents）」改为「选 agent（搜索式多选，检测到的置顶；预选 = 上次选择记忆 > 检测到的 > agents；恰好检测到 1 个时跳过该步不弹窗）」。
- 「状态文件」段落补充：`` `state.json`（version 1：UI 记忆，当前仅 last_selected_agents；损坏/缺失自动降级为空，写失败仅警告） ``。

- [ ] **步骤 6：全量测试 + Commit**

运行：`cargo fmt && cargo clippy && cargo test`
预期：全绿。

```bash
git add src/tui/mod.rs src/wizard.rs AGENTS.md Cargo.toml
git commit -m "TUI 向导迁移 inquire，清除 dialoguer 依赖"
```

---

### 任务 7：端到端验证 + 测试机工作流

按项目 AGENTS.md 的「测试机验证工作流」执行（功能改动必走）。

- [ ] **步骤 1：本地全量验证**

运行：`cargo fmt --check && cargo clippy && cargo test`
预期：全绿。

- [ ] **步骤 2：同步测试机并编译**

```bash
rsync -a --exclude-from=scripts/rsync-exclude.txt ./ test@192.168.188.210:/mnt/sda3/deploy/skills/tools/
ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && cargo build'
```

预期：编译成功。

- [ ] **步骤 3：测试机真实场景（非交互全链路 + 取消语义）**

```bash
ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && \
  H=$(mktemp -d) && export SKILLS_HOME=$H && B=target/debug/skills && \
  git init --bare /tmp/fixture.git && \
  W=$(mktemp -d) && mkdir -p $W/skills/demo && \
  printf -- "---\nname: demo\ndescription: 演示技能\n---\n# Demo\n" > $W/skills/demo/SKILL.md && \
  git -C $W init -q && git -C $W add -A && git -C $W -c user.email=t@t -c user.name=t commit -qm init && \
  git -C $W push -q /tmp/fixture.git master && \
  $B add file:///tmp/fixture.git -y && \
  $B list && \
  $B tag demo skill --source local/tmp/fixture smoke 2>/dev/null || true; \
  $B update --dry-run && \
  $B remove demo && \
  echo "--- state.json（-y 路径不应产生记忆）---" && \
  (cat $H/state.json 2>/dev/null || echo "（无 state.json，符合预期）")'
```

逐条确认：add -y 成功（输出含 `✓ 已安装`、`共安装 1/1 条`，管道下无 ANSI 色码）；list 有 demo；dry-run 正常；remove 成功；`-y` 路径**不写** state.json。
（tag 的 source key 以 list/registry 实际 key 为准，若解析失败不阻断主链路。）

取消语义冒烟（伪终端喂 ESC）：

```bash
ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && \
  export SKILLS_HOME=$(mktemp -d) && \
  printf "\033" | script -qec "target/debug/skills add file:///tmp/fixture.git" /dev/null; echo "exit=$?"'
```

预期：输出含 `已取消安装`，`exit=0`。若 `script` 喂键在该环境不可靠（inquire 收不到 ESC），记录现象，改由本地人工 `cargo run -- add <本地目录>` 过一遍四步弹窗（过滤、全选、单检测省略、ESC 取消）作为交互验收。

- [ ] **步骤 4：最终 Commit（若有遗留改动）**

```bash
git status --short || true
# 有改动则：git add -A && git commit -m "收尾"
```

**禁止 `git push`**（等用户明确指令）。

---

## 自检结果

- **规格覆盖：** 规格 6 节全部有对应任务——§1 inquire 迁移（任务 3/4/6）、§2 流程精简（任务 4 单检测省略）、§3 spinner+颜色（任务 2/5/6）、§4 摘要+取消语义（任务 5/6）、§5 选择记忆（任务 1/4/5/6）、§6 非交互提示（任务 5 步骤 3）。依赖选型（任务 1）。不做项（确认页/环境检测/审计卡片）计划中也未出现。
- **占位符：** 无 TODO/待定；所有代码步骤含完整代码块。
- **类型一致性：** `SkillChoice{name,description}`、`pick_skills(&[SkillChoice]) -> Result<Option<Vec<String>>>`、`pick_agents(&[&'static str], &[String]) -> Result<Option<Vec<String>>>`、`pick_scope -> Result<Option<Scope>>`、`pick_method -> Result<Option<Method>>`、`agent_defaults(&[&str], &[String]) -> Vec<String>`、`detected_first(&[&str]) -> Vec<&'static AgentSpec>`、`state::{load, save_last_agents}`、`ui::{spinner, progress_or_print, ok, skip, err, warn, method_label}` 在各任务间一致。
- **已知风险：** inquire 0.9 的 crossterm 版本可能与顶层 crossterm 0.28 并存（两个版本共存可编译，仅体积略增）；`min_length!` 宏的双参形态、`with_default` 的确切签名以编译器为准微调。
