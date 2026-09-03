#!/usr/bin/env bash
# skills 安装脚本（中文交互），适用于 Linux / macOS
# 用法：bash install.sh [--dir <安装目录>]
# 默认安装到 /usr/local/bin（无写权限时回退到 ~/.local/bin）
set -euo pipefail

install_dir=""
while [ $# -gt 0 ]; do
  case "$1" in
    --dir)
      install_dir="${2:?--dir 后面需要跟安装目录}"
      shift 2
      ;;
    -h|--help)
      echo "用法：bash install.sh [--dir <安装目录>]"
      echo "  默认安装到 /usr/local/bin，无写权限时回退到 ~/.local/bin"
      exit 0
      ;;
    *)
      echo "未知参数：$1（用 --help 查看用法）" >&2
      exit 1
      ;;
  esac
done

# 允许从任意目录调用：二进制与本脚本在同一目录
src_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
bin="$src_dir/skills"
if [ ! -f "$bin" ]; then
  echo "错误：未在 $src_dir 找到 skills 二进制文件" >&2
  echo "请先解压安装包，再在解压后的目录中运行本脚本" >&2
  exit 1
fi

if [ -z "$install_dir" ]; then
  if [ -w /usr/local/bin ]; then
    install_dir=/usr/local/bin
  else
    install_dir="$HOME/.local/bin"
  fi
fi

echo "正在安装 skills 到 $install_dir ..."
mkdir -p "$install_dir"
cp "$bin" "$install_dir/skills"
chmod +x "$install_dir/skills"

# macOS 从浏览器下载的文件带隔离属性，不移除会被 Gatekeeper 拦截
if [ "$(uname -s)" = "Darwin" ]; then
  xattr -d com.apple.quarantine "$install_dir/skills" 2>/dev/null || true
fi

echo "安装完成：$install_dir/skills"

case ":$PATH:" in
  *":$install_dir:"*) ;;
  *)
    echo ""
    echo "提示：$install_dir 不在 PATH 环境变量中"
    echo "请将下面这行加入 ~/.bashrc 或 ~/.zshrc，然后重开终端："
    echo "  export PATH=\"$install_dir:\$PATH\""
    ;;
esac

echo ""
echo "验证安装："
"$install_dir/skills" --version && echo "可以使用了，运行 skills --help 查看帮助"
