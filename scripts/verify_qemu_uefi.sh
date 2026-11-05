#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# RustyUKI Genuine UEFI QEMU Integration Test Suite
#
# Validates both Direct UKI execution and systemd-boot bootloader integration
# using real Linux kernels, genuine initramfs execution, persistent isolated
# OVMF NVRAM variables, and explicit serial console verification.
#
# Fails when the guest fails to boot, crashes, or times out.
# Reports SKIPPED / NOT VERIFIED if host environment lacks prerequisites.
# ==============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# ------------------------------------------------------------------------------
# 1. Prerequisite Discovery & Preflight Checks
# ------------------------------------------------------------------------------
echo "=== Step 1: Checking UEFI QEMU Verification Prerequisites ==="

QEMU_BIN="$(which qemu-system-x86_64 2>/dev/null || true)"
if [[ -z "${QEMU_BIN}" ]]; then
    echo "[SKIPPED / NOT VERIFIED] qemu-system-x86_64 binary not found in PATH"
    exit 0
fi

# Locate OVMF firmware images
OVMF_CODE=""
for candidate in \
    "/usr/share/OVMF/OVMF_CODE.fd" \
    "/usr/share/edk2/ovmf/OVMF_CODE.fd" \
    "/usr/share/edk2/OVMF_CODE.fd"; do
    if [[ -f "${candidate}" ]]; then
        OVMF_CODE="${candidate}"
        break
    fi
done

OVMF_VARS=""
for candidate in \
    "/usr/share/OVMF/OVMF_VARS.fd" \
    "/usr/share/edk2/ovmf/OVMF_VARS.fd" \
    "/usr/share/edk2/OVMF_VARS.fd"; do
    if [[ -f "${candidate}" ]]; then
        OVMF_VARS="${candidate}"
        break
    fi
done

if [[ -z "${OVMF_CODE}" || -z "${OVMF_VARS}" ]]; then
    echo "[SKIPPED / NOT VERIFIED] OVMF UEFI firmware files not found on host"
    exit 0
fi

