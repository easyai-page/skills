# Task 8 报告：删除引擎

## 实现

- 新建 `src/core/remove.rs`，提供 `RemoveOutcome::{Removed, RecordOnly}` 和 `remove_install`。
- 删除前先按技能名与目标查找 Registry 记录。
- 记录存在但目标磁盘路径不存在时，仅移除 Registry 记录并返回 `RecordOnly`。
- `Method::Copy` 仅删除确认是普通目录的目标。
- `Method::Symlink` 仅删除确认是符号链接的入口，并核对链接目标是否为记录对应的缓存路径。
- 记录与磁盘实况不一致时返回 `Error::Mismatch`，保留磁盘内容与 Registry 记录。
- 通过 `symlink_metadata` 检查入口，删除链接本身，不跟随链接进入缓存。
- 在 `src/core/mod.rs` 声明 `remove` 模块，并在 `src/core/error.rs` 增加 `Mismatch` 错误。

## 测试

按 TDD 执行：

1. 先加入内联测试并运行 `cargo test remove`，确认因删除 API 和 `Error::Mismatch` 尚未实现而按预期编译失败。
2. 加入最小实现后运行 `cargo test remove`，6 个测试通过。
3. 运行 `cargo fmt -- --check`，通过。
4. 运行 `git diff --check`，通过。
5. 运行 `cargo test`，53 个测试通过，0 个失败。

测试覆盖：复制安装删除、符号链接删除且缓存保留、磁盘已手动删除时只清理记录、未知安装、符号链接记录与普通目录实况不匹配时拒绝删除。

## 审查备注

代码审查指出了简报给定实现边界之外的潜在增强项，包括对异常 `metadata` 错误、恶意 Registry 中的技能路径以及 Windows junction 的额外覆盖。本任务按简报要求的签名、行为和实现保持范围不扩展；当前平台测试覆盖 Unix 符号链接路径。

## 结果

任务 8 实现完成，已提交。
