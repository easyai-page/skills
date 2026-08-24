# 最终审查修复报告（fix-wave，2026-08-24）

基线：feat/skills-manager @ 923c573，103 unit + 10 cli_smoke + 2 e2e 全绿，
`cargo clippy --all-targets -- -D warnings` 与 `cargo fmt --check` 干净。
最终审查结论「With fixes」，本报告覆盖全部 5 项修复，一轮完成。

完成后验证（单次全量）：

```
cargo test --workspace
  test result: ok. 104 passed; 0 failed   （unit，+1：web 损坏 config 用例；
                                            TUI 删 1 个死状态测试、cache 净 +1）
  test result: ok. 11 passed; 0 failed    （cli_smoke，+1：add 部分失败回归）
  test result: ok. 3 passed; 0 failed     （e2e，+1：symlink 全链路，cfg(unix)）
cargo clippy --all-targets -- -D warnings → Finished，无警告
cargo fmt --check → 无 diff
```

各修复均先写失败测试（RED）再改实现（GREEN），下文附逐条验证命令与结果。

---

## Fix 1 — CLI add 部分失败留下磁盘/registry 脱节

**问题**（src/cli/commands.rs 原 :109-157）：add 循环中任一失败（未知技能名、
安装错误、确认覆盖后重装失败）都在 `reg.save` 之前返回，而已成功的迭代早已
把副本写入磁盘并推进了内存 registry → 磁盘上有 `list`/`remove` 都看不到的
孤儿副本。

**修复**：循环内每个「技能×目标」安装成功后立即 `reg.save(&layout)?`
（tmp+rename 原子写，代价可忽略）；跳过（-y 冲突跳过、用户拒绝覆盖）不落盘；
循环末尾保留最终 save。中途任何错误退出时，registry 与磁盘已写入副本一致。

**测试**（tests/cli_smoke.rs `add_partial_failure_keeps_registry_consistent_with_disk`）：
重定向 agents target 到临时目录后 `add <本地源> -s alpha -s ghost … -y`，
ghost 不存在 → 非零退出且 stderr 含 "ghost"；断言磁盘副本已写入、`list` 可见
alpha、`remove` 能正常清掉。
RED：`cargo test --test cli_smoke add_partial_failure` → FAILED
（"registry 丢失已装副本: （无已安装技能）"）；GREEN：11 passed。

## Fix 2 — 本地源缓存永久冻结 + update 补救提示无效

**问题**（src/core/cache.rs 原 :24-28）：`ensure_cached` 对本地源只判缓存目录
非空即复用，源目录改动后重跑 add 永远拿到旧快照；而 update 对本地源只能报错
「本地源请重新 add 对应本地路径刷新」（src/core/update.rs :119-123）——
一条什么也不做的指示。

**修复**：`ensure_cached` 对 `local_path` 源（无 url）取消复用分支，每次 add
都先清旧缓存再重新拷贝（纯文件操作，无 git，代价低）；git 源复用语义不变。
附带一个自保守卫：本地源路径 canonicalize 后恰为缓存目录本身（用户直接 add
`~/.skills/local/<name>`）时直接复用，否则「先清缓存」会删掉用户的源目录。
update.rs 的错误消息经此修复后变得真实有效（重新 add 确实刷新），保持原文；
`local_source_update_returns_friendly_error` 测试仍通过。

**测试**（src/core/cache.rs）：
- `ensure_cached_recopies_local_source_on_every_call`（替换原
  `ensure_cached_copies_local_source_once_then_reuses`）：源目录改内容/删文件/
  加文件后再次 ensure_cached → fresh=true、新内容生效、已删文件不残留。
  RED：`cargo test --bin skills cache` → FAILED（"本地源每次 add 都应重拷"）；
  GREEN：14 passed。
- `ensure_cached_never_deletes_local_source_that_is_the_cache`：源即缓存时
  复用且目录内容不变（修复前后均应通过的自保锁）。

## Fix 3 — web run_update 对损坏 config 静默回退默认

**问题**（src/web/api.rs 原 :158）：`Config::load(&s.layout).unwrap_or_default()`
使损坏的 config.toml 在 update 时被当作默认配置，内置 target 解析到用户真实
home 目录并落盘。

