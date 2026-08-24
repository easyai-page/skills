# 任务 15 报告：copy 副本本地修改检测

## 实现内容

按调度简报的 6 条绑定设计决策逐条落地：

1. **`.skills-manifest` 扩展为 sha256 基线**（新增 `src/core/manifest.rs`）：
   格式 `{"version":1,"manager":"skills","files":{"<相对路径>":"<sha256-hex>"}}`；
   键为相对路径、正斜杠（`forward_slash_relative`，Windows 上也是 `docs/deep.md`）、
   `BTreeMap` 保证有序；manifest 自身不入清单（`hash_tree`/`collect_extras` 均跳过）。
   copy 安装在 staging 阶段写入（`install::stage_copy` → `manifest::write_copy_manifest`），
   update 走同一 staging 流程（`replace_copy_install` → `stage_copy`），
   故每次成功更新后基线自动刷新；symlink 安装不写 manifest
   （`symlink_install_does_not_write_manifest_marker` 测试锁定）。
   原 `install.rs` 里只含归属标识的旧 `write_copy_manifest` 删除，
   `COPY_MANIFEST` 常量移到 manifest 模块并在 install 再导出，保持既有引用路径。
2. **分歧判定**（`manifest::detect_local_modifications`）：内容被改、被跟踪文件缺失、
   副本内出现基线外文件、manifest 缺失/损坏/无 `files` 字段（任务 15 之前的旧版安装）
   四类一律视为分歧；每类产出人类可读明细行（哪个文件被改/删/新增、更新后将被删除）。
   基线条目经 `checked_relative_path` 校验（拒绝绝对路径与 `..`），
   防止用户手改 manifest 借校验读副本外文件。
3. **`execute_plan(..., force: bool)`**：`force=false` 时在任何变更之前
   （包括 repo pull 之前）对计划内所有将更新的 copy 副本做
   `pre_scan_local_modifications` 预扫描；任一副本有分歧即整体返回一个
   `Error::Mismatch`，汇总所有副本的明细，不产生任何部分变更。
   副本已被手动删除（无可保护内容）或被替换成链接/文件（归属核验问题，
   force 也无法跳过）不混入可确认清单，交主循环按原语义报错。
   `force=true` 行为同任务 15 之前，另因 staging 流程统一而自动重写 manifest。
4. **CLI**：`skills update` 新增 `--force`（`src/cli/mod.rs`）。不带 `--force` 遇到
   Mismatch 时（`src/cli/commands.rs`）：打印明细 → dialoguer Confirm（默认否）→
   确认后以 force 重跑一次；放弃则「未做任何修改」。非 TTY 下 `interact()` 失败，
   错误文案明确引导「非交互环境请改用 skills update --force 明确覆盖」。
5. **Web**：`POST /api/update` 支持 `?force=true` 查询参数（`src/web/api.rs`，
   JSON body 契约不变）；Mismatch 映射为 409 + 明细正文，其余错误仍为 500。
   `index.html` 的 `api()` 把 HTTP status 挂到 Error 上，`runUpdate` 捕获 409 →
   `confirm(明细)` → 同意后自动以 `?force=true` 重试一次。
6. **TUI**：无 update 触发入口，本任务未接线（未触碰 `src/tui/`）。

## TDD 证据

诚实说明：本任务的 RED→GREEN 由前一实现 agent 完成，其 transcript 显示新测试先行
失败（Mismatch 路径、manifest 基线断言），实现后转绿；该 agent 在写报告前因 API 错误
中断。本报告不重述其 RED 输出，以下为本次收尾时重跑的 GREEN 证据：

`cargo test --bin skills -- core::`（任务 15 新增/扩展的 11 个相关测试全绿）：

```text
test core::install::tests::copy_install_manifest_records_sha256_baseline ... ok
test core::install::tests::copy_install_writes_manifest_marker ... ok
test core::install::tests::symlink_install_does_not_write_manifest_marker ... ok
test core::manifest::tests::missing_or_legacy_manifest_reports_no_baseline ... ok
test core::manifest::tests::written_manifest_detects_clean_then_flags_each_divergence_class ... ok
test core::update::tests::clean_copy_updates_without_force ... ok
test core::update::tests::extra_untracked_file_blocks_update_without_force ... ok
test core::update::tests::force_update_overwrites_local_modification_and_rewrites_manifest ... ok
test core::update::tests::legacy_manifest_without_files_blocks_then_force_repairs_baseline ... ok
test core::update::tests::local_modification_blocks_update_without_force ... ok
test core::update::tests::missing_tracked_file_blocks_update_without_force ... ok
test result: ok. 88 passed; 0 failed; 0 ignored; 0 measured; 15 filtered out
```

`cargo test --bin skills -- web::`：

```text
test web::api::tests::run_update_409_on_local_modification_then_force_succeeds ... ok
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 97 filtered out
```

全量：`cargo test` = 103（unit）+ 10（cli_smoke）+ 2（e2e）全绿；
`cargo clippy -- -D warnings`、`cargo clippy --all-targets -- -D warnings`、
`cargo fmt --check` 全部退出 0。

## CLI 冒烟实录

