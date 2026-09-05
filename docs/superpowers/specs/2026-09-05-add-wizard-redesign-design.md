# add 安装向导重设计：对齐 vercel-labs/skills 交互流程

日期：2026-09-05
状态：已确认（用户已批准方向与关键决策）

## 背景与目标

当前裸 `add` 的交互只有两步（技能多选 → target 多选），target 模型把「装到哪个 agent」和「项目/全局」揉在一个字符串里，且内置 target 只有 agents/claude/codex 三个。

参考 [vercel-labs/skills](https://github.com/vercel-labs/skills) 的交互流程（已从源码 `src/add.ts` 确认），把安装向导重设计为固定四步，并内置一份齐全的 agent 注册表。**只参考交互过程**：安装机制（symlink/copy、`.skills-manifest`、原子性、更新策略、安全不变量）全部保持现状。

### 已确认的决策

1. agent 支持做到「完整对齐」：内置 agent 注册表（项目路径 + 全局路径 + 本机检测），agent 类型参照参考项目（约 70 个）。
2. 项目级安装也按 agent 分目录（Claude → `./.claude/skills/`、universal 一批共享 `./.agents/skills/`）。
3. registry.json **不做向后兼容**，直接换新格式。
4. CLI / TUI / Web 三前端共用同一套安装逻辑。

## 交互流程（裸 `add`，TTY 下无 flag 时）

固定四步，顺序与参考项目一致：

1. **选技能**（多选）：第一行是「全选 (n/m)」快捷行（光标在其上按空格切换全选/全清），默认全不选，空选 = 取消。跳过条件：单技能包自动选中；`-s` 已指定；`-s '*'` 表示全选。
2. **选 agent**（多选）：先检测本机已装 agent（按各 agent 的配置目录是否存在判断，平行参考项目 `detectInstalled`），检测到的默认勾选；一个都没检测到时展示全量列表、预选 universal（`agents`）。跳过条件：`-a` 已指定；`-a '*'` 表示全选。
3. **选 scope**（单选）：项目（默认，装当前目录）/ 全局。所选 agent 全都不支持全局时跳过。`-g` 跳过（= 全局）。
4. **选安装方式**（单选）：symlink（默认，推荐）/ copy。`--method` 跳过。

四步完成后打印一行安装摘要（技能数 × 目标数、方式）直接开装；目标已存在的冲突仍走现有逐条 `Confirm`（`-y` 跳过确认、跳过已存在项）。

### 非交互（管道/脚本）兜底

不带 flag 且非 TTY：技能 = 全部，agent = universal，scope = 当前项目，method = 默认。不弹任何窗。（顺带修掉现状的不对称：管道下裸 add 不带 `-s` 会因 dialoguer 报错。）

### 明确不做（YAGNI）

- 参考项目的 universal 锁定组（恒选中不可取消）——我们只做预选。
- 上次选择的记忆持久化。
- 最终「确认安装」弹窗——摘要打印后即装，冲突确认已存在。
- 安全审计、telemetry、find-skills 推荐等参考项目的附加步骤。

## agent 注册表（新增 `src/core/agents.rs`）

静态表从参考项目 `src/agents.ts` 移植（约 70 个），每个 agent：

```rust
pub struct AgentSpec {
    pub id: &'static str,              // "claude-code"、"codex"、"agents"(universal) …
    pub name: &'static str,            // 显示名
    pub project_dir: &'static str,     // 项目内相对路径，如 ".claude/skills"
    pub global_dir: Option<&'static str>, // 全局路径（含 ~）；None = 不支持全局（如 eve）
    pub detect_dirs: &'static [&'static str], // 本机检测用的配置目录（~ 开头）
    pub universal: bool,               // project_dir == ".agents/skills"
}
```

- **universal**：project_dir 为 `.agents/skills` 的一批 agent 共享同一目录；同一 (技能, 项目) 选中多个 universal agent 时去重，只装一份。内置 `agents` 即代表 universal 共享目录本身。
- **检测**：`detect_dirs` 任一存在（`~` 展开后）即视为已安装。
- **config.toml 的 `[targets]` 保留**，语义改为：key 与内置 agent id 同名 → 覆盖其全局路径；新名字 → 自定义 agent（仅全局路径，无项目路径、无检测）。
- `config targets add/remove` 子命令语义不变，操作的就是这张覆盖表。

## Target 模型重构（核心改动）

现状（`src/core/paths.rs` / `registry.rs`）：

```rust
enum Target { Global { name }, Project { root } }        // 运行时
enum TargetRec { Global { name }, Project { root } }     // 持久化（serde tag = "kind"）
```

项目级固定 `<root>/.agents/skills`，全局级查 `cfg.targets[name]`。

新模型：**target = agent × scope**。

```rust
pub enum Scope { Project(PathBuf /* 项目根 */), Global }

pub struct Target {
    pub agent: String,   // agent id（内置或 config 自定义）
    pub scope: Scope,
}
```

安装目录解析：

- `Project(root)` → `<root>/<agent.project_dir>`；agent 无项目路径（自定义 agent）时报错。
- `Global` → agent 全局路径（`[targets]` 覆盖优先，其次内置表）；agent 不支持全局时报错。

### 持久化格式（registry.json v3）

```json
"target": { "agent": "claude-code", "scope": "global" }
"target": { "agent": "codex", "scope": "project", "root": "/abs/path" }
```

- `Registry.version` bump 到 3。
- **不做兼容**：加载时 `version < 3` 或 target 反序列化失败 → 报错，提示用户备份后删除 `$SKILLS_HOME/registry.json` 重建。与现状「config 损坏报错 + 自愈出口」的哲学一致；`remove`/`update` 对磁盘上遗留的旧副本因记录缺失会自然成为孤儿，由用户手动清理（旧副本都带 `.skills-manifest` 或是指向缓存的 symlink，容易识别）。

### CLI flag 变化

| flag | 语义 |
|---|---|
| `-s, --skill <名...>` | 不变；新增支持 `'*'` 全选 |
| `-a, --agent <id...>` | 新增；`'*'` = 全部 agent |
| `-t, --target <...>` | 保留兼容：`global:<id>` / `project:<abs>`（= universal agent + 项目根） |
| `-g, --global` | scope = 全局（原来等价第一个 global target，现等价「全局 scope」，agent 由 `-a` 或交互决定） |
| `--method <symlink\|copy>` | 不变 |
| `-y, --yes` | 跳过全部交互：全技能 + 检测到的 agent（检测不到则 universal）+ 项目 scope + 默认 method |
| `--all` | 新增；= 全技能 + 全 agent + `-y` |

## 三前端统一

交互步骤的**数据与决策**收敛到 core，前端只负责呈现：

- core 新增：`agents::all()` / `agents::detect_installed()` / `agents::resolve(id)`；`scan_skills` 已有。
- **CLI**（`src/cli/commands.rs`）：四步 dialoguer 向导收敛为一个 `add_wizard` 函数；技能选择的「全选快捷行」用 dialoguer MultiSelect 的第一项实现（选中它 = 全选）。
- **TUI**（`src/tui/mod.rs`）：`install_wizard` / `fav_install_wizard` 改为同样四步，删掉与 CLI 重复的 `pick_targets`；仍直接操作内存 `app.registry`、退出时落盘的约定不变。
- **Web**（`src/web/api.rs` + `static/index.html`）：无弹窗，全参数驱动。新增 `GET /api/agents`（返回注册表 + 每项 detected 标记 + 是否支持 global）；install/fav-install 接口的 `targets` 字段换用新 `TargetRec`；前端页面加 agent 多选与 scope 选择 UI。

## 影响面

- `src/core/paths.rs`：Target/Scope 重定义、install_dir 解析、路径安全校验（canonicalize 仍在缓存根内等）不变量保持。
- `src/core/agents.rs`：新增。
- `src/core/config.rs`：`[targets]` 覆盖语义；内置默认 agents/claude/codex 融入 agent 注册表。
- `src/core/registry.rs`：TargetRec 换格式、version 3、拒绝旧版。
- `src/cli/commands.rs`：add 向导重写；remove/update/list/auto-update 的 target 展示与解析适配；`config targets` 不变。
- `src/tui/mod.rs`、`src/web/api.rs`、`src/web/static/index.html`：见上。
- 测试：单元测试就地更新；`tests/e2e.rs` / `cli_smoke.rs` 的 fixture 与断言适配新 target 格式；新增——四步向导的非交互兜底、agent 检测、universal 去重、`-a '*'`、旧版 registry 拒绝加载。
- `AGENTS.md`：更新 target 模型与交互流程描述。

## 安全不变量（保持不变）

install/update/remove/git 的六条不变量（原子性、归属核验、本地修改保护、registry 与磁盘一致、config 自愈、路径安全）全部保持。本次改动只改变「目标路径怎么算出来」，不改变安装/更新/删除的事务语义。新增注意点：agent id 同样必须是合法标识（来自内置表或 config key），项目路径拼接后仍受「技能名单一 Normal 组件、缓存根 canonicalize」约束。

## 测试策略

- 单元：agents 表完整性（id 唯一、universal 标记与 project_dir 一致）、Target 解析、registry v3 读写与旧版拒绝。
- 集成（e2e，本地 bare 仓库、SKILLS_HOME 隔离）：`add -s x -a claude-code` → `./.claude/skills`；`-a` 多个 universal agent 去重；`-g -a codex` → 全局 codex 目录；非 TTY 裸 add → 当前项目 `.agents/skills`；`-y`、`--all` 各一条链路；remove/update 全链路回归。