**修复**：改为 `map_err` 返回 500 + 错误消息（"加载 config 失败: …"），与紧随
其后的 `Registry::load` 损坏处理（人工裁定的行为基准）一致。

**测试**（src/web/api.rs `run_update_returns_500_on_corrupted_config`）：
写入越界 port 的 config.toml → POST /api/update → 500 且 body 含
"加载 config 失败"。RED：`cargo test --bin skills web::api` → FAILED；
GREEN：7 passed。

## Fix 4 — e2e 缺 symlink 方式覆盖

**修复**（tests/e2e.rs `add_list_remove_symlink_flow`，`#[cfg(unix)]`）：
`add --method symlink` → 断言安装产物是符号链接且 read_link 指向
`<SKILLS_HOME>/<cache key>/skills/alpha`（key 从 registry.json 实读不硬编码）、
透过链接可读 SKILL.md、`list` 输出含 "Symlink"；`remove` 后链接消失、缓存
内容原样保留。沿用文件内既有 fixture / SKILLS_HOME / redirect_agents_target
模式。

**残留缺口**：Windows junction 回退路径无覆盖。junction 断言需要 Windows
环境且无开发者模式权限条件，CI 单测也无法稳定构造，故 cfg-gate 到 unix；
junction 创建逻辑本身（install.rs `make_symlink` cfg(windows) 分支）仍未被
任何测试触达，属审查清单外的既定豁免。GREEN：`cargo test --test e2e` →
3 passed。

## Fix 5 — 文档 v0.1 范围 + TUI 死状态

**a) 文档**（human 批准的 v0.1 范围口径）：
- plan `自检结果`：原「TUI → 任务 12 ✓」「Web 完整管理界面 → 任务 13 ✓」
  改为如实标注 v0.1 范围并列出后续候选——TUI v0.1 = 三视图浏览/导航、
  auto_update 三态切换、安装向导（单目标、默认 method）；Web v0.1 =
  已安装列表（分类/三态开关/删除）+ 仓库缓存包级开关 + 更新执行（409 确认链），
  无安装向导、无配置管理。
- spec TUI/Web 两小节同步修订为 v0.1 范围 + 后续候选（TUI remove/tag/update
  快捷键、tag 筛选、向导多选目标与 method 选择、sources 检查更新与缓存清理；
  Web 安装向导、配置管理页）。文档其他部分未动。

**b) TUI 死状态（src/tui/app.rs、src/tui/mod.rs）**：选择**移除**而非接线。
`tag_filter` 只有 reducer 支持和一个直接写字段的测试，没有任何按键入口；
`Action::Select` 同样无消费者。接线 `f` 键需要输入模式状态机（焦点切换、
文本缓冲、Esc/Enter 处理），远超 40 行上限，违背 KISS。已删除：
`tag_filter` 字段、`visible_rows` 中的过滤逻辑、`Action::Select` 变体、
mod.rs 的 Enter 绑定、reducer 测试 `filter_by_tag_narrows_rows`。

**偏差说明**：被删的 `filter_by_tag_narrows_rows` 是 plan 任务 12 点名要求的
测试。该测试锁的是一个永远不可能被用户触达的状态——测试通过 ≠ 功能存在。
tag 筛选能力保留在 CLI `list --tag`（有 e2e/cli_smoke 覆盖），TUI 侧作为
后续候选写入文档。

---

## 提交

```
6fbcc3e fix(cli): add 逐条落盘防部分失败脱节
af8141b fix(core): local 源 add 时重拷刷新缓存
f80a9df fix(web): update 遇损坏 config 返回 500
7af78a5 test(e2e): 补 symlink 方式全链路
ed59f1a docs: 修订 TUI/Web v0.1 范围表述 + 清理 TUI 死状态
```

## 出范围事项（最终审查明确推迟，未触碰）

update.rs symlink 名称键 commit 刷新；git.rs staging 目录清理；cache.rs
链接目录拷贝限制；web 跨网络持锁；DNS-rebinding Host 校验；add 重命名选项；
registry fsync；其余台账 deferred minors。
