# 任务 5 报告：git 封装（gitoxide/gix）

## 状态

任务 5 已完成。代码提交为待写入提交的任务 5 变更；本报告不纳入提交。

## 实现内容

- 新增 `src/core/git.rs`，提供固定 wrapper：
  - `shallow_clone(url, dest) -> Result<String>`
  - `fetch_and_reset(path) -> Result<Option<String>>`
  - `head_commit(path) -> Result<String>`
- `shallow_clone` 使用 `gix::prepare_clone`、`with_shallow`、`fetch_then_checkout` 和 `main_worktree`，不调用 git CLI，返回完整 HEAD object id。
- `fetch_and_reset` 使用 gix remote fetch，解析当前本地分支对应的 `refs/remotes/origin/*`，然后使用 gix index/worktree-state checkout API 更新索引和工作区，并移动本地分支 ref。
- `head_commit` 使用 `gix::open` 和 `Repository::head_id`。
- 测试仅使用系统 git 创建本地 bare fixture；被测 git 封装实现没有系统命令调用。
- `src/core/mod.rs` 暴露 `pub mod git;`。
- `Cargo.toml` 启用 gix `worktree-mutation`，并加入与 gix 0.66 配套的 `gix-worktree-state`、`gix-worktree`、`gix-filter`；`Cargo.lock` 已更新。

## gix 0.66 API 核对与偏差

已核对本机 cargo registry 中的 `gix-0.66.0` 源码，并用 Context7 查询了 clone/fetch API。

1. `gix::prepare_clone(url, path)` 在 0.66 存在，返回 `clone::PrepareFetch`；`fetch_then_checkout` 需要 `worktree-mutation` 与 `blocking-network-client` feature，当前配置已启用。
2. `PrepareCheckout::main_worktree(progress, should_interrupt)` 在 0.66 存在并返回 `(Repository, checkout::Outcome)`，当前调用与实际签名一致。
3. 简报示例中的 `Shallow::DepthAtLeast` 不存在于 gix 0.66。实际 enum 使用 `Shallow::DepthAtRemote(NonZeroU32)`，因此实现用 `DepthAtRemote(1)` 表示远端视角的 depth=1。
4. 既有仓库 fetch 使用 `prepare_fetch(...).with_shallow(...).receive(...)`；该链式 API 在 gix 0.66 可用。fetch 后显式重新打开仓库，避免继续使用 fetch 前的 ref 视图。
5. gix 0.66 没有一个可直接调用的完整 `reset --hard` wrapper，因此工作区 reset 使用其公开的 index/worktree-state API：从目标 commit 的 tree 重建 index，checkout 到工作区，写回 index，再更新本地分支 ref。没有引入 git CLI 到生产实现。

## 测试命令与结果

### `cargo fmt --all`

通过并完成格式化。格式化命令曾发现 `src/core/config.rs` 中两处与本任务无关的历史格式差异；为避免任务 5 提交带入旁支变更，已恢复该文件原状。任务 5 相关代码保持 rustfmt 格式。

### `cargo test git`

通过：7 passed, 0 failed。

覆盖：

- 本地 bare repo 的 depth=1 clone 返回 40 位完整 commit hash。
- clone 后 `head_commit` 与返回值一致，工作区文件已 checkout，`.git/shallow` 存在。
- 无新远端提交时 `fetch_and_reset` 返回 `None`。
- 远端提交推进后返回新 hash，HEAD、索引和工作区同步；远端删除文件被移除，新增文件出现。
- 再次 fetch 无变化时返回 `None`。
- 不存在的仓库返回错误。

### `cargo test`

通过：24 passed, 0 failed。

测试过程中只有已有的 dead-code warning：`Error` 中若干未构造 variant，以及 `Layout::new` 未使用；没有编译错误或测试失败。

## 疑虑与边界

- 当前 `fetch_and_reset` 针对当前 HEAD 为符号引用且存在 `origin/<当前分支>` 的普通 clone；detached HEAD 会返回明确错误。
- 实现按需求将工作区视为可完全覆盖的缓存目录，reset 时会删除工作区中除 `.git` 外的内容。调用方不应在该目录放置未提交的用户文件。
- gix 0.66 的 `DepthAtRemote(1)` 在连续 fetch 时维持远端视角的 depth=1；浅边界文件可能保留多个边界记录，这是 gix 的实现行为，不影响 HEAD 和工作区结果。
- 测试依赖系统 `git`，仅用于 fixture 创建和断言；生产 git 封装保持纯 Rust/gix。

## 提交范围

应提交：`src/core/git.rs`、`src/core/mod.rs`、`Cargo.toml`、`Cargo.lock`。
不应提交：本报告及 `.superpowers/` 下其他文件。

## 任务 5 第 1 轮修复

### 改动

- `fetch_and_reset` 保存 `find_default_remote` 选中的实际 remote 名称，用 `refs/remotes/<remote>/<branch>` 查找 tracking ref；新增非 `origin`（`upstream`）remote 回归测试。
- `checkout_tree` 改为先在工作区旁的 staging 目录完成目标 commit tree、临时 index 和 checkout，全部准备成功后才清理并替换真实工作区；新增 checkout 失败后旧文件仍存在的测试。

### 验证

