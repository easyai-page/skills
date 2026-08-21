# 任务 11 报告：CLI（clap）命令树与全部子命令

**提交：** `b713df3` feat(cli): 完整命令树与子命令实现（分支 feat/skills-manager，基于 9c2396a）

## 交付物

| 文件 | 内容 |
|---|---|
| `src/cli/mod.rs` | clap derive 命令树：add / list（alias ls）/ remove / update / tag / auto-update / config(get/set/targets add|remove) / tui / ui |
| `src/cli/commands.rs` | 各子命令薄壳实现，逻辑全部委派 core 层 |
| `src/main.rs` | 接线：parse → `cli::commands::run`，出错 eprintln + exit 1 |
| `src/tui/mod.rs`、`src/web/mod.rs` | 最小占位（任务 12/13 填充） |
| `src/core/paths.rs` | `Layout::new` 先读 `SKILLS_HOME` 环境变量（测试隔离/自定义数据目录出口） |
| `tests/cli_smoke.rs` | smoke 测试 2 个（help 列全部子命令、空 layout 下 list 成功） |
| `Cargo.toml` | dev-dependencies 追加 `assert_cmd = "2"` |

## TDD 过程

1. 步骤 1：写 smoke 测试 + SKILLS_HOME 改造，`cargo test --test cli_smoke` → **红**（help 缺少 add）。
2. 步骤 3-4：实现命令树与 commands.rs → `cargo test --test cli_smoke` → **绿**。
3. 步骤 5：`cargo fmt` + `cargo test` → 77 单测 + 2 smoke 全 PASS，`cargo fmt --check` 干净。

## 设计约束落实

- `config` 子命令只读写 config.toml（toml::Value 整体重写）；`auto-update`/`tag` 只写 registry.json，互不混用。
- 目标语法经 `Target::parse` 严格校验 global:<name> / project:<绝对路径>。
- 安装默认 `cfg.default_method`（Symlink），`--method copy` 切换。
- 显式 `update <skill> --target <t>` 构造 `Selection` 强制更新该副本（手动验证输出「更新（显式指定）」）。
- 人类可读输出走 println/stdout；core 层 eprintln 警告未动。
- docs/ 与 .superpowers/ 未触碰。

## 对简报代码的两处必要修正（简报原样无法编译/有缺陷）

1. **`toml::Value` 无 `.entry()` 方法**：简报 `run_config` 的 Set/Targets 写法不可编译，改为 `as_table_mut().entry(String)` 显式建表，语义不变。
2. **`config set` 一律写 String 会锁死 CLI**（手动 smoke 实测）：`config set web.port 9000` 写成 `"9000"` 后，`Config::load` 反序列化 u16 失败，之后所有命令（含 config get 自己）全部报错。新增 `scalar()` 标量推断：纯数字 → Integer、true/false → Boolean、其余 String。字符串值行为与简报完全一致。
3. `save` 闭包补 `create_dir_all(parent)`：首次写入时 SKILLS_HOME 目录尚不存在会 io 报错（与 `Registry::save` 行为对齐）。
4. `Update` 的 `all` 标志简报实现中未使用，模式绑定改为 `all: _` 消除警告（命令面保留）。

## 手动端到端验证（SKILLS_HOME 隔离）

- 生命周期：add（默认 symlink）→ list → tag → list --tag → auto-update --on（symlink 副本打印跟随提示）→ update --dry-run → remove → 磁盘与记录同步清空 ✓
- `add --method copy` 产生真实副本；重复 add `-y` 走 Conflict 分支输出「跳过已存在」✓
- `ls` 别名、`--global` 过滤 ✓
- config：set/get web.port（数字）、defaults.method、targets add/get/remove 回环 ✓
- 参数缺失报错：`auto-update` 无参 → 「需指定 --source <包> 或 <技能> + --target」，exit 1 ✓

## 测试小结

`cargo test`：77 passed（core 单测）+ 2 passed（cli_smoke）= **79/79 PASS**；新增代码无编译警告（既有 core 警告 5 条为任务 1-10 遗留，未动）。

## 遗留说明

- `tui`/`web` 为占位实现，等任务 12/13。
- `update --all` 目前与无参 update 等价（build_plan 全量策略），标志保留在命令面。
- `task-10-report.md` 为任务 10 遗留未跟踪文件，本任务未提交它。

---

# 任务 11 第 1 轮修复报告

**提交：** `6848064` fix(cli): 配置损坏自愈、-g 真实语义、update 参数配对、auto-update 三选一（基于 b713df3）

## 审查发现处理

### 1. 关键：Config::load 失败锁死全部命令（commands.rs:16+288-297）

