# Task 5 报告：CLI fav install 接线 + add/fav 共用安装循环收敛 + e2e 全链路

## 实现内容

### src/cli/commands.rs

1. **抽取三个 add/fav 共用助手**（文件底部、`#[cfg(test)]` 之前）：
   - `resolve_targets(target: &[String], global: bool, cfg: &Config) -> Result<Vec<Target>>`：-t 列表 / -g（配置第一个 global target）/ 裸默认（当前项目）三段解析。
   - `resolve_method(method: Option<MethodArg>, cfg: &Config) -> Method`：CLI 参数优先，缺省取 `cfg.default_method`。
   - `install_loop(...)`：8 参数 + `#[allow(clippy::too_many_arguments)]`（注释写明：参数即一次安装批次的全部上下文，打包成结构体只是挪位置，与 `install_skill` 同款平铺签名）。语义 = 「逐技能逐目标安装 + Conflict 确认/跳过 + 逐条落盘」，逐条落盘保住 registry 与磁盘一致的原 add 不变量。
2. **Add 分支重构**为助手调用；技能存在性校验按 brief 有意提前（全部校验通过才开工）。
3. **FavSub::Install 占位替换为真实现**：`resolve_key` → 未给 `--skill` 且收藏含多技能时 dialoguer MultiSelect 从收藏技能集里选（不重扫全仓）→ 空选取消 → `resolve_targets`/`resolve_method` → `install_loop` 包 `favorites::fav_install` → 末尾 `reg.save`。

### tests/e2e.rs（追加 3 个测试，逐字采用 brief）

- `favorites_flow`：整仓收藏 → 两级列表 → 单技能删/补（upsert）→ 从收藏安装（source 给 URL 走 resolve_key 规范化）→ 删整包收藏不影响已安装副本。
- `favorites_single_skill_repo_display`：单技能仓库二级留空、用途挂一级行。
- `fav_install_heals_missing_cache`：手动删缓存目录后 fav install 自愈重克隆。

### tests/cli_smoke.rs（1 个测试改写，见偏差 1）

## 测试结果

### RED 证据（实现前）

```
$ cargo test --test e2e favorites
test favorites_single_skill_repo_display ... ok
test favorites_flow ... FAILED
stderr="错误: fav install 尚未实现（下一任务）\n"

$ cargo test --test e2e fav_install_heals
test fav_install_heals_missing_cache ... FAILED
stderr="错误: fav install 尚未实现（下一任务）\n"
```

`favorites_single_skill_repo_display` 在 RED 阶段即通过——它锁定的是 Task 4 已实现的收藏/列表行为，属预期。

### 重构中检（Step 4 前半）

抽助手 + Add 重构后、接 fav install 前跑全套：unit 120 绿、cli_smoke 13 绿（含改写后的 `add_unknown_skill_fails_before_any_install`）、e2e 仅剩 `favorites_flow` / `fav_install_heals_missing_cache` 两个预期失败。

### GREEN 证据（接线后）

```
$ cargo test
test result: ok. 120 passed; 0 failed   (unit)
test result: ok. 13 passed; 0 failed    (cli_smoke)
test result: ok. 6 passed; 0 failed     (e2e，含 3 个新测试)
```

```
$ cargo clippy --all-targets   → 零警告（fav_install never used 警告随接线消除）
$ cargo fmt                    → 已执行
```

## 与 brief 的偏差

1. **`cli_smoke::add_partial_failure_keeps_registry_consistent_with_disk` 改写为 `add_unknown_skill_fails_before_any_install`**。任务说明写「行为不变，现有测试不能破」，但 brief Step 3 明确标注「有意的行为改进：技能存在性校验提前」，与原测试锁定的旧语义（alpha 先装、ghost 后报错、留半截副本）直接冲突。以 brief 的设计语义为准：新测试锁定新语义——ghost 不存在时 alpha 不装、磁盘与 registry 均无半截状态。原测试守护的「逐条落盘一致性」不变量在 install_loop 注释中保留，且新语义下「仓库中无技能」已不可能发生在安装中途。
2. **两处 clippy 修偏（纯语法、零语义变化）**：brief 逐字代码触发 `unused_parens`（`&(dyn Fn...)` → `&dyn Fn...`）与 `op_ref`（闭包内 `&e.name == s` → `e.name == s`，因 `s: &str` 而非旧代码的 `&String`）。为满足「零警告」要求按 clippy 建议修正。
3. **commit 范围**：brief 只列 `src/cli/commands.rs tests/e2e.rs`，实际加入 `tests/cli_smoke.rs`（偏差 1 的必要组成部分）。

## 留给审查者的疑虑

1. **fav install 的交互多选不可被 e2e 覆盖**：`--skill` 缺省时走 dialoguer MultiSelect，e2e 全部显式传 `--skill` 绕过交互（与 add 的既有测试策略一致）。多技能收藏 + 无 `--skill` 的路径只有手工验证。
2. **fav install 缺「仓库中无技能」前置校验**：Add 分支重构后先校验 picked 全部存在再开工；fav install 分支依赖 `fav_install` 内部逐个报 `NotBookmarked`，若用户一次 `--skill` 多个且含未收藏名，会出现「前几个装完、第 N 个报错」的中途失败（有逐条落盘兜底，registry 不脱节目，但体验与 add 不一致）。brief 未要求对齐，未擅自加。
3. **Conflict 覆盖路径在 fav install 下仅单测覆盖**（`fav_install_conflict_returns_decision_request` 只验证 Conflict 上抛）；e2e 中 `-y` 跳过路径已被 `favorites_flow` 之后的重复安装场景间接覆盖跳过分支，覆盖重装（Confirm→yes）无 e2e。
