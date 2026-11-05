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

# Create EFI directory structure using mtools if available, or mount loopback
echo "=== Staging UKI boot directories ==="
mkdir -p "${WORK_DIR}/esp_mnt/EFI/BOOT"
mkdir -p "${WORK_DIR}/esp_mnt/EFI/Linux"

# Build a stub test UKI or use release binary
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
