#!/usr/bin/env bash
# Cert factory for TLS failure tests (TST-API-055). Usage: certs.sh <out-dir>
# Writes: ca.{pem,key} (trusted), other-ca.{pem,key} (unknown CA), and server certs signed by
# ca: ok (SAN DNS:localhost + IP:127.0.0.1), expired, future (not yet valid), wrongname
# (SAN DNS:other.example only), iponly (SAN IP:127.0.0.1 only), unknown (signed by other-ca);
# client.{pem,key}, client-enc.key (PKCS#8 encrypted, pass "clientpass"), client-legacy.key
# (traditional "Proc-Type: 4,ENCRYPTED" PEM, pass "clientpass").
set -euo pipefail
out="${1:?out dir}"
mkdir -p "$out"
cd "$out"
q() { "$@" >/dev/null 2>&1; }
ca() { # name subj
  q openssl req -x509 -newkey rsa:2048 -nodes -keyout "$1.key" -out "$1.pem" -days 30 \
    -subj "/CN=$2" -addext basicConstraints=critical,CA:TRUE -addext keyUsage=critical,keyCertSign,cRLSign
}
leaf() { # name san signer not_before not_after [eku]
  local name=$1 san=$2 signer=$3 nb=$4 na=$5 eku=${6:-serverAuth}
  q openssl req -newkey rsa:2048 -nodes -keyout "$name.key" -out "$name.csr" -subj "/CN=$name"
  printf 'basicConstraints=CA:FALSE\nextendedKeyUsage=%s\n%s\n' "$eku" "${san:+subjectAltName=$san}" > "$name.ext"
  q openssl x509 -req -in "$name.csr" -CA "$signer.pem" -CAkey "$signer.key" -CAcreateserial \
    -out "$name.pem" -extfile "$name.ext" -not_before "$nb" -not_after "$na"
  rm -f "$name.csr" "$name.ext"
}
past=20200101000000Z; past2=20200201000000Z
future=20990101000000Z; future2=20990201000000Z
now=$(date -u -d '-1 day' +%Y%m%d%H%M%SZ); later=$(date -u -d '+30 days' +%Y%m%d%H%M%SZ)
ca ca "IT Test CA"
ca other-ca "Unknown CA"
leaf ok "DNS:localhost,IP:127.0.0.1" ca "$now" "$later"
leaf expired "DNS:localhost,IP:127.0.0.1" ca "$past" "$past2"
leaf future "DNS:localhost,IP:127.0.0.1" ca "$future" "$future2"
leaf wrongname "DNS:other.example" ca "$now" "$later"
leaf iponly "IP:127.0.0.1" ca "$now" "$later"
leaf unknown "DNS:localhost,IP:127.0.0.1" other-ca "$now" "$later"
leaf client "" ca "$now" "$later" clientAuth
q openssl pkcs8 -topk8 -in client.key -out client-enc.key -v2 aes-256-cbc -passout pass:clientpass
q openssl rsa -in client.key -out client-legacy.key -aes256 -traditional -passout pass:clientpass
rm -f ./*.srl
echo "$out"
