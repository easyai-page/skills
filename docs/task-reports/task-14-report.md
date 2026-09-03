# 任务 14 报告：端到端集成测试 + 三平台 CI

## 实现内容

1. **`tests/e2e.rs`**（新建）：两个端到端测试，通过 `assert_cmd` 驱动真实编译出的
   `skills` 二进制，fixture 为系统 git CLI 造的本地 bare 仓库，全程不依赖网络，
   `SKILLS_HOME` 指向临时目录隔离：
   - `add_list_remove_copy_flow`：add alpha（copy 到 global:agents）→ 二次 add 同仓库
     断言 stdout 含「已缓存」（复用缓存不重复下载）→ list 含 alpha/beta → remove alpha
     后磁盘副本删除、beta 保留。
   - `update_respects_two_level_policy`：add alpha+beta → 包级 `auto-update --on`、
     alpha 副本级 `--off` → 上游推新提交 → 裸 `update` 跳过 alpha（断言磁盘内容仍是
     v1）→ 显式 `update alpha -t global:agents` 强制更新（断言内容变为 v2）。
     source key 不硬编码，从 `registry.json` 的 `sources` 第一个键读取。
2. **`.github/workflows/ci.yml`**（新建）：与 brief 一致 —— push/PR 触发，
   ubuntu/macos/windows 矩阵，`dtolnay/rust-toolchain@stable`，`cargo test`。
3. **存量 clippy 警告清理**（验收要求 `cargo clippy -- -D warnings` 通过）：
   实际有 16 条（brief 预估约 5 条），逐条最小化修复，见下「文件变更」。

## 与 brief 的偏差及原因

| 偏差 | 原因 |
|---|---|
| add 调用显式加 `-t global:agents` | brief 省略了 `-t` 并假设默认装 global:agents；实际 CLI 裸 add（无 `-t`/`-g`）默认装进当前项目 `<cwd>/.agents/skills`（`src/cli/commands.rs:100`，`tests/cli_smoke.rs` 已锁定该语义）。不加 `-t` 断言必败 |
| `remove alpha` 去掉 `-y` | `remove` 子命令没有 `-y/--yes` 旗标（`src/cli/mod.rs` 的 `Cmd::Remove` 只有 skills/target/tag），且其实现本就不交互、无确认提示；带 `-y` 会被 clap 直接拒绝 |
| Cow 修复用 `name.clone().into_owned()` | clippy 的首选建议 `name.into_owned()` 不能编译（`name` 在共享引用后，E0507），采用 rustc 建议的等价形式 |
| 抽出 `redirect_agents_target` 辅助函数 | 两个测试都要写同一份 `[targets]` 配置（含 Windows 反斜杠转义），DRY |

警告清理明细（全部最小化、不改变行为）：

- `tags.rs`：`Method` 移入 test 模块导入（仅测试用到）
- `cache.rs`：`SkillEntry.description` 加 `#[allow(dead_code)]` 并注释（frontmatter 解析
  字段，测试锁定解析正确性，展示层暂未消费）；折叠一处嵌套 if 为 let-chain
- `error.rs`：删除 `AlreadyCached` 变体 —— 全库无任何构造点（缓存复用走
  `fresh: false` + 提示而非报错），删除零行为变化
- `git.rs`：`checkout_tree` 加 `#[cfg(test)]`（仅失败注入回归测试用，与既有
  `checkout_tree_with_failure` 同模式）；`FailurePoint` 变体去掉公共前缀 `After`
  （6 处引用同步改名）；rollback 折叠嵌套 if
- `paths.rs`：`Layout::at` 加 `#[cfg(test)]`（已逐一核实全部 14 个调用点都在
  `#[cfg(test)]` 模块内）；折叠 SKILLS_HOME 探测的嵌套 if
- `config.rs`：折叠 3 处嵌套 if（let-chain，edition 2024）
- `remove.rs`：折叠 project root 校验的嵌套 if
- `install.rs`：`install_skill` 9 参数加 `#[allow(clippy::too_many_arguments)]` 并注释
  理由（参数即一次安装的完整上下文，打包结构体不降低复杂度）
- `tui/ui.rs`：`iter().map(|t| *t).collect()` → `to_vec()`
- `update.rs`（测试内）：`assert_eq!(x, true)` → `assert!(x)`

## 验证

- `cargo test --test e2e`：2 passed（一次通过；fixture 是新建文件，无「先红后绿」
  可比基线 —— 本任务即测试任务，TDD 证据 N/A）
- `cargo test`：93（unit）+ 10（cli_smoke）+ 2（e2e）= 105 passed, 0 failed
- `cargo clippy -- -D warnings`：退出 0（修复前 16 warnings）
- `cargo clippy --all-targets -- -D warnings`：退出 0（比验收更严的一档也通过）
- `cargo fmt --check`：clean（`cargo fmt` 顺带收敛了变体改名后变短的行）

## 文件变更

- 新增：`tests/e2e.rs`、`.github/workflows/ci.yml`
- 修改（仅警告清理）：`src/core/{tags,cache,error,git,paths,config,remove,install,update}.rs`、
  `src/tui/ui.rs`
- 未触碰：`docs/`、`.superpowers/`、生产行为（除警告清理外）

## 自审

- 两个 e2e 测试均走真实二进制、真实 git 仓库、真实文件系统断言，无 mock ✓
- 全量测试、clippy（两档）、fmt 全绿 ✓
- 无范围蔓延：只动了测试 + CI + 警告清理 ✓

## 顾虑

1. **Windows CI 的 file:// URL**：测试与既有 core 测试一样用 `format!("file://{}",
   bare.display())` 拼 URL，Windows 上 `display()` 含反斜杠。gix 是否接受需 CI 实测；
   若失败，core 的 git.rs 测试会一同失败，属既有模式而非本任务新引入。本机（Linux）
   无法预验。
2. `SkillEntry.description` 保留 + allow 而非删除：字段承载 SKILL.md frontmatter
   解析结果且有测试锁定，判断为「展示层尚未消费」而非死代码；若维护者认为应删，
   一行可去。
