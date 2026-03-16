#!/bin/bash

SSL_DIR="/etc/squid/ssl"
CERT_DB="/var/lib/squid/ssl_db"

# Generate CA cert if not present (persisted via volume)
if [ ! -f "$SSL_DIR/ca-cert.pem" ]; then
    echo "Generating CA certificate..."
    openssl req -new -newkey rsa:2048 -sha256 -days 3650 -nodes -x509 \
        -subj "/CN=whoah-cache-ca/O=whoah/OU=build-cache" \
        -keyout "$SSL_DIR/ca-key.pem" \
        -out "$SSL_DIR/ca-cert.pem" 2>/dev/null
    echo "CA certificate generated."
fi

# Initialize the SSL certificate database (recreate each start)
rm -rf "$CERT_DB"
/usr/lib/squid/security_file_certgen -c -s "$CERT_DB" -M 4MB

# Fix all permissions
chown -R proxy:proxy /var/spool/squid /var/log/squid /var/lib/squid "$SSL_DIR"

# Initialize cache directory
squid -z -N 2>/dev/null || true

echo "Starting Squid with SSL-bump..."
exec squid -N -f /etc/squid/squid.conf
