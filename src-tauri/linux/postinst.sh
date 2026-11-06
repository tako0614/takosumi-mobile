#!/bin/sh
# Load the browser's AppArmor profile now, so the sandboxed browser can start
# without waiting for a reboot. Hosts without AppArmor skip this.
set -e
if [ "$1" = "configure" ] && [ -x /usr/sbin/apparmor_parser ] && [ -d /sys/kernel/security/apparmor ]; then
  /usr/sbin/apparmor_parser -r -T -W /etc/apparmor.d/takosumi-cef-browser || true
fi
exit 0
