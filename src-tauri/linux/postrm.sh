#!/bin/sh
# Unload and remove the AppArmor profile that postinst installed into
# /etc/apparmor.d; on purge, also the compiled cache entry that
# `apparmor_parser -W` wrote for it. Only files named takosumi-cef-browser
# are removed. An upgrade keeps them; postinst replaces them afterwards.
set -e
NAME=takosumi-cef-browser
TARGET=/etc/apparmor.d/$NAME
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
if [ "$1" = "purge" ]; then
  # The current cache directory, then every feature set's under the default
  # location (a kernel change starts a new one) and the pre-3.0 location.
  current=""
  if [ -x "$PARSER" ]; then
    current=$("$PARSER" --print-cache-dir 2>/dev/null || true)
  fi
  for entry in "${current:+$current/$NAME}" /var/cache/apparmor/*/"$NAME" /etc/apparmor.d/cache/"$NAME"; do
    if [ -n "$entry" ] && [ -f "$entry" ] && [ ! -L "$entry" ]; then
      rm -f "$entry"
    fi
  done
fi
exit 0
