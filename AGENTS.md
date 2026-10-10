# AGENTS.md

本项目的 agent 约定与开发指南（CLAUDE.md 是指向本文件的软链接，兼容 Claude Code）。通用约定见用户级 `~/.agents/AGENTS.md`，此处只写项目特有内容；冲突时以本文件为准。

## 项目是什么

`skills`：单二进制 Rust 技能包管理器（CLI + TUI + 本地 Web 三界面），管理含 `SKILL.md` 的 AI agent 技能包——下载缓存、多目标安装（symlink/copy）、安装记录、标签分类、两级自动更新。git 操作用 gix（gitoxide，纯 Rust，不依赖系统 git）。跨平台：Windows / macOS / Linux。

## 常用命令

```bash
cargo build                        # 构建
cargo test                         # 全部测试（单元 + 集成）
cargo test <name>                  # 单个测试（按名字匹配）
cargo test --test e2e              # 端到端测试（真实二进制 + 本地 bare 仓库，无网络）
cargo test --test cli_smoke        # CLI 冒烟测试
cargo clippy                       # lint
cargo fmt                          # 格式化（提交前必跑）
cargo run -- list                  # 本地运行 CLI（子命令：add/list/remove/update/tag/auto-update/config/fav/tui/ui）
# add 常用参数：-s <技能>（可多个，'*' 全选）-a/--agent <id>（可多个，'*' 全内置 agent）
#               --all（全技能+全 agent）-g（全局）-t <agent@global|agent@project:/abs> --method symlink|copy -y
bash scripts/pack-release.sh <target> <version>  # 本地验证 release 打包（产物在 dist/）
```

发版：改 `Cargo.toml` version → 提交 → `git tag vX.Y.Z && git push origin master --tags`，ci workflow（`ci.yml`，结构与 ssh-tunnel 一致：test 常驻、build/release 仅 tag 触发）自动跑 test → verify（tag 与 version 一致性）→ 6 平台 build → 发布。故障处置见 `RELEASE.md`。

## 架构

**三前端共享一个 core。** `src/main.rs` → `cli::commands::run` 分发；`src/core/` 承载全部业务逻辑，`src/tui/`（ratatui）与 `src/web/`（axum，绑 127.0.0.1，页面是 `src/web/static/index.html` 内嵌单文件）只是薄壳，所有磁盘/git/registry 操作都必须走 core，不在前端另写逻辑。

**核心概念**（详见 `docs/superpowers/specs/2026-08-20-skills-manager-design.md` 与 `docs/superpowers/specs/2026-09-05-add-wizard-redesign-design.md`）：

- **source（技能包）**：git 仓库或本地目录，缓存到 `$SKILLS_HOME`（默认 `~/.skills/`）下 `<host>/<owner>/<repo>/`（GitHub 归到 `github/`，本地路径归到 `local/`）。缓存按来源去重，已缓存则 `add` 复用、`update` 才刷新；git 缓存一律全克隆（完整历史），早期 depth=1 浅克隆的遗留缓存会在 update 时自动补全为完整历史。
- **skill**：含 `SKILL.md` 的目录，名字取 frontmatter 的 `name`；扫描只认根级或两层目录内、frontmatter 完整（name+description）的条目，非法条目跳过并警告。
- **target = agent × scope**（`src/core/paths.rs`）：agent 来自内置注册表（`src/core/agents.rs`，78 条，移植自 vercel-labs/skills 的 agents.ts 数据 + 自加 "agents" universal 条目）或 config.toml 自定义；scope 为 `global`（agent 全局目录）或 `project:<绝对路径>`（装到 `<root>/<agent.project_dir>`，多个 universal agent 共享 `.agents/skills`）。命令行语法 `agent@global` / `agent@project:<abs>`，兼容旧写法 `global:<agent>` / `project:<abs>`（后者 = agents + 项目根）；Display 输出新格式。`config.toml [targets]` 只做覆盖扩展：同名 id 覆盖内置全局路径，新 id = 自定义 agent（仅全局，无项目目录）。
- **install**：一条「技能 × 目标」记录，方式二选一——`symlink`（默认，指向缓存）或 `copy`（独立副本，根目录带 `.skills-manifest`：sha256 基线，既证明副本归属又用于本地修改检测）。

交互终端下裸 `add` 走四步向导（`src/wizard.rs`，CLI/TUI 共用）：选技能 → 选 agent（搜索式多选，检测到的置顶；预选 = 上次选择记忆 > 检测到的 > agents；恰好检测到 1 个时跳过该步不弹窗）→ 选 scope（项目/全局；所选 agent 均不支持全局时不问，直接项目）→ 选安装方式。显式参数不弹窗：`-s` 选技能（支持 `'*'` 全选）、`-a/--agent` 选 agent（支持 `'*'` 全内置）、`--all` = 全技能 + 全 agent、`-g` 全局、`-t` 直接给 target 列表、`--method` 指定方式；`-y` = 全技能 + 检测到的 agent（无检测 → agents）+ 项目 + 默认方式；非交互（管道/脚本）兜底 = 全技能 + agents + 项目。按 install_dir 去重：同目录的多个 agent 只装一份。空技能包（无可安装技能）先报错不弹窗。

