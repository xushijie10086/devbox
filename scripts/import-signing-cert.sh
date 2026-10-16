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

# base64 还原成 .p12（macOS 自带的 openssl 只用来做 base64，不解析 .p12：
# 钥匙串导出的 .p12 用的加密方式，LibreSSL 不一定读得出来，抽出来的证书可能是空的）
printf '%s' "$SIGNING_CERT_P12" | /usr/bin/openssl base64 -d -A -out "$tmp/id.p12"
if [ ! -s "$tmp/id.p12" ]; then
  echo "::error::SIGNING_CERT_P12 不是有效的 base64：请用 base64 -i 文件.p12 | pbcopy 重新生成并粘贴" >&2
  exit 1
fi

kc="${RUNNER_TEMP:-$tmp}/devbox-release.keychain-db"
pw=$(uuidgen)
echo "::add-mask::${pw}"
# 同名钥匙串已存在（同一台机器上重复运行）时先删掉，免得 create-keychain 报「已存在」
security delete-keychain "$kc" >/dev/null 2>&1 || true
security create-keychain -p "$pw" "$kc"
security set-keychain-settings -lut 21600 "$kc"
security unlock-keychain -p "$pw" "$kc"
# 放到搜索列表最前，codesign 才找得到；不动 runner 自带的钥匙串
# shellcheck disable=SC2046
security list-keychains -d user -s "$kc" $(security list-keychains -d user | tr -d '"')

# 交给 macOS 自己导入（和在本机双击 .p12 一样）；密码不对、文件损坏都会在这里报错
if ! security import "$tmp/id.p12" -k "$kc" -P "$SIGNING_CERT_PASSWORD" -T /usr/bin/codesign >"$tmp/import.log" 2>&1; then
  echo "::error::导入 .p12 失败：密码不对，或文件不是从钥匙串导出的 .p12。系统的报错：$(tr '\n' ' ' < "$tmp/import.log")" >&2
  exit 1
fi
security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$pw" "$kc" >/dev/null

# 证书名和证书本身都问钥匙串要（不看是否「有效」：此时还没设信任，所以不加 -v）
identity=$(security find-identity -p codesigning "$kc" | grep -o '"[^"]*"' | head -1 | tr -d '"' || true)
if [ -z "$identity" ]; then
  echo "::error::.p12 里没有「带私钥、可用于代码签名」的证书。请确认导出的是 DevBox Local Signing 这一张（展开能看到下面有私钥），不是只导出了证书" >&2
  security find-identity -p codesigning "$kc" >&2 || true
  exit 1
fi
security find-certificate -c "$identity" -p "$kc" > "$tmp/cert.pem"
if ! grep -q "BEGIN CERTIFICATE" "$tmp/cert.pem"; then
  echo "::error::从钥匙串里取不出证书「${identity}」" >&2
  exit 1
fi
# CI 弹不出授权框，信任写到系统域
sudo security add-trusted-cert -d -r trustRoot -p codeSign -k /Library/Keychains/System.keychain "$tmp/cert.pem"

if ! security find-identity -v -p codesigning "$kc" | grep -qF "\"$identity\""; then
  echo "::error::证书导入了，但没有变成可用于代码签名的有效身份" >&2
  security find-identity -p codesigning "$kc" >&2 || true
  exit 1
fi

echo "已导入签名身份：${identity}"
if [ -n "${GITHUB_ENV:-}" ]; then
  echo "APPLE_SIGNING_IDENTITY=${identity}" >> "$GITHUB_ENV"
fi
