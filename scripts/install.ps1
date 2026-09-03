# skills 安装脚本（中文交互），适用于 Windows
# 用法：powershell -ExecutionPolicy Bypass -File install.ps1 [-Dir <安装目录>]
# 默认安装到 %LOCALAPPDATA%\Programs\skills 并加入用户 PATH
# 注意：本文件必须保存为 UTF-8 with BOM，否则 Windows PowerShell 5.1 会把中文显示为乱码
param([string]$Dir = "")

$ErrorActionPreference = "Stop"

$srcDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$bin = Join-Path $srcDir "skills.exe"
if (-not (Test-Path $bin)) {
    Write-Host "错误：未在 $srcDir 找到 skills.exe"
    Write-Host "请先解压安装包，再在解压后的目录中运行本脚本"
    exit 1
}

if ($Dir -eq "") {
    $Dir = Join-Path $env:LOCALAPPDATA "Programs\skills"
}

Write-Host "正在安装 skills 到 $Dir ..."
New-Item -ItemType Directory -Force -Path $Dir | Out-Null
Copy-Item $bin (Join-Path $Dir "skills.exe") -Force
Write-Host "安装完成：$Dir\skills.exe"

# 加入用户 PATH（已包含则跳过）
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
if (-not $userPath) { $userPath = "" }
if ($userPath -notlike "*$Dir*") {
    [Environment]::SetEnvironmentVariable("Path", "$userPath;$Dir", "User")
    Write-Host "已将 $Dir 加入用户 PATH（重新打开终端后生效）"
}

Write-Host ""
Write-Host "验证安装："
& (Join-Path $Dir "skills.exe") --version
Write-Host "可以使用了，运行 skills --help 查看帮助"
