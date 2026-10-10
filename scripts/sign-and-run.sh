#!/bin/bash
# cargo 的 runner（见 .cargo/config.toml）：`tauri dev` 每次编译出新的二进制后，先用固定证书签名再运行。
#
# 为什么：macOS 按「代码签名身份」记住文件夹访问授权。没有固定证书时，每次编译的签名都不同，
# 授权就失效，反复弹「DevBox 想访问“文稿”文件夹」。用固定证书签名后，身份不变，授权一直有效。
#
# 没有创建证书、不是 macOS、签名失败，都只是跳过，原样运行，不影响开发。
# 证书创建方法见 README「固定签名，消除文件夹权限弹窗」。
set -u
bin="$1"
identity="${DEVBOX_SIGN_IDENTITY:-DevBox Local Signing}"

# 只签应用本体（不签 devbox_lib 的测试二进制，省时间）
if [ "$(uname)" = "Darwin" ] && [ "$(basename "$bin")" = "devbox" ]; then
  if security find-identity -v -p codesigning 2>/dev/null | grep -qF "\"$identity\""; then
    if ! codesign --force --sign "$identity" --identifier dev.devbox.app "$bin" 2>/dev/null; then
      echo "[sign-and-run] 用「$identity」签名失败，按未签名运行（授权弹窗可能再次出现）" >&2
    fi
  else
    echo "[sign-and-run] 没找到代码签名证书「$identity」，按未签名运行；创建方法见 README「固定签名」" >&2
  fi
fi

exec "$@"
