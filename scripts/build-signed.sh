#!/bin/bash
# 本机打包并用固定证书签名：重新构建、升级版本后，文件夹访问授权仍然有效，不会再反复弹窗。
# 用法：scripts/build-signed.sh            # 默认证书名「DevBox Local Signing」
#       DEVBOX_SIGN_IDENTITY="我的证书" scripts/build-signed.sh
# 证书创建方法见 README「固定签名，消除文件夹权限弹窗」。
set -euo pipefail
cd "$(dirname "$0")/.."

if [ "$(uname)" != "Darwin" ]; then
  echo "这个脚本只在 macOS 上使用" >&2
  exit 1
fi

identity="${DEVBOX_SIGN_IDENTITY:-DevBox Local Signing}"
if ! security find-identity -v -p codesigning | grep -qF "\"$identity\""; then
  echo "没找到可用的代码签名证书「$identity」。" >&2
  echo "请先按 README「固定签名，消除文件夹权限弹窗」创建并信任证书，然后确认下面命令能看到它：" >&2
  echo "    security find-identity -v -p codesigning" >&2
  exit 1
fi

echo "==> 用「$identity」签名并打包"
APPLE_SIGNING_IDENTITY="$identity" npm run tauri -- build --bundles app,dmg

app=$(ls -d src-tauri/target/release/bundle/macos/*.app | head -1)
echo
echo "==> 签名信息（应该是 certificate leaf = H\"…\"，而不是 cdhash H\"…\"）"
codesign -dr - "$app" 2>&1 | grep -i "designated" || true
echo
echo "产物："
echo "  $app"
ls src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null | sed 's/^/  /'
echo
echo "第一次安装这个签名的版本会再弹一次，点「允许」；之后重新构建、升级都不会再弹。"
