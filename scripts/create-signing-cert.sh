#!/bin/bash
# 一条命令创建「代码签名」自签名证书并设为受信任，用来给 DevBox 固定签名。
#
# 为什么需要：macOS 按「代码签名身份」记住「允许访问文稿文件夹」。没有固定证书时，每次重新编译、
# 升级版本签名都会变，授权作废，于是反复弹窗。用同一个证书签名，身份就不变。
#
# 用法：scripts/create-signing-cert.sh
#   DEVBOX_SIGN_IDENTITY="证书名"   自定义证书名（默认 DevBox Local Signing）
#   已经有可用证书会直接退出，可以放心重复运行。
#
# 运行时 macOS 会弹两次授权，都是正常的：
#   1. 「把证书设为受信任」要你输入登录密码或按 Touch ID
#   2. 第一次用它签名时，提示「codesign 想使用钥匙串中的密钥」，点「始终允许」
set -euo pipefail

NAME="${DEVBOX_SIGN_IDENTITY:-DevBox Local Signing}"

if [ "$(uname)" != "Darwin" ]; then
  echo "这个脚本只在 macOS 上使用" >&2
  exit 1
fi

# 默认钥匙串（一般是登录钥匙串）
KEYCHAIN="${KEYCHAIN:-$(security default-keychain -d user | tr -d ' "')}"

has_identity() {
  security find-identity -v -p codesigning "$KEYCHAIN" 2>/dev/null | grep -qF "\"$NAME\""
}

if has_identity; then
  echo "已经有可用的代码签名证书「$NAME」，无需重复创建。"
  exit 0
fi

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

echo "==> 生成证书「$NAME」（有效期 10 年）"
cat > "$tmp/openssl.cnf" <<CNF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $NAME
[ext]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
CNF
# 一律用系统自带的 openssl：Homebrew 的 openssl@3 导出的 .p12 用了新算法，macOS 的 security 导入会报 MAC verification failed
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -config "$tmp/openssl.cnf" -keyout "$tmp/key.pem" -out "$tmp/cert.pem" 2>/dev/null
pass=$(uuidgen)
/usr/bin/openssl pkcs12 -export -inkey "$tmp/key.pem" -in "$tmp/cert.pem" \
  -name "$NAME" -out "$tmp/id.p12" -passout "pass:$pass"

echo "==> 导入钥匙串：$KEYCHAIN"
security import "$tmp/id.p12" -k "$KEYCHAIN" -P "$pass" -T /usr/bin/codesign >/dev/null
# 给出钥匙串密码就一并放行 codesign（CI 用）；没给就由第一次签名时的弹窗「始终允许」
if [ -n "${KEYCHAIN_PASSWORD:-}" ]; then
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$KEYCHAIN_PASSWORD" "$KEYCHAIN" >/dev/null
fi

echo "==> 设为受信任（用于代码签名）"
if [ -n "${CI_TRUST_ADMIN:-}" ]; then
  # 无人值守环境（CI）弹不出授权框，改写到系统域
  sudo security add-trusted-cert -d -r trustRoot -p codeSign -k /Library/Keychains/System.keychain "$tmp/cert.pem"
else
  security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$tmp/cert.pem"
fi

if has_identity; then
  echo
  echo "完成。现在这条命令应该能看到「$NAME」："
  echo "    security find-identity -v -p codesigning"
  echo "之后：开发模式（npm run tauri dev）会自动用它签名；打包用 scripts/build-signed.sh。"
  echo "切换后第一次运行 DevBox 会再弹一次文件夹授权，点「允许」，以后就不会再弹了。"
else
  echo >&2
  echo "证书已导入，但还没变成「有效」状态。请手动在「钥匙串访问」里双击「$NAME」→ 展开「信任」→「代码签名」选「始终信任」，然后再运行：" >&2
  echo "    security find-identity -v -p codesigning" >&2
  exit 1
fi
