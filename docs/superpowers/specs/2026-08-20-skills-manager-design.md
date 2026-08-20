# skills-manager 设计文档

日期：2026-08-20
状态：已批准

## 概述

一个跨平台（Windows / macOS / Linux）的技能管理工具，提供 CLI + TUI + 本地 Web 管理页三种界面，参考 vercel-labs/skills 但聚焦单机技能资产管理：技能包下载缓存、多目标安装（复制 / 软连接）、安装记录、分类管理、两级自动更新策略。

技术栈：**Rust**。git 操作用 gitoxide（纯 Rust 实现，不依赖系统 git），TUI 用 ratatui，Web 用 axum，CLI 用 clap。单二进制分发。

## 核心概念

- **技能（skill）**：含 SKILL.md 的目录，名称取自 frontmatter。
- **技能包（source）**：一个 git 仓库或本地目录，可包含多个技能。下载缓存到 `~/.skills/`，按 `<host>/<owner>/<repo>` 组织（GitHub → `github/` 子目录；本地路径来源 → `local/`）。
- **安装（install）**：一条「技能 × 目标」记录。同一技能可装到多个目标，同一技能包可只装其中几个技能。
- **目标（target）**：内置预置 `agents`（`~/.agents/skills`）、`claude`（`~/.claude/skills`）、`codex`（`~/.codex/skills`）等全局目标；项目级目标固定为 `./.agents/skills`（按项目根绝对路径标识）。可通过 config.toml 扩展自定义目标。
- **安装方式**：`symlink`（指向缓存，默认）或 `copy`（独立副本）。每条 install 记录记录所用方式。

## 磁盘布局

```
~/.skills/
├── github/<owner>/<repo>/     # 技能包缓存（git 浅克隆）
├── <host>/<owner>/<repo>/     # 其他 git 来源
├── local/<name>/              # 本地路径来源
├── config.toml                # 可选；无配置文件也能工作
└── registry.json              # 安装记录（核心数据）
```

缓存按来源去重：同一仓库已缓存则不重复下载，`add` 时复用并提示可用 `update` 更新。

## 配置体系

**内置默认值（编译进二进制）**：预置 target 列表、默认安装方式 symlink、Web 端口 7823、全局 auto_update 默认 false。

**config.toml（可选）**：只做覆盖与扩展——新增自定义 target、修改默认值。所有项可被 CLI 参数覆盖（如 `--method copy`、`skills ui --port 9000`）。

**registry.json**：

```jsonc
{
  "version": 1,
  "sources": {
    "github/mattpocock/skills": {
      "url": "https://github.com/mattpocock/skills",
      "commit": "a1b2c3d",
      "fetched_at": "2026-08-20T10:00:00Z",
      "auto_update": true            // 包级开关（可选）
    }
  },
  "installs": [
    {
      "skill": "A",
      "source": "github/mattpocock/skills",
      "source_path": "skills/A",
      "target": { "kind": "global",  "name": "agents" },       // → ~/.agents/skills/A
      //  或 { "kind": "project", "root": "/abs/path/work" }    // → work/.agents/skills/A
      "method": "copy",
      "commit": "a1b2c3d",           // 安装时版本
      "tags": ["frontend"],          // 用户自定义分类
      "auto_update": false,          // 副本级开关（可选，缺省跟随包级）
      "installed_at": "..."
    }
  ]
}
```

registry 写入用临时文件 + rename 原子替换，避免中途失败写脏数据。

## 两级更新模型

**第 1 级 · 仓库级**：`sources[source].auto_update`（未设置用内置默认 false）控制 `git fetch + reset` 是否执行。symlink 安装只有这一层有意义——仓库更新后所有软连接自动生效，registry 只更新 commit 字段。给 symlink 记录设副本级开关无意义，CLI 会提示。

**第 2 级 · copy 副本级**：仓库更新后，逐条 method=copy 的 install 记录解析 `auto_update`（未设置则跟随所属包级），决定是否重新复制到目标目录。同一技能包里的不同技能被复制到不同项目/不同 agent 全局时，每个副本独立受控。

**手动出口**：`skills update <skill> --target <t>` 显式指定时无视配置强制更新该副本。

## 界面

三个前端共享同一个 `core` 库（source 解析、缓存、registry、安装引擎、更新引擎），前端只是薄壳。

