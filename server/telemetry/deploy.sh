#!/usr/bin/env bash
# Initial isolated deployment only. Does not edit WordPress or its database.
set -euo pipefail
test "$(id -u)" = 0
source_dir=$(cd -- "$(dirname -- "$0")" && pwd)
binary=${1:-$source_dir/target/release/giveaway-scout-telemetry}
test -x "$binary"
config=/etc/nginx/sites-available/historyalive.ru
test "$(readlink -f /etc/nginx/sites-enabled/historyalive.ru)" = "$config"
for target in /opt/giveaway-scout-telemetry /etc/giveaway-scout-telemetry /etc/systemd/system/giveaway-scout-telemetry.service /etc/nginx/conf.d/giveaway-scout-telemetry.conf /etc/nginx/snippets/giveaway-scout-telemetry.conf; do
    if test -e "$target"; then
        printf 'Refusing to overwrite an existing deployment: %s\n' "$target" >&2
        exit 1
    fi
done
test -d /root/backups || install -d -m 0700 /root/backups
backup_dir=$(mktemp -d /root/backups/giveaway-scout-telemetry-20261025.XXXXXX)
cp -p "$config" "$backup_dir/nginx-original.conf"
curl -fsS -o /dev/null https://historyalive.ru/
mutated=0
rollback() {
    result=$?
    if test "$result" -ne 0 && test "$mutated" = 1; then
        systemctl disable --now giveaway-scout-telemetry.service || true
        install -m 0644 "$backup_dir/nginx-original.conf" "$config"
        mkdir -m 0700 "$backup_dir/rollback"
        for file in /etc/nginx/conf.d/giveaway-scout-telemetry.conf /etc/nginx/snippets/giveaway-scout-telemetry.conf /etc/systemd/system/giveaway-scout-telemetry.service; do
            if test -f "$file"; then
                target_dir="$backup_dir/rollback$(dirname -- "$file")"
                mkdir -p "$target_dir"
                mv -- "$file" "$target_dir/"
            fi
        done
        systemctl daemon-reload
        nginx -t && systemctl reload nginx
        printf 'Deployment rolled back. Backup: %s\n' "$backup_dir" >&2
    fi
    exit "$result"
}
trap rollback EXIT
mutated=1
install -d -m 0755 /opt/giveaway-scout-telemetry
install -m 0755 "$binary" /opt/giveaway-scout-telemetry/giveaway-scout-telemetry
install -d -o root -g www-data -m 0750 /etc/giveaway-scout-telemetry
openssl rand -out /etc/giveaway-scout-telemetry/salt 32
chmod 0600 /etc/giveaway-scout-telemetry/salt
umask 077
openssl rand -base64 32 > /etc/giveaway-scout-telemetry/dashboard-password
printf 'giveaway-scout:' > /etc/giveaway-scout-telemetry/dashboard.htpasswd
openssl passwd -6 -stdin < /etc/giveaway-scout-telemetry/dashboard-password >> /etc/giveaway-scout-telemetry/dashboard.htpasswd
chown root:www-data /etc/giveaway-scout-telemetry/dashboard.htpasswd
chmod 0640 /etc/giveaway-scout-telemetry/dashboard.htpasswd
{
    printf 'Страница: https://historyalive.ru/giveaway-scout/statistics/\nЛогин: giveaway-scout\nПароль: '
    tr -d '\n' < /etc/giveaway-scout-telemetry/dashboard-password
    printf '\n'
} > /etc/giveaway-scout-telemetry/dashboard-access.txt
install -m 0644 "$source_dir/giveaway-scout-telemetry.service" /etc/systemd/system/giveaway-scout-telemetry.service
systemctl daemon-reload
systemctl enable --now giveaway-scout-telemetry.service
for attempt in 1 2 3 4 5; do
    if curl -fsS -o /dev/null http://127.0.0.1:8793/health; then break; fi
    sleep 1
done
curl -fsS -o /dev/null http://127.0.0.1:8793/health
install -m 0644 "$source_dir/nginx-http.conf" /etc/nginx/conf.d/giveaway-scout-telemetry.conf
install -m 0644 "$source_dir/nginx-location.conf" /etc/nginx/snippets/giveaway-scout-telemetry.conf
awk '!inserted && /server_name historyalive.ru www.historyalive.ru;/ {
    print; print "    include /etc/nginx/snippets/giveaway-scout-telemetry.conf;"; inserted=1; next
} { print } END { if (!inserted) exit 1 }' "$config" > "$backup_dir/nginx-candidate.conf"
install -m 0644 "$backup_dir/nginx-candidate.conf" "$config"
nginx -t
systemctl reload nginx
curl -fsS -o /dev/null https://historyalive.ru/
systemctl is-active nginx giveaway-scout-telemetry.service
printf 'DEPLOYED; backup=%s\n' "$backup_dir"
trap - EXIT