**状态文件**（都在 `$SKILLS_HOME` 下）：`registry.json`（version 3：sources + installs + skill_tags，tmp+rename 原子写；install 的 target 记为 `{"scope":"global","agent":"..."}` / `{"scope":"project","agent":"...","root":"..."}`）、`config.toml`（可选，只做覆盖扩展；无配置文件也能工作）与 `state.json`（version 1：UI 记忆，当前仅 last_selected_agents；损坏/缺失自动降级为空，写失败仅警告）。**registry v3 不兼容旧版**：v3 起 target 改为 agent×scope 格式，不做迁移——version != 3 直接拒绝加载，提示备份后删除重建（旧记录无法用旧二进制之外的途径自动转换，按提示操作即可）。标签绑定 (source, skill)，与安装目标无关。

**两级更新策略**：包级 `sources[key].auto_update` 控制是否 fetch；copy 副本级 `auto_update` 可覆盖包级（`--inherit` 清除覆盖）；symlink 永远跟随包级。裸 `update` 按策略全量跑；`update <skill> --target <t>` 显式强制更新该副本（参数配对有严格校验，不静默吞参）。

## 必须守住的安全不变量

改动 install/update/remove/git 时不得破坏这些语义（都有对应测试锁定）：

1. **原子性**：copy 安装走 staging 目录 + rename 提交；更新走「暂存 → 旧副本 rename 备份 → 提交 → 删备份」，失败回滚，回滚也失败时错误必须带备份路径。git 侧 `fetch_and_reset` 的 worktree/index/ref 切换同样是事务式（`git.rs` 的 `CheckoutTransaction`），任一阶段失败完整回滚并校验恢复结果。
2. **归属核验**：remove/update 只动带 `.skills-manifest` 标识的 copy 副本；实况与记录不符（不是目录、是链接、指向不对、记录损坏）返回 `Error::Mismatch` 并保留现场，绝不误删用户数据。
3. **本地修改保护**：非 force 的 update 先做全量预扫描，任何 copy 副本偏离 manifest 基线（内容被改/文件缺失/基线外新增）即整体 `Mismatch` 中止，不产生部分变更；用户确认后以 force 重跑并重写基线。
4. **registry 与磁盘一致**：CLI add 逐条落盘（每装成一个副本就 save 一次），中途失败不留 list/remove 管不到的孤儿副本。
5. **config 分支先于 `Config::load` 分发**（`commands.rs` 顶部注释）：配置损坏时 `config set` 是唯一自愈出口；`config set` 对已知标量键写入前校验，且原配置可解析时写入后也必须可解析，防止写出锁死全部 CLI 的值。
6. **路径安全**：技能名必须是单一 Normal 路径组件；source_path 解析后必须仍在缓存根内（canonicalize 校验，防 symlink 逃逸）；manifest 基线条目拒绝 `..` 等越界组件。

## 测试约定

- 单元测试就地 `#[cfg(test)]`；集成测试在 `tests/`，用 `assert_cmd` 跑真实二进制，**靠 `SKILLS_HOME` 环境变量指向临时目录隔离**，fixture 用本地 bare git 仓库（`file://` URL），全程不联网。
- 失败注入是既有模式：`install.rs` 用线程本地计数/掩码注入 rename、备份清理失败；`git.rs` 用 `FailurePoint` 枚举注入 checkout 各阶段失败。新增原子操作沿用此模式写回归测试。
- TUI 的安装向导直接操作内存中的 `app.registry`（用户在 TUI 里的未落盘修改不能被磁盘副本覆盖），退出时才落盘；错误路径有意跳过落盘。
- CI（`ci.yml`）在 ubuntu/macos/windows 三平台跑 `cargo test`（带 rust-cache），提交前本地至少保证 `cargo test` 全绿。

## 仓库惯例

- 代码注释、commit message、文档一律中文；代码与命名用英文。
- `docs/superpowers/specs|plans/` 是设计与实现计划文档；`docs/task-reports/task-*.md` 是子代理开发的任务报告（历史记录，非文档入口）。
- `.agents/skills/` 是本仓库通过本工具自安装的技能集（dogfooding），`.codex/skills` 是指向它的符号链接，不是源码。

## 测试机验证工作流（功能新增 / bug 修复必走）

每次功能新增或 bug 修复，本地 `cargo test` 通过后，**自动**执行以下流程，不需要用户另行指示（本规则覆盖全局「不主动部署测试服务器」约定，仅限本项目）：

1. **同步代码**：rsync 到测试机。**禁止 `--delete`**（远端残留文件只多不少，宁可手动清理也不冒误删风险）；需要排除的路径一律写进 `scripts/rsync-exclude.txt`，用 `--exclude-from` 引用，不把排除规则散落在命令行里：

   ```bash
   rsync -a --exclude-from=scripts/rsync-exclude.txt ./ test@192.168.188.210:/mnt/sda3/deploy/skills/tools/
   ```

2. **测试机编译**：`ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && cargo build'`（非交互 ssh 不会加载 ~/.profile，必须显式 export PATH）

3. **真实场景测试**：在测试机上用真实二进制跑完整用户场景（配合 `SKILLS_HOME` 指向临时目录隔离），例如 `add → list → tag → update --dry-run → remove` 全链路；只跑单元测试不算完成。

4. **自动提交**：以上全部通过后自动 `git commit`（中文 message，说明变更意图）。**禁止自动 `git push`**——push 只用于跨设备同步，等用户明确指令。

测试机环境（2026-09-05 已配置）：rustup 默认 profile，stable 1.98.1（含 rustfmt/clippy），crates.io 走 rsproxy 镜像（`~/.cargo/config.toml`）。
