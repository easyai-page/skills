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
