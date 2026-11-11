#!/bin/sh
# Workspace layer check: no crate below music video-app may depend on the interface
# toolkit (egui, eframe, winit). Only the app front end may ever name them,
# so the front end stays replaceable. Fails with a nonzero exit when a
# forbidden dependency appears in a Cargo.toml below crates/music video-app.
set -u

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
FORBIDDEN='egui|eframe|winit'
STATUS=0

for manifest in "$ROOT"/crates/*/Cargo.toml; do
    case "$manifest" in
        */music video-app/Cargo.toml) continue ;;
    esac
    if grep -Eq "^[[:space:]]*($FORBIDDEN)[[:space:]]*=" "$manifest"; then
        echo "layer violation: $manifest names a forbidden interface dependency" >&2
        STATUS=1
    fi
done

if [ "$STATUS" -eq 0 ]; then
    echo "layers ok: no interface toolkit dependencies below music video-app"
fi
exit "$STATUS"
