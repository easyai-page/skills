# 技能发现：递归扫描 + 目录剪枝设计

日期：2026-10-10
状态：已确认（用户已批准方案 A：递归扫描 + 目录剪枝）

## 背景与目标

安装 `mattpocock/skills` 时提示「没有技能」。根因：该仓库的技能全部位于三层目录（`skills/<分类>/<技能名>/SKILL.md`，共 38 个，分 engineering / productivity / in-progress / misc 四类），而 `scan_skills`（`src/core/cache.rs`）只扫描根级或最多两层目录内的 `SKILL.md`，第三层不进入，扫描结果为空。

该仓库虽带有 `.claude-plugin/plugin.json` 显式列出 27 个技能路径，但现有代码仅将清单用于**分组**（`load_plugin_groupings`），不用于**发现**。

目标：技能发现支持任意目录深度，通用覆盖分类目录结构，同时不引入误报、不改变既有行为语义。

## 方案决策

已评估三个方案：

- **A. 递归扫描 + 目录剪枝（采纳）**：整树遍历，发现 `SKILL.md` 即剪枝。通用、任何深度可发现。
- B. manifest 驱动发现：仅按 `.claude-plugin` 清单声明的路径发现。精确但无清单的深层仓库仍不支持，且依赖上游清单质量（mattpocock 的 plugin.json 就漏列了 in-progress/misc 共 11 个技能）。
- C. 固定加深到三层：治标不治本。

## 设计

改动集中在 `src/core/cache.rs` 的 `scan_skills`，`SkillEntry`、`read_entry`、分组逻辑与函数签名均不变。

### 发现算法

1. 根级 `SKILL.md` 优先判断保持不变（单技能仓库直接返回）。
2. 否则从根递归遍历目录树：
   - 进入每个子目录前，若该目录含 `SKILL.md`，调 `read_entry` 记录后**不再深入其子目录**——技能目录内部不允许嵌套技能，其中的 `SKILL.md` 视为素材/示例，剪枝避免误报。
   - frontmatter 非法时照常警告跳过，但**仍然剪枝**——该文件已声明此目录是技能目录。
   - 跳过所有 `.` 开头的隐藏目录（`.git`、`.agents`、`.claude-plugin` 等），比现状只跳过 `.git` 更严格，避免扫进插件清单与变更记录目录。
   - 用 `DirEntry::file_type()` 判断目录，不跟随符号链接；另设深度上限（8 层）兜底，防链接环或病态仓库。
3. 每层遍历按文件名排序；最终结果仍按 `name → rel_path` 排序（现有收尾逻辑不动），输出确定性有保证。

### 分组逻辑：不变

`load_plugin_groupings` 与分类推断保持原样。现有代码中 `comps[0] == "skills"` 时取第二级做分类的分支本就为 `skills/<分类>/<名>` 结构预备，mattpocock 仓库扫描出来后自然按分类分组；其有 marketplace.json + plugin.json 时按现有清单分组语义处理（清单列到的归清单组，未列到的归 "Other"）。

### 错误处理

`read_dir` / 读文件错误照旧向上传播，不新增错误类型。

### 影响面

`scan_skills` 签名不变，add / update / wizard / Web 各调用方零改动。

## 测试

沿用现有惯例（就地单测 + `tests/` 集成测试）：

- 三层结构（`skills/cat/name/SKILL.md`）能被全部发现；
- 技能目录内嵌套的 `SKILL.md` 被剪枝，不重复计数；
- 隐藏目录内的 `SKILL.md` 被跳过；
- 符号链接不跟随（构造链接环也不挂）；
- 既有单测（两层、根级、非法 frontmatter 跳过、`.git` 跳过等）保持绿色。