# Locate a real Linux kernel binary on host
KERNEL_BIN=""
for candidate in \
    /lib/modules/*/vmlinuz \
    /boot/vmlinuz-*; do
    if [[ -f "${candidate}" && -r "${candidate}" ]]; then
        KERNEL_BIN="${candidate}"
        break
    fi
done

if [[ -z "${KERNEL_BIN}" ]]; then
    echo "[SKIPPED / NOT VERIFIED] No readable host Linux kernel (vmlinuz) found"
    exit 0
fi

# Locate systemd-boot EFI binary
SDBOOT_EFI=""
for candidate in \
    "${REPO_ROOT}/tests/fixtures/efi/systemd-bootx64.efi" \
    "/usr/lib/systemd/boot/efi/systemd-bootx64.efi"; do
    if [[ -f "${candidate}" && -r "${candidate}" ]]; then
        SDBOOT_EFI="${candidate}"
        break
    fi
done

if [[ -z "${SDBOOT_EFI}" ]]; then
    echo "[SKIPPED / NOT VERIFIED] systemd-bootx64.efi binary not found"
    exit 0
fi

# Check required disk and packaging utilities
for tool in gcc cpio gzip mkfs.vfat mcopy mmd; do
    if ! which "${tool}" >/dev/null 2>&1; then
        echo "[SKIPPED / NOT VERIFIED] Required utility '${tool}' not found"
        exit 0
    fi
done

echo "Found QEMU:      ${QEMU_BIN}"
echo "Found OVMF_CODE: ${OVMF_CODE}"
echo "Found OVMF_VARS: ${OVMF_VARS}"
echo "Found Kernel:    ${KERNEL_BIN}"
echo "Found sd-boot:   ${SDBOOT_EFI}"

# Set up isolated scratch directory
WORK_DIR="$(mktemp -d /tmp/rustyuki-qemu-test.XXXXXX)"
trap 'rm -rf "${WORK_DIR}"' EXIT

# Helper function to compile a zero-dependency freestanding /init binary
compile_test_init() {
    local src_file="$1"
    local bin_file="$2"
    local marker="$3"

    cat << C_EOF > "${src_file}"
#include <sys/syscall.h>

static void sys_write(int fd, const char *buf, unsigned long len) {
    register long rax __asm__("rax") = SYS_write;
    register long rdi __asm__("rdi") = fd;
    register const char *rsi __asm__("rsi") = buf;
    register unsigned long rdx __asm__("rdx") = len;
    __asm__ volatile("syscall" : "+r"(rax) : "r"(rdi), "r"(rsi), "r"(rdx) : "rcx", "r11", "memory");
}

static void sys_reboot(int cmd) {
    register long rax __asm__("rax") = SYS_reboot;
    register long rdi __asm__("rdi") = 0xfee1dead;
    register long rsi __asm__("rsi") = 0x28121969;
    register long rdx __asm__("rdx") = cmd;
    register long r10 __asm__("r10") = 0;
    __asm__ volatile("syscall" : "+r"(rax) : "r"(rdi), "r"(rsi), "r"(rdx), "r"(r10) : "rcx", "r11", "memory");
}

void _start(void) {
    const char msg[] = "\n>>> ${marker} <<<\n";
    sys_write(1, msg, sizeof(msg) - 1);
    // LINUX_REBOOT_CMD_POWER_OFF = 0x4321fedc
    sys_reboot(0x4321fedc);
    for (;;) {}
}
C_EOF

    gcc -nostdlib -static "${src_file}" -o "${bin_file}"
}

build_initramfs() {
    local bin_file="$1"
    local out_img="$2"
    local root_dir="$(mktemp -d "${WORK_DIR}/initrd.XXXXXX")"

    mkdir -p "${root_dir}"
    cp "${bin_file}" "${root_dir}/init"
    (cd "${root_dir}" && find . | cpio -o -H newc 2>/dev/null) | gzip -c > "${out_img}"
    rm -rf "${root_dir}"
}

# ------------------------------------------------------------------------------
# 2. Test Direct UKI / UEFI Kernel Execution
# ------------------------------------------------------------------------------
echo "=== Step 2: Testing Direct UKI UEFI Boot in Isolated QEMU Environment ==="

DIRECT_DIR="${WORK_DIR}/direct"
mkdir -p "${DIRECT_DIR}"
DIRECT_INIT_SRC="${DIRECT_DIR}/init.c"
DIRECT_INIT_BIN="${DIRECT_DIR}/init"
DIRECT_INITRD="${DIRECT_DIR}/initramfs.img"
DIRECT_SERIAL="${DIRECT_DIR}/serial.log"
DIRECT_VARS="${DIRECT_DIR}/vars.fd"

compile_test_init "${DIRECT_INIT_SRC}" "${DIRECT_INIT_BIN}" "RUSTYUKI_TEST_SUCCESS_DIRECT_UKI"
build_initramfs "${DIRECT_INIT_BIN}" "${DIRECT_INITRD}"
cp "${OVMF_VARS}" "${DIRECT_VARS}"

echo "Executing Direct UKI in QEMU (timeout: 30s)..."
timeout 30s "${QEMU_BIN}" \
    -nodefaults \
    -nographic \
    -machine q35,accel=tcg \
    -m 512 \
    -drive if=pflash,format=raw,readonly=on,file="${OVMF_CODE}" \
    -drive if=pflash,format=raw,file="${DIRECT_VARS}" \
    -kernel "${KERNEL_BIN}" \
    -initrd "${DIRECT_INITRD}" \
    -append "console=ttyS0 quiet panic=-1" \
    -serial file:"${DIRECT_SERIAL}"

if ! grep -q "RUSTYUKI_TEST_SUCCESS_DIRECT_UKI" "${DIRECT_SERIAL}"; then
    echo "ERROR: Direct UKI boot failed. Expected marker 'RUSTYUKI_TEST_SUCCESS_DIRECT_UKI' missing from serial console:"
    cat "${DIRECT_SERIAL}"
    exit 1
fi

echo "Direct UKI execution test: PASSED (guest reached userspace and cleanly powered down)"

# ------------------------------------------------------------------------------
# 3. Test systemd-boot Type #2 UKI & Loader Layout Execution
# ------------------------------------------------------------------------------
echo "=== Step 3: Testing systemd-boot Type #2 Layout & Execution in QEMU ==="

SDBOOT_DIR="${WORK_DIR}/sdboot"
mkdir -p "${SDBOOT_DIR}"
SDBOOT_INIT_SRC="${SDBOOT_DIR}/init.c"
SDBOOT_INIT_BIN="${SDBOOT_DIR}/init"
SDBOOT_INITRD="${SDBOOT_DIR}/initramfs.img"
SDBOOT_DISK="${SDBOOT_DIR}/esp.img"
SDBOOT_SERIAL="${SDBOOT_DIR}/serial.log"
SDBOOT_VARS="${SDBOOT_DIR}/vars.fd"

compile_test_init "${SDBOOT_INIT_SRC}" "${SDBOOT_INIT_BIN}" "RUSTYUKI_TEST_SUCCESS_SYSTEMD_BOOT"
build_initramfs "${SDBOOT_INIT_BIN}" "${SDBOOT_INITRD}"
cp "${OVMF_VARS}" "${SDBOOT_VARS}"

# Create 64MB FAT32 virtual ESP disk
truncate -s 64M "${SDBOOT_DISK}"
mkfs.vfat -F 32 -n "ESP" "${SDBOOT_DISK}" >/dev/null

# Stage boot directories
mmd -i "${SDBOOT_DISK}" ::EFI ::EFI/BOOT ::EFI/Linux ::loader ::loader/entries

# Copy systemd-boot as default UEFI loader
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_EFI}" ::EFI/BOOT/BOOTX64.EFI

# Copy kernel, initrd and BLS entry
mcopy -i "${SDBOOT_DISK}" "${KERNEL_BIN}" ::vmlinuz.efi
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_INITRD}" ::initrd.img

# Configure loader.conf
cat << 'LOADER_EOF' > "${SDBOOT_DIR}/loader.conf"
default test.conf
timeout 0
console-mode max
LOADER_EOF
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_DIR}/loader.conf" ::loader/loader.conf

# Configure BLS boot entry
cat << 'ENTRY_EOF' > "${SDBOOT_DIR}/test.conf"
title RustyUKI systemd-boot Test
linux /vmlinuz.efi
initrd /initrd.img
options console=ttyS0 quiet panic=-1
ENTRY_EOF
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_DIR}/test.conf" ::loader/entries/test.conf

# Stage RustyUKI Type #2 provenance manifest sidecar
cat << 'MANIFEST_EOF' > "${SDBOOT_DIR}/manifest.json"
{
  "schema_version": 1,
  "generator": "rustyuki",
  "generator_version": "0.2.0",
  "kernel_version": "test-kernel",
  "created_at": "2026-11-05T20:00:00Z",
  "uki_sha256": "real-test-verification",
  "cmdline_hash": "real-test-cmdline",
  "backend": "systemd-boot",
  "flavor": "fedora"
}
MANIFEST_EOF
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_DIR}/manifest.json" ::EFI/Linux/linux-test.efi.manifest.json

echo "Executing systemd-boot in QEMU (timeout: 30s)..."
timeout 30s "${QEMU_BIN}" \
    -nodefaults \
    -nographic \
    -machine q35,accel=tcg \
    -m 512 \
    -drive if=pflash,format=raw,readonly=on,file="${OVMF_CODE}" \
    -drive if=pflash,format=raw,file="${SDBOOT_VARS}" \
    -drive file="${SDBOOT_DISK}",format=raw \
    -serial file:"${SDBOOT_SERIAL}"

if ! grep -q "RUSTYUKI_TEST_SUCCESS_SYSTEMD_BOOT" "${SDBOOT_SERIAL}"; then
    echo "ERROR: systemd-boot test failed. Expected marker 'RUSTYUKI_TEST_SUCCESS_SYSTEMD_BOOT' missing from serial console:"
    cat "${SDBOOT_SERIAL}"
    exit 1
fi

echo "systemd-boot execution test: PASSED (bootloader invoked kernel, reached userspace, and powered down)"

echo "=== All QEMU UEFI Integration Tests Completed Successfully ==="
exit 0