- **(a) 提前分发**：`run()` 中 Config 分支改为在 `Config::load` **之前**匹配（`if let Some(Cmd::Config { sub }) = &cli.cmd` 提前 return），`run_config` 只读写 config.toml 原文（toml::Value），不依赖反序列化；签名改为 `&ConfigCmd`。配置损坏时 `config get/set` 仍是自愈出口。
- **(b) 写入前校验**：新增 `validate_config_set` 对已知标量键做类型/范围校验——`web.port` 限 u16（拒绝 99999/-1/abc）、`defaults.method` 限 symlink|copy。另有整份配置解析兜底：新增 `core::config::validate_config_toml`，原配置可解析时写入后也必须可解析（兜住 `config set targets.x 123` 这类未知键破坏）；原配置已损坏时是自愈场景，只校验本次写入的键，允许逐个键修回。注：config.toml 无 auto_update 键（那是 registry 层），审查举例不适用。
- **回归测试**：`config_set_rejects_out_of_range_values_before_writing`（越界值拒绝且不落盘）、`config_subcommand_survives_and_heals_corrupt_config`（双键损坏下 config 子命令可用 + 逐键自愈恢复 list）。

### 2. 重要：add 的 -g 死标志且硬编码 agents（commands.rs:85-89）

真实语义（npm 风格，与 list -g 的"只看全局"一致）：
- `-g` 且无显式 `--target`：装进配置里**第一个可用 global target**（BTreeMap 按名排序，内置默认下即 agents，与简报"等价 global:agents"兼容）；
- 无 `-g` 无 `--target`：默认装进**当前项目** `<cwd>/.agents/skills`；
- `cfg.targets` 为空（删光 target）：明确报错「没有可用的全局 target：请先 skills config targets add」。注：当前 `Config::default` 在有 HOME 时总内置 agents/claude/codex，故该分支是防御性兜底，经 `default_global_target` 单测覆盖。
- 测试：`add_global_flag_uses_first_configured_target_and_bare_add_uses_project`（-g 装进重定向后的 agents、bare add 装进 cwd 项目）+ commands.rs 内 2 个单测。

### 3. 重要：update 静默吞参（commands.rs:200-208）

- skill 与 `--target` 必须同时给出，否则报错（`update alpha` 无 target / `update --target t` 无 skill 均 exit 1）；
- 多 skill + `--target`：**逐个构造 Selection 并合并 Plan**（sources 排序去重；单 skill 即特例），选此项因 core `Selection`/`build_plan` 签名不变、无需动任务 10 模块；
- 显式指定未安装的技能：dry-run 也返回 NotInstalled（此前仅 execute 报错）；
- `--all` 接线为显式全量更新，与技能名/`--target` 互斥报错。
- 测试：`update_requires_skill_and_target_together`、`update_multi_skill_with_target_selects_each`（多 skill dry-run 两条「显式指定」+ ghost 明确报错）。

### 4. 重要：auto-update 三选一（commands.rs:251-276）

- mod.rs：`AutoUpdate` 变体加 `#[command(group = ArgGroup::new("policy").required(true).multiple(false).args(["on","off","inherit"]))]`，clap 层强制三选一，裸调直接 usage 错误（exit 2），不会静默清设置；
- commands.rs：`inst.auto_update = val` 直接赋值（组保证 else 即 inherit）。
- 测试：`auto_update_requires_exactly_one_policy_flag`（裸调/--on+--off 被拒且 registry 字节不变；on→inherit 往返）。

## 次要项

- **-y 死标志**：remove 的 `-y` 从未接线（删除本就不交互），按"移除或接线"选择**移除**（add 的 -y 在 Conflict 分支真实使用，保留）。
- **--all**：见上，已接线为显式全量 + 互斥校验。
- **remove --tag 重复项 dedup**：待删集合按 (skill, target) 去重，--tag 命中与显式 skill 重叠时只删一次。
- **commands.rs:130 吞 remove 错误**：单项失败 eprintln 并计数，正常 save 后非零退出（`N 项删除失败`），不再静默 exit 0。
- **SKILLS_HOME 空串防御**：`Layout::new` 空串视为未设置（否则 root="" 会写 CWD），paths.rs 单测覆盖。
- **cli_smoke.rs:22 恒真断言**：help 检查改为按行匹配行首 token（`split_whitespace().next() == Some(cmd)`）。

## 测试小结

`cargo fmt --check` 干净；`cargo test` **92/92 PASS**（单测 82 = 77+5 新增：config validate 1、paths env 1、cli::commands 3；cli_smoke 10 = 2+8 新增）；`cargo test cli` 3 passed。编译警告维持任务 1-10 遗留 5 条，未新增。docs/ 与 .superpowers/ 未触碰；`task-10-report.md` 仍未跟踪未提交（任务 10 遗留）。
