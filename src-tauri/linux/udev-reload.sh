#!/bin/sh
# Apply 70-maxwell-peq.rules to a headset or dongle that is already plugged in.
if command -v udevadm >/dev/null 2>&1; then
    udevadm control --reload-rules || true
    udevadm trigger --subsystem-match=hidraw || true
fi
exit 0
