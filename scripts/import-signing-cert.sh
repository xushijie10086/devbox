#!/bin/bash
# CI 用：把 GitHub Secrets 里的 .p12 证书导入临时钥匙串，设为受信任，供 codesign / tauri build 使用。
#
# 为什么：macOS 按「代码签名身份」记住文件夹授权。官方发布包每个版本都用同一张证书签名，
# 所有用户升级后就不会再弹「DevBox 想访问“文稿”文件夹」。证书从本机导出，见 README「发布包也固定签名」。
#
# 需要的环境变量：
#   SIGNING_CERT_P12        .p12 文件的 base64
#   SIGNING_CERT_PASSWORD   导出 .p12 时设的密码
# 成功后把证书名写进 $GITHUB_ENV 的 APPLE_SIGNING_IDENTITY（tauri build 会读它）。
set -euo pipefail

if [ "$(uname)" != "Darwin" ]; then
  echo "这个脚本只在 macOS 上使用" >&2
  exit 1
fi
if [ -z "${CI:-}" ]; then
  echo "这个脚本会改系统的证书信任设置，只给 CI 用；本机请用 scripts/create-signing-cert.sh" >&2
  exit 1
fi
: "${SIGNING_CERT_P12:?缺少 SIGNING_CERT_P12}"
: "${SIGNING_CERT_PASSWORD:?缺少 SIGNING_CERT_PASSWORD}"
echo "::add-mask::${SIGNING_CERT_PASSWORD}"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

# 一律用系统自带的 openssl（与创建证书时一致）
printf '%s' "$SIGNING_CERT_P12" | /usr/bin/openssl base64 -d -A -out "$tmp/id.p12"
if ! /usr/bin/openssl pkcs12 -in "$tmp/id.p12" -clcerts -nokeys -passin env:SIGNING_CERT_PASSWORD -out "$tmp/cert.pem" 2>/dev/null; then
  echo "::error::读不出 SIGNING_CERT_P12：请确认它是 .p12 文件的 base64（base64 -i 文件.p12），并且 SIGNING_CERT_PASSWORD 是导出时设的密码" >&2
  exit 1
fi

kc="${RUNNER_TEMP:-$tmp}/devbox-release.keychain-db"
pw=$(uuidgen)
echo "::add-mask::${pw}"
security create-keychain -p "$pw" "$kc"
security set-keychain-settings -lut 21600 "$kc"
security unlock-keychain -p "$pw" "$kc"
# 放到搜索列表最前，codesign 才找得到；不动 runner 自带的钥匙串
# shellcheck disable=SC2046
security list-keychains -d user -s "$kc" $(security list-keychains -d user | tr -d '"')

security import "$tmp/id.p12" -k "$kc" -P "$SIGNING_CERT_PASSWORD" -T /usr/bin/codesign >/dev/null
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$pw" "$kc" >/dev/null
# CI 弹不出授权框，信任写到系统域
sudo security add-trusted-cert -d -r trustRoot -p codeSign -k /Library/Keychains/System.keychain "$tmp/cert.pem"

identity=$(security find-identity -v -p codesigning "$kc" | grep -o '"[^"]*"' | head -1 | tr -d '"')
if [ -z "$identity" ]; then
  echo "::error::证书导入了，但没有变成可用于代码签名的有效身份（需要证书带「代码签名」用途且私钥在 .p12 里）" >&2
  security find-identity -p codesigning "$kc" >&2 || true
  exit 1
fi

echo "已导入签名身份：${identity}"
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "APPLE_SIGNING_IDENTITY=${identity}" >> "$GITHUB_ENV"
fi
