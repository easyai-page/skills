# 任务 9 报告：两级更新引擎

- 提交：`bf528c3` feat(core): 两级更新引擎（包级 pull + 副本级传播 + dry-run 计划）
- 基线：HEAD `0b60cf4`（任务 1-8 已完成）
- 日期：2026-08-20

## 交付内容

### 新建 `src/core/update.rs`

两级更新策略：

1. **仓库级（SourceRecord.auto_update）**：`repo_should_update` 决定是否对缓存仓库执行 `fetch_and_reset`，默认 false。
2. **副本级（Install.auto_update）**：`copy_should_update` 中副本级配置覆盖包级配置（`inst.auto_update.or(source.auto_update).unwrap_or(false)`）。

核心接口（严格按简报）：

- `repo_should_update(&SourceRecord) -> bool`
- `copy_should_update(&Registry, &Install) -> bool`
- `Selection { skill, target }`：显式 `update <skill> --target` 的选中项
- `Plan { sources, symlinks, copies: Vec<CopyDecision> }`：dry-run 可展示全部决定（含跳过项及 reason）
- `build_plan(&Registry, Option<&Selection>) -> Plan`：
  - `None`：扫描全部仓库与副本；symlink 跟随仓库级策略（仓库在 pull 列表才更新）；copy 需仓库在计划内 **且** 两级配置允许；reason 区分「仓库不更新 / 副本级/包级配置关闭 / 更新」。
  - `Some(sel)`：无视配置强制更新该副本，且强制将其来源仓库加入 pull 列表。
- `execute_plan(layout, cfg, reg, plan) -> Result<Vec<String>>`：
  - 对 plan.sources 逐个 `git::fetch_and_reset`，更新 `SourceRecord.commit/fetched_at`；
  - 对 update=true 的 copy 副本：删除旧目录 → `cache::copy_dir` 从缓存重复制 → 同步 `Install.commit`；
  - symlink 副本仅同步 `Install.commit` 跟随仓库；
  - 最后 `reg.save(layout)` 原子落盘；返回人类可读的操作清单。

### 修改 `src/core/mod.rs`

新增 `pub mod update;`（1 行）。

## TDD 过程

1. **RED**：先写 4 个测试（update.rs 仅含 tests），`cargo test update` 编译失败（E0425/E0422/E0433，函数与类型未定义），符合简报预期。
2. **GREEN**：补上实现，`cargo test update` → 4 passed / 0 failed。

## 测试小结

- `cargo test update`：4/4 PASS
  - `repo_update_follows_source_flag_default_false`：包级默认 false，Some(true) 才更新
  - `copy_effective_flag_install_overrides_source`：副本级覆盖包级（false 盖 true、true 盖 false、None 跟随、双 None 默认 false）
  - `plan_respects_two_levels`：包级开时 symlink 全更新 + copy 按副本级过滤；包级关时全不更新
  - `explicit_skill_target_forces_update`：显式指定无视双 false 强制更新且强制 pull 仓库
- `cargo fmt`：已执行（仅重排 mod.rs 一行）
- `cargo test` 全量：**63 passed, 0 failed**（任务 1-8 的 59 个测试无回归）
- 编译警告：仅既有的 cache.rs dead_code 警告，本次无新增警告

## 未触碰

- `docs/`、`.superpowers/` 未改动。
- 未接入 CLI 子命令（简报未要求，属后续任务）。

## 备注

- `execute_plan` 中 `{new_commit:.8}` 依赖 Rust 对字符串 Display 的精度截断语义展示短 SHA，编译与测试均已验证。
- copy 副本重复制前 `remove_dir_all(dest)`：因 `.skills-manifest` 归属标识保证该目录归本工具管理（任务 8 已加固），与 install 引擎的原子复制策略一致地先清后拷。

---

# 任务 9 第 1 轮修复报告

- 提交：`2676866` fix(core): 加固更新引擎（原子替换+归属校验+manifest 重写+execute_plan 集成测试）
- 日期：2026-08-20

## 逐条处理

1. **关键（copy 更新丢 manifest）**：install.rs 抽取可复用的 `stage_copy`（staging 复制+`.skills-manifest` 写入，失败自清理），install 与 update 共用同一份流程；update 改走 `replace_copy_install`，更新后的副本带归属标识，remove 可正常删除（有集成测试验证）。
2. **重要（绕过任务 8 防线）**：remove.rs 的 `validate_record` 与副本归属核验（提取为 `verify_copy_ownership`）改为 `pub(crate)`，update 在动磁盘前执行完全相同的校验；manifest 缺失 / 外部目录替换 / 实况与记录不符均返回 `Error::Mismatch`，不删不更新。
3. **重要（先清后拷非原子）**：`replace_copy_install` = staging 完成 → 旧副本 rename 到 `.{skill}-update-backup-*.tmp` 备份位 → 暂存 rename 提交 → 成功后删备份；任一阶段失败回滚恢复原副本（提交失败时 backup rename 回原位），与 install 原子语义一致。dest 已被手动删除时无备份直接提交。
4. **重要（unwrap panic）**：`reg.find(...)` 未命中改为 `Error::NotInstalled`；`reg.sources[...]` 两处索引改 `get` + `Error::Mismatch` 错误返回。
5. **重要（execute_plan 零测试）**：新增 8 个集成测试（见下）。

## 次要项

- 显式 selection 找不到记录：`Plan` 新增 `missing: Option<String>` 字段（`build_plan` 填充），`execute_plan` 遇之返回 `Error::NotInstalled`，结果可区分。
- 本地源显式更新：plan.sources 中缓存无 `.git` 时返回友好 `Error::Msg`（提示重新 add 本地路径），取代 gix 的晦涩报错。
- 未扩大重构：docs/ 与 .superpowers/ 未动。

## 测试小结

- `cargo test update`：12/12 PASS（原 4 个计划单测无改动 + 新增 8 个）
  - `copy_update_replaces_via_staging_and_stays_removable`：更新后 manifest 存在、内容 v2、无暂存/备份残留、remove 可正常删除
  - `copy_update_refused_when_ownership_marker_missing` / `..._dir_replaced_by_foreign_dir`：Mismatch 拒绝，原目录与记录不动
  - `execute_plan_copy_missing_record_returns_not_installed`：plan/registry 不匹配不再 panic
  - `explicit_selection_for_unknown_skill_is_distinguishable`：missing 字段 + NotInstalled
  - `local_source_update_returns_friendly_error`：本地源友好报错
  - `explicit_selection_forces_update_for_symlink_and_copy`：真 git 远端推 c2，symlink/copy 均强制更新且 commit 跟随仓库
  - `fetch_without_change_does_not_touch_commit`：fetch 无变化时 commit/fetched_at 不抖动、无“仓库 →”条目
- `cargo fmt` 已执行；`cargo test` 全量：**71 passed, 0 failed**（任务 1-8 的 59 个无回归）
- 无新增编译警告（`reason` 字段告警为 bf528c3 既有）
