#!/bin/sh
# Rebuilds the test databases from their SQL sources.
set -eu
cd "$(dirname "$0")"
for name in keys content; do
    rm -f "$name.db"
    sqlite3 -bail "$name.db" < "$name.sql"
    echo "$name.db"
done
