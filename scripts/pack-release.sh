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
