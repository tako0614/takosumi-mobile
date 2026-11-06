#!/bin/sh
# Install the per-profile browser's AppArmor profile, so its Chromium sandbox
# can start on hosts that restrict unprivileged user namespaces through
# AppArmor (Ubuntu 24.04 and later).
#
# The profile needs AppArmor 4 policy (abi/4.0). It is installed whenever the
# host's AppArmor userspace provides that ABI and apparmor_parser accepts the
# profile, whatever the running kernel: an install in a chroot without
# securityfs, or before a reboot into a newer kernel, still finds it at boot.
# AppArmor 3 (Debian 12, Ubuntu 22.04) cannot parse it, and an unparsable file
# in /etc/apparmor.d can make apparmor.service fail at boot, so it is skipped
# there. After an upgrade from such a release, reinstall the package (for
# example `apt install --reinstall takosumi`) to install it. It is loaded now
# only when the running kernel mediates user namespaces. The shipped copy
# stays in /usr/share/takosumi/apparmor; postrm removes the installed one.
set -e
PROFILE=/usr/share/takosumi/apparmor/takosumi-cef-browser
TARGET=/etc/apparmor.d/takosumi-cef-browser
PARSER=/usr/sbin/apparmor_parser
if [ "$1" = "configure" ] && [ -f "$PROFILE" ] && [ -x "$PARSER" ] \
  && [ -e /etc/apparmor.d/abi/4.0 ] \
  && "$PARSER" -Q -T -K "$PROFILE" >/dev/null 2>&1; then
  install -m 0644 "$PROFILE" "$TARGET.dpkg-new"
  mv -f "$TARGET.dpkg-new" "$TARGET"
  if [ -e /sys/kernel/security/apparmor/features/namespaces/userns_create ]; then
    "$PARSER" -r -T -W "$TARGET" || true
  fi
fi
exit 0
