#!/bin/sh
# Unload and remove the AppArmor profile that postinst installed into
# /etc/apparmor.d. An upgrade keeps it; postinst replaces it afterwards.
set -e
TARGET=/etc/apparmor.d/takosumi-cef-browser
PARSER=/usr/sbin/apparmor_parser
case "$1" in
  remove | purge)
    if [ -f "$TARGET" ]; then
      if [ -x "$PARSER" ] && [ -d /sys/kernel/security/apparmor ]; then
        "$PARSER" -R -T "$TARGET" >/dev/null 2>&1 || true
      fi
      rm -f "$TARGET"
    fi
    ;;
esac
exit 0
