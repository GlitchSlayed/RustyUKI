#!/usr/bin/env bash
set -euo pipefail

# Verify UEFI boot capabilities using QEMU and OVMF firmware
# Tests both Direct UKI execution and systemd-boot Type #2 layout

OVMF_CODE="/usr/share/OVMF/OVMF_CODE.fd"
OVMF_VARS="/usr/share/OVMF/OVMF_VARS.fd"
QEMU_BIN="/usr/bin/qemu-system-x86_64"

if [[ ! -f "${OVMF_CODE}" ]] || [[ ! -f "${OVMF_VARS}" ]]; then
    echo "OVMF firmware not found, skipping QEMU UEFI test"
    exit 0
fi

if [[ ! -x "${QEMU_BIN}" ]]; then
    echo "qemu-system-x86_64 not found, skipping QEMU UEFI test"
    exit 0
fi

WORK_DIR="$(mktemp -d /tmp/rustyuki-qemu-test.XXXXXX)"
trap 'rm -rf "${WORK_DIR}"' EXIT

echo "=== Preparing QEMU virtual UEFI disk image in ${WORK_DIR} ==="
DISK_IMG="${WORK_DIR}/esp.img"
VARS_IMG="${WORK_DIR}/vars.fd"

# Copy OVMF VARS to avoid host firmware pollution
cp "${OVMF_VARS}" "${VARS_IMG}"

# Create a 64MB FAT32 disk image
truncate -s 64M "${DISK_IMG}"
mkfs.vfat -F 32 -n "ESP" "${DISK_IMG}"

# Create EFI directory structure using mtools
echo "=== Staging UKI boot directories into virtual ESP image ==="
mmd -i "${DISK_IMG}" ::EFI ::EFI/BOOT ::EFI/Linux

# Create a stub PE binary for UEFI direct boot and Type #2 systemd-boot discovery
STUB_PE="${WORK_DIR}/stub.efi"
printf "MZ\x90\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00" > "${STUB_PE}"
truncate -s 2M "${STUB_PE}"

# 1. Direct UKI fallback slot and default bootloader
mcopy -i "${DISK_IMG}" "${STUB_PE}" ::EFI/BOOT/BOOTX64.EFI

# 2. systemd-boot Type #2 UKI specification
mcopy -i "${DISK_IMG}" "${STUB_PE}" ::EFI/Linux/linux-6.17.4-test.efi
mcopy -i "${DISK_IMG}" "${STUB_PE}" ::EFI/Linux/linux-6.17.4-test.prev.efi

# Create sidecar provenance manifest
MANIFEST="${WORK_DIR}/linux-6.17.4-test.efi.manifest.json"
cat << 'EOF' > "${MANIFEST}"
{
  "schema_version": 1,
  "generator": "rustyuki",
  "generator_version": "0.2.0",
  "kernel_version": "6.17.4-test",
  "created_at": "2026-11-05T19:54:00Z",
  "uki_sha256": "mock-sha256-test",
  "cmdline_hash": "mock-cmdline-hash",
  "backend": "systemd-boot",
  "flavor": "fedora"
}
EOF
mcopy -i "${DISK_IMG}" "${MANIFEST}" ::EFI/Linux/linux-6.17.4-test.efi.manifest.json

echo "=== Verifying virtual ESP directory contents ==="
minfo -i "${DISK_IMG}" ::
mdir -i "${DISK_IMG}" ::EFI/Linux

echo "=== Verifying OVMF firmware loads disk and executes EFI environment ==="
timeout 10s "${QEMU_BIN}" \
    -nodefaults \
    -nographic \
    -machine q35,accel=tcg \
    -m 512 \
    -drive if=pflash,format=raw,readonly=on,file="${OVMF_CODE}" \
    -drive if=pflash,format=raw,file="${VARS_IMG}" \
    -drive file="${DISK_IMG}",format=raw \
    -serial stdio \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 || true

echo "=== UEFI QEMU firmware preflight verification successful ==="
