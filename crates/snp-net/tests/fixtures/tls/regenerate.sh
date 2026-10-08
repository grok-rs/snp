#!/bin/sh
# Generates the snp-net TLS test fixtures (a CA, a server certificate for 127.0.0.1/localhost, an unrelated CA).
set -eu
out="$1"
work="$2"
rm -rf "$work"
mkdir -p "$work" "$out"
cd "$work"
cat > ca.cnf <<'EOF'
[req]
distinguished_name = dn
prompt = no
x509_extensions = v3_ca
[dn]
CN = snp-net test CA
O = snp test fixtures
[v3_ca]
basicConstraints = critical,CA:TRUE
keyUsage = critical,keyCertSign,cRLSign
subjectKeyIdentifier = hash
EOF
cat > server.cnf <<'EOF'
[req]
distinguished_name = dn
prompt = no
[dn]
CN = localhost
O = snp test fixtures
[v3_server]
basicConstraints = critical,CA:FALSE
keyUsage = critical,digitalSignature
extendedKeyUsage = serverAuth
subjectAltName = IP:127.0.0.1,DNS:localhost
subjectKeyIdentifier = hash
authorityKeyIdentifier = keyid
EOF
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out ca.key
openssl req -x509 -new -key ca.key -sha256 -days 36500 -config ca.cnf -out ca.pem
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out server.key
openssl req -new -key server.key -config server.cnf -out server.csr
openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -sha256 -days 36500 \
    -extfile server.cnf -extensions v3_server -out server.pem
openssl pkcs8 -topk8 -nocrypt -in server.key -out server-key.pem
openssl genpkey -algorithm EC -pkeyopt ec_paramgen_curve:P-256 -out other.key
openssl req -x509 -new -key other.key -sha256 -days 36500 -config ca.cnf -subj "/CN=unrelated CA" -out other-ca.pem
openssl x509 -in server.pem -noout -text | grep -E "Not After|DNS|IP Address|CA:"
cp ca.pem server.pem server-key.pem other-ca.pem "$out/"
