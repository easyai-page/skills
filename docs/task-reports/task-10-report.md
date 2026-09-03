# 任务 10 报告：分类管理（tags）

## 状态
完成。TDD 流程：RED（编译失败）→ GREEN（3 测试通过）→ fmt → 全量测试 → 提交。

## 提交
- SHA：`9c2396a`
- 标题：`feat(core): 分类管理（按 install 粒度打标与筛选）`

## 变更文件
- 新建 `src/core/tags.rs`：`set_tags`（按 skill+target 精确定位单条 install，覆盖式写 tags；未命中返回 `Error::NotInstalled`）、`filter_by_tag`（返回命中 install 引用列表），内联 3 个测试。
- 修改 `src/core/mod.rs`：加 `pub mod tags;`（按字母序位于 source 与 update 之间）。

## 与简报的唯一偏差
测试模块内测试函数名 `filter_by_tag` 与 `use super::*` 导入的同名实现函数发生遮蔽（shadowing），导致测试体内调用解析到测试自身。修复方式：测试体内调用改写为 `super::filter_by_tag(...)`。测试名、接口签名、断言内容均未改动。

## TDD 记录
1. RED：先写测试 + `pub mod tags;`，`cargo test tags` 编译失败（`set_tags`/`filter_by_tag` 未定义，符合预期）。
2. GREEN：按简报实现两个函数后，3 个测试全部 PASS。

## 测试小结
- `cargo test tags`：3 passed / 0 failed（set_tags_on_one_install_only、filter_by_tag、set_tags_on_missing_install_errors）。
- `cargo test tag`：5 passed / 0 failed（含 2 个名称含 "tag" 的既有测试）。
- `cargo test`（全量）：77 passed / 0 failed。
- `cargo fmt` 已执行，无额外 diff 残留（仅 mod.rs 新增 1 行 + tags.rs 新文件）。

## 设计确认
tag 操作仅作用于内存中的 `Registry` 结构，落盘经 `Registry::save` 写 registry.json，与 config.toml 完全分离，符合既定设计。tags 按 install 粒度存储，同一技能在不同 target 的副本互不影响（已由测试覆盖）。
