#!/usr/bin/env bash
# Replace only this collector. Keep the existing database, salt, dashboard login and Nginx.
set -euo pipefail
test "$(id -u)" = 0
source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
binary=${1:-$source_dir/target/release/giveaway-scout-telemetry}
test -x "$binary"
service=giveaway-scout-telemetry.service
unit=/etc/systemd/system/$service
destination=/opt/giveaway-scout-telemetry/giveaway-scout-telemetry
database=/var/lib/giveaway-scout-telemetry/usage.sqlite3
test -f "$unit"
test -f "$database"
test -f /etc/giveaway-scout-telemetry/salt
systemctl is-active --quiet "$service"
grep -Eq '^ExecStart=(/usr/bin/python3 -B /opt/giveaway-scout-telemetry/service.py|/opt/giveaway-scout-telemetry/giveaway-scout-telemetry)$' "$unit"
curl -fsS -o /dev/null http://127.0.0.1:8793/health
curl -fsS -o /dev/null https://historyalive.ru/

install -d -m 0700 /root/backups
backup_dir=$(mktemp -d /root/backups/giveaway-scout-telemetry-rust.XXXXXX)
sed 's|^ExecStart=.*|ExecStart=/opt/giveaway-scout-telemetry/giveaway-scout-telemetry|' "$unit" > "$backup_dir/service.rust"
curl -fsS http://127.0.0.1:8793/metrics > "$backup_dir/metrics-before.json"
report_failure() {
    result=$?
    if test "$result" -ne 0; then
        systemctl stop "$service" || true
        printf 'Rust update failed; collector stopped. Data backup: %s\n' "$backup_dir" >&2
    fi
    exit "$result"
}
trap report_failure EXIT
systemctl stop "$service"
# Copy SQLite plus any WAL only while neither implementation is running.
for suffix in '' -wal -shm; do
    if test -f "$database$suffix"; then cp -p "$database$suffix" "$backup_dir/usage.sqlite3$suffix"; fi
done
install -m 0755 "$binary" "$destination.new"
mv -f "$destination.new" "$destination"
# Preserve every existing hardening/resource setting; change only the executable.
install -m 0644 "$backup_dir/service.rust" "$unit"
find /opt/giveaway-scout-telemetry -type f \( -name '*.py' -o -name '*.pyc' \) -delete
if test -d /opt/giveaway-scout-telemetry/__pycache__; then
    rmdir /opt/giveaway-scout-telemetry/__pycache__
fi
systemctl daemon-reload
systemctl start "$service"
healthy=0
for attempt in 1 2 3 4 5; do
    if curl -fsS -o /dev/null http://127.0.0.1:8793/health; then healthy=1; break; fi
    sleep 1
done
test "$healthy" = 1
"$destination" verify-live
systemctl is-active --quiet "$service"
nginx -t
trap - EXIT
printf 'UPDATED_TO_RUST; backup=%s\n' "$backup_dir"
