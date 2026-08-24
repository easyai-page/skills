# Release 流水线设计：tag 触发多平台自动编译发布

> 日期：2026-08-24 · 状态：已批准（方案 A：手写 workflow）
> 关联：本仓库 spec `2026-08-20-skills-manager-design.md`（产品本体）

## 目标

推 `v*` tag 到 GitHub 时，自动为 6 个目标平台编译 `skills` 二进制并发布 GitHub Release，全程无人值守；版本号不一致、任一平台编译失败时不产出残缺 release。

## 触发与版本一致性

- 文件：`.github/workflows/release.yml`，与现有 `ci.yml` 并列、互不干扰（ci.yml 不改；tag push 顺带触发 ci.yml 测试，无害）。
- 触发：`on: push: tags: ['v*']`。
- `verify` job：取 `github.ref_name` 去掉 `v` 前缀，与 Cargo.toml 的 `version` 字段比对，不一致即 fail。目的：tag 与包版本漂移（如打 v0.2.0 但 Cargo.toml 还是 0.1.0）在编译前拦截，零资源浪费。
- 权限：`permissions: contents: write`（创建 release 的最小权限）。

## 构建矩阵（`build` job，needs: verify）

matrix.include 逐项指定 runner 与构建方式，6 个目标：

| target | runner | 构建方式 | 产物归档 |
|---|---|---|---|
| x86_64-unknown-linux-gnu | ubuntu-latest | 原生 `cargo build --release --target` | tar.gz |
| x86_64-unknown-linux-musl | ubuntu-latest | `cross build --release --target`（容器） | tar.gz |
| aarch64-unknown-linux-gnu | ubuntu-latest | `cross build --release --target`（容器） | tar.gz |
| x86_64-apple-darwin | macos-latest | 同机交叉：`rustup target add` + `cargo build` | tar.gz |
| aarch64-apple-darwin | macos-latest | 原生 | tar.gz |
| x86_64-pc-windows-msvc | windows-latest | 原生 | zip |

决策依据：

- linux 两个非原生目标统一走 `cross`（docker 容器含完整交叉工具链），不依赖 GitHub arm runner 的仓库可见性限制，ring/rustls 等含 C 依赖的编译在容器内无忧。`cross` 按需 `cargo install cross`，只给这两个目标。
- macOS 双架构在同一台 macos-latest（Apple Silicon）完成：aarch64 原生，x86_64 用 rustup target 同机交叉（macOS 官方支持的成熟路径）。
- Windows 原生编译，避免 msvc 交叉的复杂性。

## 打包与发布

- 归档命名：`skills-<version>-<target>.<ext>`，内含单个二进制 `skills`（Windows 为 `skills.exe`）。`<version>` 为去 `v` 前缀的 tag 版本号。
- 各 build job 用 `actions/upload-artifact` 上传归档。
- `release` job（needs: build，6 个目标全部成功才执行）：
  1. 下载全部 artifact 到 `dist/`
  2. 生成 `SHA256SUMS`（覆盖全部 6 个归档，sha256sum 输出格式）
  3. `gh release create <tag> --generate-notes dist/*` 一步发布正式 release（非 draft），notes 由 GitHub 从提交记录自动生成
- 发布工具用 runner 预装的 `gh` CLI + 自动 `GITHUB_TOKEN`，不引入第三方 action（减少供应链信任面）。

## 错误处理

| 失败点 | 行为 |
|---|---|
| tag 版本 ≠ Cargo.toml version | verify fail，不编译 |
| 任一 target 编译失败 | release job 不执行，无半截 release |
| 同名 tag 的 release 已存在 | gh 报错；处理路径：GitHub 网页删除该 release 后重跑 workflow，或删 tag 重打 |

## 验证方式

GitHub Actions 无法本地全真验证，三级：

1. 本地 `actionlint` 静态校验 workflow 语法（机器无此工具则跳过，靠下一级）。
2. 合并后打 `v0.1.0` tag 实跑一次——首个 release 即冒烟测试，检查 6 个归档 + SHA256SUMS + notes。
3. workflow 内所有 shell 步骤统一 `shell: bash`（含 Windows runner），避免 cmd/pwsh 分叉。

## 范围外（后续候选）

install.sh 一键安装脚本、Homebrew tap、release-plz 版本自动化、CHANGELOG.md 维护。触发条件：出现真实用户需求或发布频率上升到手动版本管理成为负担。

## Workflow 骨架（实现基准，允许微调细节）

```yaml
name: release
on:
  push:
    tags: ['v*']

permissions:
  contents: write

jobs:
  verify:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v4
      - name: tag 版本与 Cargo.toml 一致性
        run: |
          tag="${GITHUB_REF_NAME#v}"
          pkg=$(grep -m1 '^version' Cargo.toml | cut -d'"' -f2)
          [ "$tag" = "$pkg" ] || { echo "tag $tag != Cargo.toml $pkg"; exit 1; }

  build:
    needs: verify
    strategy:
      matrix:
        include:
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
      - if: matrix.tool == 'cross'
        run: cargo install cross --locked
      - name: 编译
        shell: bash
        run: ${{ matrix.tool }} build --release --target ${{ matrix.target }}
      - name: 打包
        shell: bash
        run: |
          ver="${GITHUB_REF_NAME#v}"
          name="skills-$ver-${{ matrix.target }}"
          stage="stage/$name"
          mkdir -p "$stage" dist
          if [ "$RUNNER_OS" = "Windows" ]; then
            cp "target/${{ matrix.target }}/release/skills.exe" "$stage/"
            (cd stage && zip -r "../dist/$name.zip" "$name")
          else
            cp "target/${{ matrix.target }}/release/skills" "$stage/"
            tar -czf "dist/$name.tar.gz" -C stage "$name"
          fi
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
          pattern: '*'
          path: dist
          merge-multiple: true
      - name: 生成校验和并发布
        run: |
          cd dist
          sha256sum *.tar.gz *.zip > SHA256SUMS
          gh release create "$GITHUB_REF_NAME" --generate-notes ./*
        env:
          GH_TOKEN: ${{ secrets.GITHUB_TOKEN }}
```
