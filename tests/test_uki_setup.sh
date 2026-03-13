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
INSTALL_PLUGIN="$TMPDIR_WORK/usr-lib-kernel-install.d-90-uki-ukify.install"
BACKUP_ROOT="$TMPDIR_WORK/backups"
EFI_DIR="$TMPDIR_WORK/esp/EFI/Linux"
CMDLINE="root=UUID=test-uuid rw quiet"
AUTO_DETECT_CMDLINE=0
UKIFY_SB_KEY="/etc/pki/uki/test.key"
UKIFY_SB_CERT="/etc/pki/uki/test.crt"
INITRAMFS_REQUIRED_LIST="/etc/uki/initramfs-required.txt"
INITRAMFS_FORBIDDEN_LIST="/etc/uki/initramfs-forbidden.txt"
INITRAMFS_STATE_DIR="/var/lib/uki-build"
INITRAMFS_STRICT_DIFF=1

phase_write_build_script

[[ -x "$BUILD_SCRIPT" ]] || {
    echo "Expected build script to be executable at $BUILD_SCRIPT"
    exit 1
}

if grep -q '__EFI_DIR__\|__CMDLINE__\|__AUTO_DETECT_CMDLINE__\|__UKIFY_SB_KEY__\|__UKIFY_SB_CERT__\|__INITRAMFS_REQUIRED_LIST__\|__INITRAMFS_FORBIDDEN_LIST__\|__INITRAMFS_STATE_DIR__\|__INITRAMFS_STRICT_DIFF__' "$BUILD_SCRIPT"; then
    echo "Template placeholders were not fully substituted in build script"
    exit 1
fi

grep -q "EFI_DIR=\"$EFI_DIR\"" "$BUILD_SCRIPT"
grep -q "CMDLINE=\"$CMDLINE\"" "$BUILD_SCRIPT"
grep -q 'AUTO_DETECT_CMDLINE=0' "$BUILD_SCRIPT"
grep -q "UKIFY_SB_KEY=\"$UKIFY_SB_KEY\"" "$BUILD_SCRIPT"
grep -q "UKIFY_SB_CERT=\"$UKIFY_SB_CERT\"" "$BUILD_SCRIPT"
grep -q "INITRAMFS_REQUIRED_LIST=\"$INITRAMFS_REQUIRED_LIST\"" "$BUILD_SCRIPT"
grep -q "INITRAMFS_FORBIDDEN_LIST=\"$INITRAMFS_FORBIDDEN_LIST\"" "$BUILD_SCRIPT"
grep -q "INITRAMFS_STATE_DIR=\"$INITRAMFS_STATE_DIR\"" "$BUILD_SCRIPT"
grep -q 'INITRAMFS_STRICT_DIFF=1' "$BUILD_SCRIPT"
grep -q 'lsinitrd --unpack' "$BUILD_SCRIPT"
grep -q 'diff -u "$previous_manifest" "$current_manifest"' "$BUILD_SCRIPT"
grep -q 'BOOT_SUCCESS_DIR="/var/lib/uki-ukify/boot-success"' "$BUILD_SCRIPT"
grep -q 'list_installed_kernels()' "$BUILD_SCRIPT"
grep -q 'reconcile_kernel_ukis()' "$BUILD_SCRIPT"
grep -q 'if \[\[ "\${1:-}" == "--reconcile" \]\]' "$BUILD_SCRIPT"
grep -q 'require_cmd file' "$BUILD_SCRIPT"
grep -q 'require_cmd objdump' "$BUILD_SCRIPT"
grep -q 'verify_uki_post_build()' "$BUILD_SCRIPT"
grep -q 'Stage 3/3: Verifying UKI artifact integrity and metadata' "$BUILD_SCRIPT"
grep -q '/var/log/uki-setup.log' "$BUILD_SCRIPT"

phase_write_plugin

[[ -x "$INSTALL_PLUGIN" ]] || {
    echo "Expected install plugin to be executable at $INSTALL_PLUGIN"
    exit 1
}

grep -q "BUILD_SCRIPT=\"$BUILD_SCRIPT\"" "$INSTALL_PLUGIN"
grep -q '"\$BUILD_SCRIPT" --reconcile' "$INSTALL_PLUGIN"
grep -q 'Kernel add: \${KERNEL_VER} — reconciling all installed kernel UKIs' "$INSTALL_PLUGIN"

echo "All local UKI setup checks passed."