- `cargo fmt --all`：退出码 0；formatter 对无关 `src/core/config.rs` 的两处历史格式化改动已恢复，任务文件保持格式化结果。
- `cargo test git`：实际输出 `test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 17 filtered out`。
- `cargo test`：实际输出 `test result: ok. 26 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`；仅有既有 dead-code warning。

## 任务 5 第 2 轮修复

### 根因

第 1 轮只把 tree、临时 index 和 checkout 构建放入 staging。进入回写阶段后，代码仍会先删除旧工作区，再逐项 `rename` 新文件，最后替换 index；这些操作任一步失败都会留下部分新工作区或缺失旧文件。`fetch_and_reset` 随后更新本地分支 ref，若 ref 更新失败，工作区和 index 也可能已经切换，形成三者不一致。

### 实现

- 新增 `CheckoutTransaction`，把工作区、index 和本地分支 ref 的切换作为一个显式事务。
- 目标 commit 的 tree、工作区和临时 index 仍先在唯一 staging 目录中完整构建；构建失败时不触碰真实仓库。
- 回写时把旧工作区 entries 移入同级 backup，并记录每个已移动 entry；再把 staging entries 移入真实工作区并记录。任何阶段失败时，只删除本事务实际安装的新 entries，再按记录恢复旧 entries，避免备份旧文件阶段失败时误删尚未移动的旧文件。
- index 先移动到事务目录的 `old-index`，新 index 成功安装后才标记为已安装；失败时移除新 index 并恢复旧 index。没有旧 index 的场景也能回滚新 index。
- 分支 ref 更新在工作区/index 安装之后执行；ref 更新调用返回错误，或更新成功后后续阶段失败，都会尝试写回旧 oid，然后恢复 index 和工作区。
- 成功切换后仅 best-effort 清理 staging，清理失败不作为错误返回，避免已经完成的新状态被报告为失败。跨平台依赖的是同一父目录内的 `rename`，不跨文件系统移动。
- 保留第 1 轮的实际默认 remote 名称解析，因此非 `origin` remote 修复没有回退。

### 回归测试

新增 `checkout_transaction_rolls_back_after_switch_failures`：使用本地 bare repo 和真实 c1/c2 对象，故障注入覆盖：

- `AfterIndexInstall`：工作区和新 index 已切换但 ref 尚未切换；
- `AfterRefUpdate`：工作区、index 和 ref 已切换。

两种失败均断言旧文件仍存在、远端删除/新增文件未泄漏、`.git/index` 字节与旧值一致、`refs/heads/main` 和 `head_commit` 仍指向 c1。现有 staging 构建失败测试及非 `origin` remote 测试继续保留。

### 第 2 轮验证

- `cargo fmt --all`：通过；无关 `src/core/config.rs` 的 formatter 改动已恢复。
- `cargo test git`：`10 passed, 0 failed`。
- `cargo test`：`27 passed, 0 failed`；仅有既有 dead-code warning。

## 任务 5 第 3 轮修复

### 审查问题

第 2 轮的 rollback 仍是多个独立 best-effort 操作：恢复 ref、删除/恢复 index、清理/恢复工作区即使前一步失败也会继续，并将错误拼接为普通 `Error::Git`。这不能证明调用方收到错误时缓存仍可用。

### 设计与取舍

- `CheckoutTransaction::prepare` 在切换前保存完整工作区快照和旧 `.git/index` 字节；切换阶段继续把旧顶层 entries 移入同一父目录内的 backup，并把目标 tree/index 预先完整构建在 staging 中。
- rollback 严格按 install 的逆序执行：ref、index、工作区。ref 恢复先读取当前值，只允许当前值为旧值（已恢复）或新值（用 `MustExistAndMatch(new)` 原子地写回旧值），拒绝覆盖未知并发修改。
- index 恢复以保存的旧字节为验收条件；工作区恢复使用反向 entry 顺序，并与切换前的文件、目录和符号链接快照逐项比较。
- 任一恢复或验收失败都返回新增的 `Error::GitRecovery`，保留 staging/backup 恢复材料，不让调用方误以为缓存可用。正常注入边界恢复成功后才清理 staging 并返回原切换错误。文件系统无法提供跨平台绝对原子性的部分被限制在同一父目录 `rename`，并由上述校验和不可用错误边界兜底。

### 真实回归测试

`checkout_transaction_rolls_back_after_switch_failures` 保留本地 bare repo、真实 c1/c2、非 origin 测试，并覆盖三个切换后失败边界：`AfterWorktreeInstall`、`AfterIndexInstall`、`AfterRefUpdate`。每次失败均断言旧工作区文件恢复、远端新增/删除文件不泄漏、`.git/index` 字节完全等于旧值、`refs/heads/main` 与 HEAD 仍指向旧 commit。

### 第 3 轮验证

- `cargo fmt --all`：退出码 0；formatter 对无关 `src/core/config.rs` 的历史格式差异已恢复，未纳入本任务。
- `cargo test checkout_transaction_rolls_back_after_switch_failures -- --nocapture`：`1 passed, 0 failed`。
- `cargo test git`：`10 passed, 0 failed`。
- `cargo test`：`27 passed, 0 failed`；仅有既有 dead-code warning。
