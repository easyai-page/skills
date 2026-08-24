# Release 流水线实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 推 `v*` tag 到 GitHub 时自动编译 6 个目标平台的 `skills` 二进制并发布 GitHub Release（归档 + SHA256SUMS + 自动 notes）。

**Architecture:** 单个 workflow 文件 `.github/workflows/release.yml`，三个 job 串行：verify（tag 与 Cargo.toml 版本一致性闸）→ build（6 目标 matrix 并行编译打包）→ release（汇总 + 校验和 + `gh release create`）。非平凡的打包 shell 抽成已提交的 `scripts/pack-release.sh`，本地可测，workflow 保持薄壳。

**Tech Stack:** GitHub Actions（仅官方 actions/* + dtolnay/rust-toolchain，不引入第三方发布 action）、cross（linux musl/aarch64 容器交叉编译）、bash、gh CLI（runner 预装）。

**Spec:** `docs/superpowers/specs/2026-08-24-release-pipeline-design.md`

## Global Constraints

- 触发仅 `on: push: tags: ['v*']`；`permissions: contents: write`（最小权限）。
- 构建矩阵逐字为以下 6 项（target / runner / 工具）：
  - `x86_64-unknown-linux-gnu` / ubuntu-latest / cargo 原生
  - `x86_64-unknown-linux-musl` / ubuntu-latest / cross
  - `aarch64-unknown-linux-gnu` / ubuntu-latest / cross
  - `x86_64-apple-darwin` / macos-latest / cargo 同机交叉
  - `aarch64-apple-darwin` / macos-latest / cargo 原生
  - `x86_64-pc-windows-msvc` / windows-latest / cargo 原生
- 归档命名 `skills-<version>-<target>.<ext>`（version 不带 `v` 前缀）；unix 用 tar.gz，windows 用 zip；归档内含单个二进制 `skills`（或 `skills.exe`）。
- `SHA256SUMS` 一个文件覆盖全部 6 个归档，sha256sum 标准输出格式。
- 发布命令：`gh release create "$GITHUB_REF_NAME" --generate-notes ./*`（在 dist 目录内执行），`GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}`。直接发布正式 release（非 draft）。
- 版本闸：`${GITHUB_REF_NAME#v}` 必须等于 `grep -m1 '^version' Cargo.toml | cut -d'"' -f2`，不一致即 fail。
- workflow 内所有 shell 步骤一律 `shell: bash`（含 Windows runner）。
- matrix 设 `fail-fast: false`（一个平台失败不取消其他平台的诊断输出；release job 仍需全部成功才执行）。
- 第三方 action 白名单：`actions/checkout@v4`、`actions/upload-artifact@v4`、`actions/download-artifact@v4`、`dtolnay/rust-toolchain@stable`（与现有 ci.yml 一致）。发布环节只用预装 `gh` CLI。
- 不修改 `ci.yml`；bump Cargo.toml 版本号属发版操作，不在本计划内（写进 RELEASE.md 流程）。

## 文件结构

```
scripts/pack-release.sh      # 新建：打包脚本（stage → tar.gz/zip → dist/），本地可测
.gitignore                   # 修改：追加 /dist 与 /stage（打包脚本的本地输出）
.github/workflows/release.yml # 新建：verify → build(matrix) → release
RELEASE.md                   # 新建：发版流程与故障处理
tests/                       # 不动；本计划的验证是 bash dry-run + actionlint + 首个 tag 冒烟
```

**跨任务接口锚点：**

- 任务 1 产出：`scripts/pack-release.sh <target> <version>`（version 不带 v）。读 `target/<target>/release/skills[.exe]`，写 `dist/skills-<version>-<target>.tar.gz|zip`。Unix 分支用 `tar -czf`；Windows 分支用 `7z a`（windows runner 预装；git-bash 的 GNU tar 不支持 zip 输出）。
- 任务 2 消费：workflow build job 以 `bash scripts/pack-release.sh "${{ matrix.target }}" "${GITHUB_REF_NAME#v}"` 调用；artifact 名即 `${{ matrix.target }}`。

---

### 任务 1：打包脚本 scripts/pack-release.sh

**文件：**
- 创建：`scripts/pack-release.sh`
- 修改：`.gitignore`（追加两行）
- 测试：本地 bash dry-run（见步骤）

**Interfaces:**
- Consumes: 无（首个任务）
- Produces: `scripts/pack-release.sh <target> <version>` —— 任务 2 的 workflow 以 `bash scripts/pack-release.sh "${{ matrix.target }}" "${GITHUB_REF_NAME#v}"` 调用

- [ ] **步骤 1：写失败测试（本地验证脚本，临时文件不进 git）**

创建 `/tmp/test-pack.sh`：

```bash
#!/usr/bin/env bash
set -euo pipefail
cd /home/bot/.project/skills/tools

# 造假的编译输出
rm -rf target/test-triple dist stage /tmp/pack-test-out
mkdir -p target/test-triple/release
echo "fake-binary-content" > target/test-triple/release/skills

# 执行打包（unix 分支；RUNNER_OS 未设时按非 Windows 处理）
bash scripts/pack-release.sh test-triple 0.0.0-test

# 断言：归档存在、名字正确、内部路径为 <名字>/skills
test -f dist/skills-0.0.0-test-test-triple.tar.gz
tar -tzf dist/skills-0.0.0-test-test-triple.tar.gz | grep -qx "skills-0.0.0-test-test-triple/skills"

# 断言：归档内内容正确
mkdir -p /tmp/pack-test-out
tar -xzf dist/skills-0.0.0-test-test-triple.tar.gz -C /tmp/pack-test-out
grep -qx "fake-binary-content" /tmp/pack-test-out/skills-0.0.0-test-test-triple/skills

# 清理（target/ 已 gitignore，dist/stage 须清理防污染）
rm -rf target/test-triple dist stage /tmp/pack-test-out
echo "PACK-TEST PASS"
```

运行：`bash /tmp/test-pack.sh`
预期：FAIL —— `scripts/pack-release.sh: No such file or directory`（脚本尚不存在）。

- [ ] **步骤 2：实现 pack-release.sh**

```bash
#!/usr/bin/env bash
# release 打包：把 target/<target>/release/ 下的二进制打成 dist/skills-<ver>-<target>.<ext>
# 用法：bash scripts/pack-release.sh <target> <version>   （version 不带 v 前缀）
# unix → tar.gz；Windows（RUNNER_OS=Windows）→ zip（用 7z，runner 预装；
# git-bash 自带的 GNU tar 不支持 zip 输出，bsdtar 在 git-bash PATH 里不可靠）
set -euo pipefail

target="${1:?用法: pack-release.sh <target> <version>}"
ver="${2:?用法: pack-release.sh <target> <version>}"

name="skills-$ver-$target"
stage="stage/$name"
rm -rf "$stage"
mkdir -p "$stage" dist

if [ "${RUNNER_OS:-}" = "Windows" ]; then
  cp "target/$target/release/skills.exe" "$stage/"
  (cd stage && 7z a -y -bd "../dist/$name.zip" "$name" >/dev/null)
  echo "packed dist/$name.zip"
else
  cp "target/$target/release/skills" "$stage/"
  tar -czf "dist/$name.tar.gz" -C stage "$name"
  echo "packed dist/$name.tar.gz"
fi
```

同时给 `.gitignore` 追加（现有内容仅 `/target`）：

```
/dist
/stage
```

- [ ] **步骤 3：跑测试验证通过 + 静态检查**

```bash
bash /tmp/test-pack.sh          # 预期输出 PACK-TEST PASS
shellcheck scripts/pack-release.sh   # 预期无输出（全绿）
bash -n scripts/pack-release.sh      # Windows 分支本机跑不了，至少语法须过
```

预期：三条全部通过。

- [ ] **步骤 4：Commit**

```bash
git add scripts/pack-release.sh .gitignore
git commit -m "ci: release 打包脚本（tar.gz/zip 双分支，本地可测）"
```

---

### 任务 2：release.yml workflow + RELEASE.md

**文件：**
- 创建：`.github/workflows/release.yml`
- 创建：`RELEASE.md`
- 测试：actionlint + 版本闸 dry-run（见步骤）

**Interfaces:**
- Consumes: `scripts/pack-release.sh <target> <version>`（任务 1）
- Produces: 无后续任务；验收步骤（用户执行）依赖 workflow 的实际行为与 RELEASE.md 一致

- [ ] **步骤 1：写失败验证（版本闸逻辑 dry-run）**

先验证版本闸的 bash 逻辑本身（它是 workflow 里唯一有逻辑判断的内联脚本）：

```bash
cd /home/bot/.project/skills/tools
# 模拟 GITHUB_REF_NAME=v0.1.0（当前 Cargo.toml 即 0.1.0）应通过
tag="v0.1.0"; t="${tag#v}"; pkg=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
[ "$t" = "$pkg" ] && echo "GATE-OK" || echo "GATE-FAIL"
# 模拟 GITHUB_REF_NAME=v9.9.9 应判不一致
tag="v9.9.9"; t="${tag#v}"; [ "$t" = "$pkg" ] && echo "GATE-BAD" || echo "GATE-REJECT-OK"
```

预期：`GATE-OK` + `GATE-REJECT-OK`。若版本闸写法有误（如 grep 锚点没锁住行首、抓到依赖行的 version），此步立即暴露——这就是本任务的 RED。

- [ ] **步骤 2：实现 .github/workflows/release.yml**

```yaml
name: release
on:
  push:
    tags: ['v*']

permissions:
  contents: write

# 同名 tag 重复推送时串行，不取消在跑的（发布动作不可重入，宁可排队）
concurrency:
  group: release-${{ github.ref }}
  cancel-in-progress: false

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: 校验 tag 与 Cargo.toml 版本一致
        shell: bash
        run: |
          tag="${GITHUB_REF_NAME#v}"
          pkg=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
          if [ "$tag" != "$pkg" ]; then
            echo "::error::tag v$tag 与 Cargo.toml version $pkg 不一致（先 bump Cargo.toml 再打 tag）"
            exit 1
          fi
          echo "版本一致: $tag"

  build:
    needs: verify
    strategy:
      # 一个平台失败不取消其他平台——一次跑完能看到全部失败点；
      # release job needs: build，任何失败都不会产出残缺 release
      fail-fast: false
      matrix:
        include:
          # musl 与 aarch64-linux 用 cross 容器交叉（工具链完整，不依赖 arm runner 可见性）
          - { target: x86_64-unknown-linux-gnu,  os: ubuntu-latest,  tool: cargo }
          - { target: x86_64-unknown-linux-musl, os: ubuntu-latest,  tool: cross }
          - { target: aarch64-unknown-linux-gnu, os: ubuntu-latest,  tool: cross }
          - { target: x86_64-apple-darwin,       os: macos-latest,   tool: cargo }
          - { target: aarch64-apple-darwin,      os: macos-latest,   tool: cargo }
          - { target: x86_64-pc-windows-msvc,    os: windows-latest, tool: cargo }
    runs-on: ${{ matrix.os }}
    steps:
      - uses: actions/checkout@v4
      - uses: dtolnay/rust-toolchain@stable
        with:
          targets: ${{ matrix.target }}
      # cross 编译耗时约 2-4 分钟，仅两个 linux 交叉目标需要；
      # 不用第三方 install-action，保持供应链白名单最小（见 Global Constraints）
      - if: matrix.tool == 'cross'
        run: cargo install cross --locked
      - name: 编译
        shell: bash
        run: ${{ matrix.tool }} build --release --target ${{ matrix.target }}
      - name: 打包
        shell: bash
        run: bash scripts/pack-release.sh "${{ matrix.target }}" "${GITHUB_REF_NAME#v}"
      - uses: actions/upload-artifact@v4
        with:
          name: ${{ matrix.target }}
          path: dist/*

  release:
    needs: build
    runs-on: ubuntu-latest
    steps:
      - uses: actions/download-artifact@v4
        with:
          path: dist
          merge-multiple: true
      - name: 校验和并发布
        shell: bash
        run: |
          cd dist
          ls -la   # 日志留证：6 个归档齐全
          sha256sum *.tar.gz *.zip > SHA256SUMS
          gh release create "$GITHUB_REF_NAME" --generate-notes ./*
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```

- [ ] **步骤 3：actionlint 静态校验**

```bash
cd /home/bot/.project/skills/tools
if command -v actionlint >/dev/null; then
  actionlint .github/workflows/release.yml
else
  # 未安装则用 go 装（本机 go 在 /usr/local/go/bin/go）；
  # 外网拉取失败时按惯例加代理前缀重试：
  #   https_proxy=http://127.0.0.1:7897 http_proxy=http://127.0.0.1:7897 <命令>
  GOBIN=/tmp/actionlint-bin go install github.com/rhysd/actionlint/cmd/actionlint@latest \
    && /tmp/actionlint-bin/actionlint .github/workflows/release.yml
fi
```

预期：无输出（actionlint 静默即通过）。若报 `matrix` 上下文相关警告，按提示修正。

- [ ] **步骤 4：写 RELEASE.md**

```markdown
# 发版流程

## 正常发布

1. 把 `Cargo.toml` 的 `version` 改成要发的版本号（如 `0.2.0`），提交。
2. `git tag v0.2.0 && git push origin master --tags`。
3. GitHub Actions 的 `release` workflow 自动执行：
   verify（版本一致性）→ build（6 平台并行）→ release（校验和 + 发布）。
4. 在 Releases 页确认 6 个归档 + `SHA256SUMS` + 自动 notes。

## 故障处理

| 现象 | 处置 |
|---|---|
| verify 失败：tag 与 Cargo.toml 版本不一致 | 改正 Cargo.toml version 并提交；删 tag 重打：`git tag -d vX.Y.Z && git push origin :refs/tags/vX.Y.Z && git tag vX.Y.Z && git push --tags` |
| 某个平台 build 失败 | 看该 job 日志修复代码；修复提交后删 tag 重打（同上）。其他平台的成功产物不保留——release 只在 6 个全绿时产出 |
| release job 失败：同名 release 已存在 | Releases 页删除该 release（或 `gh release delete vX.Y.Z`），然后在 Actions 页对失败运行点 Re-run |
| 想废弃一个已发布的版本 | Releases 页删除 release；tag 是否保留视版本号是否还要复用 |
```

- [ ] **步骤 5：复查 workflow 与文档一致性**

人工核对（写进实现报告）：
- RELEASE.md 的「6 个归档」与 matrix 实际 6 项一致
- RELEASE.md 故障处理的路径与 workflow 实际行为一致（verify 失败信息、release 已存在时的处置）
- `grep -n "pack-release.sh" .github/workflows/release.yml` 调用形式与任务 1 的接口一致

- [ ] **步骤 6：Commit**

```bash
git add .github/workflows/release.yml RELEASE.md
git commit -m "ci: tag 触发的 6 平台 release 流水线 + 发版文档"
```

---

## 验收（用户执行，非实现任务）

本仓库当前无 git remote。实现合并后：

1. 在 GitHub 建仓并 `git remote add origin <url>`，push master（按你的规则，push 由你执行）。
2. 打 `v0.1.0` tag 推送——这就是 spec 定的冒烟测试：确认 6 个归档、SHA256SUMS、自动 notes 齐全。
3. 若某平台编译失败（最可能：macOS x86_64 交叉或 musl），把对应 job 日志带回来修。

## 自检结果

**Spec 覆盖度：**
- tag `v*` 触发、verify 版本闸、contents: write → 任务 2 ✓
- 6 目标矩阵（cross 用于 musl/aarch64-linux；macOS 同机双架构；Windows 原生）→ 任务 2 ✓
- 打包命名/归档格式/SHA256SUMS/gh 发布 → 任务 1（脚本）+ 任务 2（workflow 调用与 release job）✓
- fail-fast: false、无半截 release、bash 统一、action 白名单 → 任务 2 ✓
- 错误处理三场景文档化 → 任务 2 RELEASE.md ✓
- 验证三级（actionlint / 首 tag 冒烟 / bash 统一）→ 任务 2 步骤 3 + 验收节 ✓
- 范围外（install.sh、Homebrew、release-plz、CHANGELOG）→ 计划未包含，符合 spec ✓

**占位符扫描：** 无 TBD/TODO；所有代码块为完整可用内容。

**类型/接口一致性：** `pack-release.sh <target> <version>`（version 无 v 前缀）在任务 1 定义、任务 2 调用，两处一致；artifact 名 `${{ matrix.target }}` 与 download-artifact 的 merge-multiple 汇总匹配；`skills-<version>-<target>.<ext>` 命名在脚本、SHA256SUMS、RELEASE.md 三处一致。

**对 spec 骨架的两处微调（不改变行为）：**
1. 打包 bash 抽为 `scripts/pack-release.sh`（spec 骨架为内联 run 块）——本地可测、单一来源；workflow 变薄壳。
2. Windows zip 用 `7z` 而非 `zip`：git-bash 的 GNU tar/zip 对 zip 输出不可靠，7z 在 windows-latest 预装。
3. download-artifact 省略 spec 骨架中的 `pattern: '*'`，用默认「下载全部」+ merge-multiple（文档化行为，少一个通配歧义点）。
