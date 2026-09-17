#!/bin/bash
# stdout is the reusable certificate fingerprint; diagnostics go to stderr.
set -euo pipefail
IDENTITY_NAME='ZeroCode Local Signing'
CERTIFICATE_DAYS=3650
KEY_BITS=2048
LOCK_ATTEMPTS=30
KEYCHAIN=${ZEROCODE_SIGNING_KEYCHAIN:-$HOME/Library/Keychains/login.keychain-db}
identity() {
  security find-identity -v -p codesigning "$KEYCHAIN" |
    awk -v name="$IDENTITY_NAME" 'index($0, "\"" name "\"") { print $2; exit }'
}
found=$(identity)
if [[ $found =~ ^[A-Fa-f0-9]{40}$ ]]; then printf '%s\n' "$found"; exit; fi
# Serialise first use across worktrees. A stale lock fails closed; it must never
# cause a second certificate to replace this machine's existing TCC identity.
lock="$KEYCHAIN.zerocode-signing.lock"
acquired=0
for ((attempt=0; attempt<LOCK_ATTEMPTS; attempt++)); do
  if mkdir "$lock" 2>/dev/null; then acquired=1; break; fi
  sleep 1
done
[[ $acquired = 1 ]] || { echo 'local signing identity is busy (or its lock is stale)' >&2; exit 1; }
work=''
cleanup() { [[ -z $work ]] || rm -rf "$work"; rmdir "$lock"; }
trap cleanup EXIT
found=$(identity)
if [[ $found =~ ^[A-Fa-f0-9]{40}$ ]]; then printf '%s\n' "$found"; exit; fi
if security find-certificate -c "$IDENTITY_NAME" "$KEYCHAIN" >/dev/null 2>&1; then
  echo 'local signing certificate exists but is not usable; unlock/repair the login keychain instead of replacing it' >&2
  exit 1
fi
umask 077
work=$(mktemp -d "${TMPDIR:-/tmp}/zerocode-signing.XXXXXX")
cat > "$work/openssl.cnf" <<EOF
[req]
distinguished_name = subject
x509_extensions = signing
prompt = no
[subject]
CN = $IDENTITY_NAME
[signing]
basicConstraints = critical,CA:false
keyUsage = critical,digitalSignature
extendedKeyUsage = critical,codeSigning
EOF
# Apple's OpenSSL produces a PKCS#12 format accepted by security import.
/usr/bin/openssl req -new -newkey "rsa:$KEY_BITS" -nodes -x509 -sha256 \
  -days "$CERTIFICATE_DAYS" -config "$work/openssl.cnf" \
  -keyout "$work/key.pem" -out "$work/cert.pem" >&2
password=$(/usr/bin/openssl rand -hex 24)
/usr/bin/openssl pkcs12 -export -inkey "$work/key.pem" -in "$work/cert.pem" \
  -name "$IDENTITY_NAME" -passout "pass:$password" -out "$work/identity.p12" >&2
security import "$work/identity.p12" -k "$KEYCHAIN" -P "$password" -T /usr/bin/codesign >&2
security add-trusted-cert -r trustRoot -p codeSign -k "$KEYCHAIN" "$work/cert.pem" >&2
found=$(identity)
[[ $found =~ ^[A-Fa-f0-9]{40}$ ]] || { echo 'local signing identity unavailable after import' >&2; exit 1; }
printf '%s\n' "$found"