前一 agent 的冒烟不完整：未开包级 `auto-update --source <key> --on`，裸 `update`
计划为空、什么都没发生。本次按正确序列重跑（本地 bare 仓库 fixture、`SKILLS_HOME`
隔离、`config.toml [targets]` 重定向 agents 到临时目录，脚本模式 crib 自
`tests/e2e.rs`）：

```text
== $ skills add file:///tmp/task15-smoke.kk7lWo/bare.git -s alpha -t global:agents --method copy -y
已安装 alpha → Global { name: "agents" } (Copy)
== source key: file/task15-smoke.kk7lWo/bare
== $ skills auto-update --source file/task15-smoke.kk7lWo/bare --on
== 安装时 commit: d689389f
（上游推 c2：SKILL.md → v2；副本本地：SKILL.md 追加「用户本地修改」+ 新增 my-notes.txt）

== $ skills update   (非交互，期望失败)
以下 copy 副本存在本地修改，更新将覆盖并丢失这些改动：
alpha @ Global { name: "agents" }:
  - 内容被修改: SKILL.md
  - 副本内新增: my-notes.txt（更新后将被删除）
确认覆盖请强制重试（CLI 加 --force，Web 端在确认框同意后自动重试）
错误: 确认交互失败（IO error: not a terminal）；非交互环境请改用 skills update --force 明确覆盖
exit=1
--- 失败后现场检查 ---
OK: 副本仍是本地修改版
OK: 新增文件未被删除
registry commit 对: d689389f... d689389f...   （install 与 source 均停在 c1）
OK: registry commit 未推进                     （预扫描在 pull 之前中止，连 fetch 都未发生）

== $ skills update --force   (期望成功)
仓库 file/task15-smoke.kk7lWo/bare → ca785d36
副本 alpha @ Global { name: "agents" } 已更新
exit=0
--- force 后现场检查 ---
SKILL.md 内容为 v2（本地修改被覆盖）
OK: 基线外文件 my-notes.txt 已随 remove+copy 清除
manifest files: { "SKILL.md": "c421fc0a…386d6" }
OK: manifest 中 SKILL.md 哈希与 v2 磁盘内容一致（基线已刷新）

== $ skills update   (期望成功、无误报)
副本 alpha @ Global { name: "agents" } 已更新
exit=0
```

全部符合预期：非 TTY 裸 update 失败且明细点名被改与新增文件、引导 --force；
目标目录与 registry 零变更；--force 覆盖成功、基线外文件清除、manifest 哈希刷新为
新内容；再次裸 update 无误报。

## 文件变更

- 新增：`src/core/manifest.rs`（manifest 写入 + 分歧检测 + 单测）
- 修改：`Cargo.toml`（+`sha2 = "0.10"`）、`Cargo.lock`、`src/core/mod.rs`（挂模块）、
  `src/core/install.rs`（stage_copy 写 sha256 基线；删旧归属标识写入函数）、
  `src/core/update.rs`（execute_plan 加 force 参数 + 预扫描 + 6 个新测试）、
  `src/cli/mod.rs`（--force 旗标）、`src/cli/commands.rs`（Mismatch 确认链）、
  `src/web/api.rs`（?force=true + 409 映射 + 集成测试）、
  `src/web/static/index.html`（409 → confirm → force 重试）
- 未触碰：`docs/`、`.superpowers/`、`src/tui/`、测试基建（e2e/cli_smoke 无需改动，
  全部原样通过）

## 自审

对照 6 条决策逐条核对工作树，均符合：

- 预扫描位于 `execute_plan` 的 sources pull 循环之前（update.rs:113-115），
  冒烟中「registry commit 未推进」实证了「任何变更之前中止」✓
- install 与 update 共用同一 staging+manifest 流程，基线「安装时写入、
  每次成功更新后刷新」由构造保证而非两处各自维护 ✓
- 归属核验类 Mismatch（副本被换成链接等）即使在 CLI 确认链里 force 重跑也会再次
  如实报错（代码注释已说明），不会误删用户目录 ✓
- Web 端 409 不产生任何变更由集成测试断言（409 后磁盘内容仍是用户修改版）✓

## 顾虑

1. **已是最新也会重复制**：包级+副本级 auto-update 均开时，只要仓库进了 pull 列表，
   copy 决策即为 `update: true`，与 fetch 是否真有新提交无关（build_plan 既有语义，
   非本任务引入）。冒烟第 3 步可见：无新提交时裸 update 仍打印「副本 alpha 已更新」。
   任务 15 之后该行为安全（干净副本重复制无误报、manifest 重写为相同内容），
   仅有一次多余 IO；是否按 commit 变化跳过留给后续任务决策。
2. **CLI 确认链会兜住归属核验类 Mismatch**：用户确认后 force 重跑仍会在
   `verify_copy_ownership` 再次失败——行为正确（不删不更新）但多一轮无效确认；
   明细文案能区分两类场景，可接受。
3. manifest 校验按字节哈希，`core.autocrlf` 等 git 层面归一化不影响已落盘副本
   （复制的是工作区文件），无误判风险；但若用户用编辑器改动行尾保存，会被如实报为
   「内容被修改」——这正是设计意图。
