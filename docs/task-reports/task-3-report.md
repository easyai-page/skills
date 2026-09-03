# Task 3 报告：core::favorites——fav_install（含缓存自愈）

日期：2026-08-27
分支：feat/skill-favorites
提交：c0058e2 `feat: fav_install 从收藏安装（含缓存自愈）`

## 实现内容

按 brief 逐字落地，唯一文件改动：`src/core/favorites.rs`（+230/-2）。

1. **测试（RED）**：tests 模块追加 `setup_with_cfg()` 辅助函数 + 5 个 fav_install 测试；
   brief 的 4 行 use（Config / COPY_MANIFEST / Target / Method）合并到 tests 模块顶部已有 use 区（内容逐字未改）。
2. **实现（GREEN）**：`fav_install(layout, cfg, reg, source_key, skill, target, method) -> Result<Install>`：
   - source_path 取自收藏快照（`fav.skills[].source_path`），不重扫仓库；
   - 缓存完整性判定：git 源（url.is_some()）以 `.git` 存在为准，本地源以目录存在为准；
   - 缓存缺失时凭快照 url/local_path 重建 SourceSpec 并 `ensure_cached` 自愈，自愈后同步刷新
     `sources[key].commit/fetched_at` 与 `favorites[key].commit`；
   - 本地源且 local_path 已不存在：返回 `Error::Msg("本地源 … 已不存在，无法重建缓存；请重新 fav 有效路径")`；
   - 未知 favorite / 未知技能：`Error::NotBookmarked`；
   - 最终委托 `install::install_skill`（commit 取 sources 记录），`Error::Conflict` 原样上抛；调用方负责落盘。
3. use 区按 brief 合并：`super::config::Config`、`super::install` 新行；`paths` 并入 `Target`；`registry` 并入 `Install, Method`。

## 测试结果

### RED 证据（实现前 `cargo test fav_install`）

```
error[E0425]: cannot find function `fav_install` in this scope
   --> src/core/favorites.rs:360:9
（共 7 处 E0425）
error: could not compile `skills` (bin "skills" test) due to 7 previous errors
```

符合 brief Step 2 预期（fav_install 未定义的编译错误）。

### GREEN 证据（实现后 `cargo test favorites`）

```
test core::favorites::tests::fav_install_installs_from_snapshot ... ok
test core::favorites::tests::fav_install_unknown_favorite_or_skill_errors ... ok
test core::favorites::tests::fav_install_heals_missing_cache ... ok
test core::favorites::tests::fav_install_errors_when_cache_and_local_source_both_gone ... ok
test core::favorites::tests::fav_install_conflict_returns_decision_request ... ok
test result: ok. 15 passed; 0 failed   （含 Task 1/2 全部 favorites/registry 测试）
```

### 全量验证（fmt 后）

- `cargo test`：120（单元）+ 11（cli_smoke）+ 3（e2e）= 134 全绿，0 failed。
- `cargo fmt`：已跑（仅重排了元组解构与 `is_some_and` 条件的换行，语义与 brief 一致）。
- `cargo clippy --all-targets`：见下方偏差说明。

## 与 brief 的偏差

1. **use 行放置位置**：brief 测试代码块以 4 行 use 开头，实际合并进 tests 模块顶部已有 use 区
   （`use super::*;` 之后），实现区的 4 个 use 按 brief 指示合并进 favorites.rs 顶部已有 use 行。
   代码文本逐字未改，仅位置调整。
2. **cargo fmt 重排**：brief 实现代码中两处（`(source_path, url, local_path)` 元组解构、
   `local_path.as_ref().is_some_and(...)` 条件）被 rustfmt 改写了换行/缩进，语义不变。
3. **clippy 警告（无新增类别，1 个新实例）**：`cargo clippy --all-targets` 无法做到零警告——
   基线（Task 2 提交 f3fb1ad）已有 8 个 dead-code 类警告（bookmark/unbookmark/resolve_key/
   is_single_skill_repo/to_fav_skill 未使用、NotBookmarked 未构造、PathBuf 未使用导入、
   cache::SkillEntry.description 未读），原因是 favorites 尚未接入 CLI（后续任务才接线）。
   本任务新增 1 个同类警告 `function fav_install is never used`（8 → 9），属同一既定模式，
   新代码本身无任何 clippy 风格 lint。未为消警告而加 `#[allow(dead_code)]`（前两任务也未加，
   接线后自然消除）。

## 留给审查者的疑虑

1. **自愈会刷新 fav.commit**：git 源缓存被删后自愈等价于浅克隆最新 HEAD，`fav.commit` 与
   `sources.commit` 随之前进——装出来的内容可能比收藏快照新。这是 brief 的明确设计
   （注释已写明「自愈后 HEAD 可能前进」），但意味着 fav install 不保证装到收藏时刻的内容；
   若未来想要「装快照时刻的 commit」，需要 ensure_cached 支持 pin commit，属于设计层决策。
2. **本地源「缓存完整」判定只看目录存在**：`cache.is_dir()` 为真即不重拷，若用户手动改坏缓存目录
   内容（目录还在但文件被删），fav install 不会自愈，install_skill 会因找不到 source_path 报错。
   与 brief 一致，但严格性弱于 git 源的 `.git` 判定，可接受（install 侧会兜底报错）。
3. **commit 取值顺序**：未触发自愈时 commit 取自既有 `sources[key]`（bookmark 时登记），
   与缓存实际 HEAD 一致的前提是没人绕过本工具动缓存——与 add 路径的既有信任假设相同。
4. **`local_path.unwrap_or_default().display()`**：错误分支已保证 local_path 为 Some
   （`is_some_and` 为真才进入），unwrap_or_default 只是满足类型，不会走到 default 路径；
   brief 原文如此，保留。
5. **工作区既有未暂存删除**：`task-8..15-report.md` 的删除是接手前就有的工作区状态，
   本提交只 `git add src/core/favorites.rs`，未触碰这些文件，留待用户处理。
