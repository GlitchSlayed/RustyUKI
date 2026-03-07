#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

TMPDIR_WORK="$(mktemp -d)"
cleanup() {
    rm -rf "$TMPDIR_WORK"
}
trap cleanup EXIT

export UKI_SETUP_SKIP_MAIN=1
# shellcheck source=uki-setup.sh
source "$REPO_ROOT/uki-setup.sh"

BUILD_SCRIPT="$TMPDIR_WORK/usr-local-sbin-uki-build.sh"
INSTALL_PLUGIN="$TMPDIR_WORK/usr-lib-kernel-install.d-90-uki-dracut.install"
BACKUP_ROOT="$TMPDIR_WORK/backups"
EFI_DIR="$TMPDIR_WORK/esp/EFI/Linux"
CMDLINE="root=UUID=test-uuid rw quiet"
AUTO_DETECT_CMDLINE=0
EFI_STUB="/usr/lib/systemd/boot/efi/linuxx64.efi.stub"

phase_write_build_script

[[ -x "$BUILD_SCRIPT" ]] || {
    echo "Expected build script to be executable at $BUILD_SCRIPT"
    exit 1
}

if grep -q '__EFI_DIR__\|__CMDLINE__\|__AUTO_DETECT_CMDLINE__\|__EFI_STUB__' "$BUILD_SCRIPT"; then
    echo "Template placeholders were not fully substituted in build script"
    exit 1
fi

grep -q "EFI_DIR=\"$EFI_DIR\"" "$BUILD_SCRIPT"
grep -q "CMDLINE=\"$CMDLINE\"" "$BUILD_SCRIPT"
grep -q 'AUTO_DETECT_CMDLINE=0' "$BUILD_SCRIPT"
grep -q "EFI_STUB=\"$EFI_STUB\"" "$BUILD_SCRIPT"

phase_write_plugin

[[ -x "$INSTALL_PLUGIN" ]] || {
    echo "Expected install plugin to be executable at $INSTALL_PLUGIN"
    exit 1
}

grep -q "BUILD_SCRIPT=\"$BUILD_SCRIPT\"" "$INSTALL_PLUGIN"
grep -Fq "UKI=\"${EFI_DIR}/linux-\${KERNEL_VER}.efi\"" "$INSTALL_PLUGIN"

echo "All local UKI setup checks passed."
