# Task 4 报告：CLI fav 子命令（收藏 / 两级列表 / 删除）

提交：`6c2bba2 feat: CLI fav 子命令（收藏/两级列表/删除）`（分支 feat/skill-favorites，未 push）

## 实现内容

按 brief 逐字落地，TDD 流程（RED → GREEN → 重构）：

1. **`src/cli/mod.rs`**
   - `Cmd` 枚举在 `Ui` 之前插入 `Fav { source, skill, sub }`，带 `#[command(args_conflicts_with_subcommands = true)]`，使 `rm`/`install` 优先按子命令解析。
   - 文件末尾追加 `FavSub` 枚举：`Rm { source, skill }`；`Install { source, skill, target, global, method, yes }`（结构本任务建好，分发留给 Task 5）。
2. **`src/cli/commands.rs`**
   - use 区并入 `favorites` 与 `FavSub`。
   - `run` 的 match 在 `Config => unreachable!` 之前插入 Fav 分支：`Rm`（整包删 → "已删除收藏 {key}（{n} 个技能）"；指定技能 → "已从 {key} 删除 {n} 个技能收藏"）、`Install` → 占位 `Err(Error::Msg("fav install 尚未实现（下一任务）"))`（计划强制）、`(None, Some(source))` → bookmark + "已收藏 {key}（{n} 个技能）"、`(None, None)` → `--skill` 无 source 时报 "--skill 需搭配 source：skills fav <仓库> --skill <名>"，否则 `print_favorites`。
   - 新增私有 `print_favorites(reg)`：空收藏打印 "（无收藏）"；一级行 `{key}    ({commit7}, 收藏于 {date10})` 或 `{key}    (本地源)`；单技能仓库用途挂一级行；多技能仓库二级 `  ├─/└─ {name} — {description}`。日期/commit 用 `chars().take()` 按字符截断，无多字节切片风险。
3. **`tests/cli_smoke.rs`**：追加 brief 的两个测试 `fav_help_and_arg_validation`、`fav_bookmark_list_rm_local_source`（逐字）。
4. **`src/core/favorites.rs`**（重构清理，见偏差 2）：`PathBuf` 导入从顶层移入 `#[cfg(test)]` 模块。

## 测试结果

### RED 证据（Step 2）

追加测试后、实现前 `cargo test --test cli_smoke fav`：

```
failures:
    fav_bookmark_list_rm_local_source
    fav_help_and_arg_validation
test result: FAILED. 0 passed; 2 failed; ... 11 filtered out
```

（help 输出中无 `fav` 子命令，`fav` 报 unrecognized subcommand。）

### GREEN 证据（Step 5）

实现后首次跑 fav 测试时 `fav_bookmark_list_rm_local_source` 仍失败一次——原因是我把 brief 的半角 `"(本地源)"` 误转写为全角 `"（本地源）"`，测试断言 `(本地源)` 抓住了这个转写错误；改回与 brief 逐字一致后通过。这印证了「brief 代码逐字使用 + 测试先行」的价值。

最终验证：

```
cargo test          → 120 + 13 + 3 = 136 passed, 0 failed（含既有 help 断言无回归）
cargo clippy --all-targets → 1 warning（见下）
cargo fmt --check   → clean
```

### clippy 警告收敛

基线（接线前）9 个警告 → 接线后 1 个：

| 警告 | 去向 |
|---|---|
| `bookmark`/`unbookmark`/`resolve_key`/`is_single_skill_repo`/`to_fav_skill` never used（5 个） | 已被 CLI 接线，消除 |
| `SkillEntry.description` never read | `bookmark`→`to_fav_skill` 链路激活，消除 |
| `Error::NotBookmarked` never constructed | `unbookmark`/`resolve_key` 激活，消除 |
| `unused import: PathBuf`（favorites.rs） | 本任务重构清零（偏差 2） |
| `fav_install` never used | **保留**：CLI `Install` 分支是计划强制的占位 Err，Task 5 接线后自然消除。CI 只跑 `cargo test`，不卡警告 |

## 与 brief 的偏差

1. **`(本地源)` 括号**：无偏差。过程中我一度误敲为全角，被 RED 阶段的测试抓住后改回，最终代码与 brief 逐字一致（半角括号）。
2. **`src/core/favorites.rs` 的 `PathBuf` 导入**：brief 未提及。这是任务 3 遗留的 clippy 警告（顶层导入仅测试使用，非 test 构建报 unused）。重构步骤将其从顶层移到 `#[cfg(test)] mod tests` 内，纯 lint 清理，不改任何设计语义。
3. **`git add` 范围**：brief 列了 3 个文件，实际多 `git add` 了 `src/core/favorites.rs`（偏差 2 的载体）。提交信息按 brief 原文。

## 留给审查者的疑虑

1. **`FavSub::Install` 占位是计划强制**：当前 `skills fav install ...` 一律返回 "fav install 尚未实现（下一任务）"，参数（target/global/method/yes）只建结构不消费。Task 5 替换为真分发，届时 `fav_install` 的 dead-code 警告同步消除。
2. **子命令名抢占**：`args_conflicts_with_subcommands` 下，`skills fav rm` / `skills fav install` 永远按子命令解析——若某仓库 key 恰好叫 `rm` 或 `install`（如本地目录名为 `rm`），裸 `skills fav rm` 无法收藏它，需写路径形态（`skills fav /abs/path/rm`）。clap 通用行为，设计可接受。
3. **`print_favorites` 的 `fav.skills[0]` 索引**：单技能分支靠 `is_single_skill_repo`（len==1）保证不越界，不变量只在 core 层注释中声明；若未来有人绕过 bookmark 直接构造 Favorite 数据需留意。
4. **TUI/Web 未消费 favorites**：本任务只接 CLI；`registry.favorites` 在 TUI/Web 界面尚不可见（后续任务范围）。
