# CLI 安装交互体验升级设计

日期：2026-09-07
状态：已确认（范围 = 交互 + 反馈全套；搜索式多选用 inquire；不做安装前确认页）

## 背景与目标

用户反馈 vercel-labs/skills（`npx skills`，TypeScript）的命令行安装交互明显优于本项目。调研结论（来源：`src/add.ts`、`src/prompts/search-multiselect.ts` 等）：

对方做得好的地方，按对我们的价值排序：

1. **搜索式多选**：打字即过滤、"Select All" 三态行、分组整组勾选、底部按键提示与已选摘要、高亮项显示描述。我们 78 个 agent 用 dialoguer MultiSelect 平铺，无搜索无分组。
2. **全程 spinner**：解析/克隆/发现/安装每步有始有终。我们 git 克隆全程静默（gix progress 全部 Discard），大仓库像卡死。
3. **按场景省略问题**：单技能不问、只检测到 1 个 agent 不问、都不支持 global 不问 scope、单目标目录不问安装方式。
4. **agent 选择记忆**：上次选择持久化，下次预选。
5. **颜色与符号体系**：成功/跳过/警告一眼可分；取消统一为退出码 0。
6. **完成摘要卡片**：安装后按分组列出结果。

目标：把 1-6 全做，对齐对方体验；明确不做环境检测自动 `-y`、安全审计卡片、upsell 提示（YAGNI）。

## 依赖选型

| 依赖 | 版本 | 用途 | 状态 |
|---|---|---|---|
| inquire | 0.9.4 | 全部交互弹窗（MultiSelect 自带打字过滤、分页、help message） | 新增，替换 dialoguer |
| indicatif | 0.18.6 | 阶段级 spinner | 新增 |
| console | 0.16.4 | 输出染色（indicatif 搭档） | 新增 |
| dialoguer | — | 现有交互库 | 移除（CLI 与 TUI 向导都共用 wizard.rs，一并替换） |

版本均为 2026-09-07 经 crates.io API 核实的最新稳定版。

## 设计

### 1. 交互组件层（wizard.rs 重写为 inquire）

- `pick_skills`：inquire MultiSelect + 打字过滤；保留现有"单技能不弹窗直接选中"；技能行显示 `name — description`；保留首行"全选"快捷能力（inquire 有内置全选键绑定，实施时核实 0.9 API，若无内置则保留首行快捷项）。
- `pick_agents`：MultiSelect + 打字过滤；列表排序改为**已检测到的 agent 置顶**（标注 `✓ 已检测`），每项 hint 显示目标目录（如 `→ ~/.claude/skills`）；预选优先级：**上次选择的记忆 > 本机检测到的 > "agents"**。
- `pick_scope` / `pick_method`：inquire Select，选项带说明文字（项目=跟随仓库提交 / 全局=跨项目可用；符号链接=单一来源易更新，推荐 / 独立副本=脱离缓存）。
- TUI 安装向导与 CLI 继续共用 wizard.rs，行为一致。

### 2. 流程精简

- 新增：交互流程中本机检测到**恰好 1 个** agent 时不弹窗，直接打印 `安装到：<Name> (已检测)` 并使用它（不影响 `-y`/`-a` 等显式路径）。
- 现有省略逻辑保持不变：单技能跳过、所选 agent 全不支持全局时跳过 scope。

### 3. 进度与颜色反馈

- CLI 层用 indicatif spinner 包住三个阶段：解析/克隆缓存、扫描技能、安装执行。文案风格：`正在克隆仓库…` → `✓ 发现 N 个技能`。spinner 只做阶段级不确定进度，**不改 core 接口**（gix progress 维持 Discard）。
- 所有面向用户的输出接 console 染色：成功绿 `✓`、跳过黄 `⊘`、失败红 `✗`、警告黄。
- method 输出从 `{:?}`（`Symlink`/`Copy`）改为用户文案（`符号链接`/`独立副本`）。

### 4. 完成摘要与取消语义

- **不做安装前确认页**（用户明确不要，保持 4 步流程）；覆盖冲突维持现有的逐个 Confirm。
- 安装结束后输出汇总块：`共安装 X/Y 个技能 → Z 个目标`，分组列出成功与跳过项；失败单列并标红。
- 取消统一化：任一向导步骤 Esc/Ctrl+C → 打印 `已取消安装`，退出码 0（取消不是错误，对齐 vercel 语义）。现有"空选=取消"行为并入同一出口。

### 5. agent 选择记忆

- 新增 `$SKILLS_HOME/state.json`：`{"version": 1, "last_selected_agents": ["claude-code", ...]}`。
  - 独立小文件，不动 registry v3 结构，不污染用户手写的 config.toml。
  - 读取时过滤已失效的 agent id；写失败不阻塞安装（仅警告）。
- 每次交互式确认 agent 选择后写入；`-a`/`-t`/`-y`/非交互路径不写（避免脚本污染记忆）。

### 6. 非交互路径

- 兜底行为保持现状（全技能 + agents + 项目 scope），不改为报错。
- 追加一行提示：`提示：非交互环境可使用 -y / -s / -a 等参数控制安装行为`。
- 现有 `-y`/`-t`/`-s '*'/--all` 语义一律不变，集成测试不受影响。

## 错误处理

- inquire 的 Esc/Ctrl+C 错误统一映射为"取消"出口（见第 4 节），不再向上抛 `Error::Msg`。
- state.json 读写失败降级为无记忆模式，仅警告不报错。
- 覆盖冲突 Confirm 维持无默认值（默认 No，防误覆盖）。

## 测试

- 向导的可测逻辑抽成纯函数并补单元测试：预选计算（记忆/检测/兜底优先级）、恰好 1 个检测 agent 的省略条件、目标展开去重、state.json 读写与失效 id 过滤。
- inquire 弹窗本身不可自动测；集成测试继续走非交互路径（`SKILLS_HOME` 隔离 + 本地 bare 仓库），现有测试保持全绿。
- 取消路径（空选）在集成测试中以非交互方式覆盖现有断言。

## 安全不变量

本设计只动 CLI 表现层与 wizard，不触碰 install/update/remove/git 的原子性、归属核验、本地修改保护等安全不变量（AGENTS.md 列出的 6 条）；spinner/颜色/摘要不改变任何磁盘操作的顺序与语义。
