#!/bin/sh
# Creates the stable self-signed code signing identity "AccRoster Local
# Signing" in your login keychain, once. scripts/bundle.sh signs every local
# build with it, so macOS sees each rebuild as the same app: Keychain "Always
# Allow" and notification permission are kept.
#
# This is a local developer identity, not distribution signing: it is not
# trusted by other Macs and is never notarized. Remove it with Keychain Access
# (login keychain, "My Certificates").
set -eu

name="${ACCROSTER_SIGNING_IDENTITY:-AccRoster Local Signing}"
keychain="$HOME/Library/Keychains/login.keychain-db"

if security find-identity -p codesigning | grep -q "\"$name\""; then
  echo "\"$name\" already exists."
  exit 0
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

cat > "$work/cert.cnf" <<EOF
[req]
distinguished_name = dn
x509_extensions = ext
prompt = no
[dn]
CN = $name
[ext]
basicConstraints = critical, CA:false
keyUsage = critical, digitalSignature
extendedKeyUsage = critical, codeSigning
EOF

# /usr/bin/openssl is LibreSSL, whose PKCS#12 files `security import` reads.
/usr/bin/openssl req -x509 -newkey rsa:2048 -nodes -days 3650 \
  -config "$work/cert.cnf" -keyout "$work/key.pem" -out "$work/cert.pem"
password="$(/usr/bin/openssl rand -hex 16)"
/usr/bin/openssl pkcs12 -export -name "$name" \
  -inkey "$work/key.pem" -in "$work/cert.pem" \
  -passout "pass:$password" -out "$work/identity.p12"
security import "$work/identity.p12" -k "$keychain" -P "$password" -T /usr/bin/codesign

echo "Created \"$name\". scripts/bundle.sh will now sign with it."
