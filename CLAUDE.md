# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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
bash scripts/pack-release.sh <target> <version>  # 本地验证 release 打包（产物在 dist/）
```

发版：改 `Cargo.toml` version → 提交 → `git tag vX.Y.Z && git push origin master --tags`，release workflow 自动跑 verify（tag 与 version 一致性）→ 6 平台 build → 发布。故障处置见 `RELEASE.md`。

## 架构

**三前端共享一个 core。** `src/main.rs` → `cli::commands::run` 分发；`src/core/` 承载全部业务逻辑，`src/tui/`（ratatui）与 `src/web/`（axum，绑 127.0.0.1，页面是 `src/web/static/index.html` 内嵌单文件）只是薄壳，所有磁盘/git/registry 操作都必须走 core，不在前端另写逻辑。

**核心概念**（详见 `docs/superpowers/specs/2026-08-20-skills-manager-design.md`）：

- **source（技能包）**：git 仓库或本地目录，缓存到 `$SKILLS_HOME`（默认 `~/.skills/`）下 `<host>/<owner>/<repo>/`（GitHub 归到 `github/`，本地路径归到 `local/`）。缓存按来源去重，已缓存则 `add` 复用、`update` 才刷新；git 缓存一律 depth=1 浅克隆。
- **skill**：含 `SKILL.md` 的目录，名字取 frontmatter 的 `name`；扫描只认根级或两层目录内、frontmatter 完整（name+description）的条目，非法条目跳过并警告。
- **target**：`global:<name>`（路径由 config.toml 的 `[targets]` 解析，内置 agents/claude/codex）或 `project:<绝对路径>`（固定装到 `<root>/.agents/skills`）。裸 `add` 默认装当前项目，`-g` 才装全局。
- **install**：一条「技能 × 目标」记录，方式二选一——`symlink`（默认，指向缓存）或 `copy`（独立副本，根目录带 `.skills-manifest`：sha256 基线，既证明副本归属又用于本地修改检测）。

**状态文件**（都在 `$SKILLS_HOME` 下）：`registry.json`（sources + installs，tmp+rename 原子写）与 `config.toml`（可选，只做覆盖扩展；无配置文件也能工作）。

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
- CI（`ci.yml`）在 ubuntu/macos/windows 三平台跑 `cargo test`，提交前本地至少保证 `cargo test` 全绿。

## 仓库惯例

- 代码注释、commit message、文档一律中文；代码与命名用英文。
- `docs/superpowers/specs|plans/` 是设计与实现计划文档；`docs/task-reports/task-*.md` 是子代理开发的任务报告（历史记录，非文档入口）。
- `.agents/skills/` 是本仓库通过本工具自安装的技能集（dogfooding），`.codex/skills` 是指向它的符号链接，不是源码。
