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

---

## 第 1 轮修复（审查必须修复项）

提交：`569a213 fix(core): 加固删除引擎（错误细分+副本所有权标识+junction+记录校验）`

### 修复内容

1. **symlink_metadata 错误细分**：仅 `ErrorKind::NotFound` 返回 `RecordOnly` 并清记录；权限/I/O 等其他错误返回 `Error::Io`，记录与磁盘均保留。回归测试用文件充当 target 根目录制造 ENOTDIR 验证（跨平台可复现，不依赖 root 下失效的 chmod 权限位）。
2. **copy 副本所有权标识**：采用 `.skills-manifest`（而非 `.skills-managed`），与计划任务 15 的约定同名兼容——任务 15 将扩展该文件写入文件名+sha256 清单，本修复写入 `{ "version": 1, "manager": "skills" }` 作为标识。install 在暂存目录写入、随 rename 原子生效；remove 仅当副本内标识为文件时才 `remove_dir_all`，否则 `Error::Mismatch` 并保留目录与记录。**兼容性说明**：修复前安装的旧副本无标识，remove 将拒绝删除并提示 Mismatch，属安全方向的有意行为。
3. **Windows junction**：`#[cfg(windows)]` 下经 `junction::exists` 识别挂载点，`junction::get_target` 取目标并 canonicalize 规范化后比较（容忍 `\\?\` 前缀）；删除用 `std::fs::remove_dir`（RemoveDirectory 只删 reparse point 本身，不递归缓存源，目录软链接同理由 remove_dir 处理）。junction API 签名已对 docs.rs 核实。**验证策略**：本机仅 Linux 工具链，无法创建 junction；junction 相关代码全部 cfg(windows) 隔离，Linux 编译与全量测试通过，Windows 行为依赖任务 14 的三平台 CI 验证。
4. **删除前记录校验**：复用 `install::validate_skill_name`（提升为 `pub(crate)`，无第二套逻辑）校验技能名为单一 Normal 组件；`TargetRec::Project.root` 必须为绝对路径。非法记录返回 `Error::Mismatch`（记录损坏），在执行任何磁盘操作前拒绝。

### 测试

新增：install 2 项（copy 写入标识、symlink 不写标识），remove 4 项（外部目录替换副本→Mismatch、metadata I/O 错误→记录保留、非法技能名记录→不删磁盘、相对 project root 记录→拒绝）。

- `cargo test remove`：10 passed
- `cargo test install`：18 passed
- `cargo test`：59 passed, 0 failed
- `cargo fmt -- --check`、`git diff --check` 通过