### CLI 子命令

| 命令 | 说明 | 关键参数 |
|---|---|---|
| `skills add <source>` | 下载（已缓存则复用）+ 选择技能、目标、方式后安装 | `-s, --skill`、`-t, --target`、`-g`、`--method symlink\|copy`、`-y` |
| `skills list`（ls） | 列出已安装技能 | `--tag`、`--target`、`-g` |
| `skills remove [skills]` | 按记录删除并核实磁盘实况 | `-t`、`--tag`、`-y` |
| `skills update [skills]` | 按两级更新模型执行 | `--all`、`-t`、`-s`、`--dry-run` |
| `skills tag <skill> <tags...>` | 分类管理 | `--remove`、`-t` |
| `skills auto-update ...` | 升级策略，**只写 registry.json** | `--target`、`-s/--source`、`--on/--off/--inherit` |
| `skills config ...` | 全局配置，**只写 config.toml** | `get`/`set`/`targets add\|remove` |
| `skills tui` | 进入 TUI（裸跑 `skills` 同效） | |
| `skills ui` | 启动 Web 管理页并打开浏览器 | `--port`、`--no-open` |

命令分界原则：改 config.toml 的走 `config`，改 registry.json 的走 `auto-update` / `tag`，不混用。

**source 解析**：`owner/repo` 自动补全为 `https://github.com/owner/repo`；完整 URL、SSH URL 原样使用；本地路径映射到 `~/.skills/local/`。

### TUI（ratatui）

三个主视图 Tab 切换：

1. **已安装**：表格列出 技能 / 来源 / 目标 / 方式 / 版本 / 分类 / auto_update，支持按 tag、目标筛选；选中后可 remove、切换 auto_update、改 tag、update。
2. **安装向导**：输入 source → 拉取/复用缓存 → 多选技能 → 多选目标 → 选方式 → 摘要确认 → 执行。
3. **仓库缓存**：sources 列表、包级 auto_update 开关、检查更新、清理无引用的缓存。

### Web UI（axum + 内嵌静态前端）

REST API + 单页前端，功能与 TUI 对等（浏览/筛选/安装向导/删除/更新/分类/升级策略/配置管理）。前端资源嵌入二进制，无外部依赖。install 列表每行 auto_update 三态开关（开/关/跟随包级）；sources 页面包级开关；「偏好设置」对应 config.toml，「升级策略」对应 registry.json。

## 关键流程

**add**：解析 source → 计算缓存 key → 已缓存则复用（提示可 update）/ 未缓存则浅克隆 → 扫描 SKILL.md（支持根级单技能与 `<dir>/SKILL.md` 多技能布局）→ 多选技能 → 多选目标 → 选方式 → 摘要确认 → 执行（symlink 指向缓存 / copy 复制目录）→ 逐条写 registry。目标目录已有同名技能时逐个询问覆盖/跳过/重命名（`-y` 时默认跳过并告警）。

**remove**：registry 查记录 → 核实磁盘实况（symlink 校验指向、copy 校验目录存在且非软链）→ 存在则删除 → 移除记录。记录与磁盘不一致（用户已手动删除）时只清记录并提示。

**update**：遍历 sources 按包级配置 pull → symlink 记录更新 commit 字段 → copy 记录逐条解析副本级配置决定重复制。`--dry-run` 打印计划（哪些仓库会 pull、哪些副本更新/跳过及原因）不执行。

## 错误处理

- git 网络失败：保留旧缓存，报错退出，registry 不落盘。
- Windows symlink 权限不足：报错并建议 `--method copy` 或开启开发者模式。
- copy 副本被用户手动改过（与缓存不一致）：update 重复制前提示将覆盖本地修改；remove 不受影响。

## 跨平台

路径全部走 `std::path` + `dirs`；symlink 用 `std::os::unix` / `std::os::windows` 分别实现（Windows 目录链接用 junction 兜底）；gitoxide 无外部命令依赖；CI 跑 Linux/macOS/Windows 三平台。

## 测试策略

- core 单元测试：source 解析、registry 读写与版本迁移、两级更新优先级解析、安装记录核实逻辑。
- 集成测试：本地 bare repo 作 fixture（不依赖网络），跑 add/list/update/remove 全链路；copy 与 symlink 两种方式各跑一遍。
- GitHub Actions matrix 三平台 CI。
