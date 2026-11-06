#!/bin/sh
# Install and load the per-profile browser's AppArmor profile, so its Chromium
# sandbox can start on hosts that restrict unprivileged user namespaces through
# AppArmor (Ubuntu 24.04 and later). The profile needs AppArmor 4 policy
# (abi/4.0) and the kernel's user-namespace mediation. Elsewhere it is not
# installed: AppArmor 3 cannot parse it, and an unparsable profile in
# /etc/apparmor.d can make apparmor.service fail at boot. The shipped copy
# stays in /usr/share/takosumi/apparmor; postrm removes the installed one.
set -e
PROFILE=/usr/share/takosumi/apparmor/takosumi-cef-browser
TARGET=/etc/apparmor.d/takosumi-cef-browser
PARSER=/usr/sbin/apparmor_parser
if [ "$1" = "configure" ] && [ -f "$PROFILE" ] && [ -x "$PARSER" ] \
  && [ -e /etc/apparmor.d/abi/4.0 ] \
  && [ -e /sys/kernel/security/apparmor/features/namespaces/userns_create ] \
  && "$PARSER" -Q -T -K "$PROFILE" >/dev/null 2>&1; then
  install -m 0644 "$PROFILE" "$TARGET.dpkg-new"
  mv -f "$TARGET.dpkg-new" "$TARGET"
  "$PARSER" -r -T -W "$TARGET" || true
fi
exit 0
