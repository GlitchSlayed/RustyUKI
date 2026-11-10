#!/usr/bin/env bash
set -euo pipefail

# ==============================================================================
# RustyUKI Genuine UEFI QEMU Integration Test Suite
#
# Validates both Direct UKI execution and systemd-boot Type #2 UKI discovery
# using real Linux kernels, genuine UKIs assembled with the production pipeline,
# persistent isolated OVMF NVRAM variables, and guest serial console verification.
#
# Exit status semantics:
#   PASS: 0   (All UEFI UKI boot tests passed and verified running kernel)
#   FAIL: 1   (Guest failed to boot, kernel panic, or verification failed)
#   SKIP: 77  (Required test infrastructure unavailable; standard CI skip code)
# ==============================================================================

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

# If running on host without ukify installed, but podman with rustyuki-dev is available,
# seamlessly forward to the container environment.
if ! which ukify >/dev/null 2>&1 && which podman >/dev/null 2>&1; then
    if podman image exists localhost/rustyuki-dev 2>/dev/null; then
        echo "ukify not found in host PATH; delegating UEFI boot verification to rustyuki-dev container..."
        exec podman run --rm \
            -v "${REPO_ROOT}":/workspace:z \
            -w /workspace \
            localhost/rustyuki-dev \
            ./scripts/verify_qemu_uefi.sh "$@"
    fi
fi

# ------------------------------------------------------------------------------
# 1. Prerequisite Preflight Checks (Exit 77 if missing)
# ------------------------------------------------------------------------------
echo "=== Step 1: Checking UEFI QEMU Verification Prerequisites ==="

QEMU_BIN="$(which qemu-system-x86_64 2>/dev/null || true)"
if [[ -z "${QEMU_BIN}" ]]; then
    echo "[SKIP] qemu-system-x86_64 binary not found in PATH"
    exit 77
fi

# Locate OVMF firmware images
OVMF_CODE=""
for candidate in \
    "/usr/share/OVMF/OVMF_CODE_4M.fd" \
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
    "/usr/share/OVMF/OVMF_VARS_4M.fd" \
    "/usr/share/OVMF/OVMF_VARS.fd" \
    "/usr/share/edk2/ovmf/OVMF_VARS.fd" \
    "/usr/share/edk2/OVMF_VARS.fd"; do
    if [[ -f "${candidate}" ]]; then
        OVMF_VARS="${candidate}"
        break
    fi
done

if [[ -z "${OVMF_CODE}" || -z "${OVMF_VARS}" ]]; then
    echo "[SKIP] OVMF UEFI firmware files not found on system"
    exit 77
fi

# Locate a real Linux kernel binary
KERNEL_BIN=""
KVER=""
for candidate in \
    /lib/modules/*/vmlinuz \
    /usr/lib/modules/*/vmlinuz \
    /boot/vmlinuz-*; do
    if [[ -f "${candidate}" && -r "${candidate}" ]]; then
        KERNEL_BIN="${candidate}"
        # Extract kernel version from directory name if under /lib/modules/
        if [[ "${candidate}" =~ /modules/([^/]+)/vmlinuz ]]; then
            KVER="${BASH_REMATCH[1]}"
        elif [[ "${candidate}" =~ /boot/vmlinuz-(.+) ]]; then
            KVER="${BASH_REMATCH[1]}"
        fi
        break
    fi
done

if [[ -z "${KERNEL_BIN}" || -z "${KVER}" ]]; then
    echo "[SKIP] No readable host Linux kernel (vmlinuz) found"
    exit 77
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
    echo "[SKIP] systemd-bootx64.efi binary not found"
    exit 77
fi

# Check required disk and packaging utilities
for tool in gcc cpio gzip mkfs.vfat mcopy mmd ukify; do
    if ! which "${tool}" >/dev/null 2>&1; then
        echo "[SKIP] Required utility '${tool}' not found"
        exit 77
    fi
done

echo "Found QEMU:      ${QEMU_BIN}"
echo "Found OVMF_CODE: ${OVMF_CODE}"
echo "Found OVMF_VARS: ${OVMF_VARS}"
echo "Found Kernel:    ${KERNEL_BIN} (${KVER})"
echo "Found sd-boot:   ${SDBOOT_EFI}"

# Set up isolated scratch directory
WORK_DIR="$(mktemp -d /tmp/rustyuki-qemu-test.XXXXXX)"
trap 'rm -rf "${WORK_DIR}"' EXIT

# Helper function to compile a zero-dependency freestanding /init binary
# that calls sys_uname to verify running kernel release and powers down cleanly.
compile_test_init() {
    local src_file="$1"
    local bin_file="$2"
    local marker="$3"

    cat << C_EOF > "${src_file}"
#include <sys/syscall.h>

struct new_utsname {
    char sysname[65];
    char nodename[65];
    char release[65];
    char version[65];
    char machine[65];
    char domainname[65];
};

static void sys_write(int fd, const char *buf, unsigned long len) {
    register long rax __asm__("rax") = SYS_write;
    register long rdi __asm__("rdi") = fd;
    register const char *rsi __asm__("rsi") = buf;
    register unsigned long rdx __asm__("rdx") = len;
    __asm__ volatile("syscall" : "+r"(rax) : "r"(rdi), "r"(rsi), "r"(rdx) : "rcx", "r11", "memory");
}

static int sys_uname(struct new_utsname *buf) {
    register long rax __asm__("rax") = SYS_uname;
    register long rdi __asm__("rdi") = (long)buf;
    __asm__ volatile("syscall" : "+r"(rax) : "r"(rdi) : "rcx", "r11", "memory");
    return (int)rax;
}

static void sys_reboot(int cmd) {
    register long rax __asm__("rax") = SYS_reboot;
    register long rdi __asm__("rdi") = 0xfee1dead;
    register long rsi __asm__("rsi") = 0x28121969;
    register long rdx __asm__("rdx") = cmd;
    register long r10 __asm__("r10") = 0;
    __asm__ volatile("syscall" : "+r"(rax) : "r"(rdi), "r"(rsi), "r"(rdx), "r"(r10) : "rcx", "r11", "memory");
}

static unsigned long str_len(const char *s) {
    unsigned long n = 0;
    while (s[n]) n++;
    return n;
}

void _start(void) {
    struct new_utsname u;
    const char banner[] = "\n=== RUSTYUKI GUEST USERSPACE ACTIVE ===\n";
    sys_write(1, banner, sizeof(banner) - 1);

    if (sys_uname(&u) == 0) {
        const char kprefix[] = "RUNNING_KERNEL_RELEASE: ";
        sys_write(1, kprefix, sizeof(kprefix) - 1);
        sys_write(1, u.release, str_len(u.release));
        sys_write(1, "\n", 1);
    }

    const char marker[] = ">>> ${marker} <<<\n\n";
    sys_write(1, marker, sizeof(marker) - 1);

    // Clean ACPI power down (LINUX_REBOOT_CMD_POWER_OFF)
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

# Generate valid os-release for embedded UKI .osrel section
OS_RELEASE_FILE="${WORK_DIR}/os-release"
cat << 'OSREL_EOF' > "${OS_RELEASE_FILE}"
NAME="Fedora Linux"
VERSION="44 (RustyUKI Verified)"
ID=fedora
VERSION_ID=44
PRETTY_NAME="Fedora Linux 44 (RustyUKI Verified)"
OSREL_EOF

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
DIRECT_DISK="${DIRECT_DIR}/esp.img"
DIRECT_UKI="${DIRECT_DIR}/linux-${KVER}.efi"

compile_test_init "${DIRECT_INIT_SRC}" "${DIRECT_INIT_BIN}" "RUSTYUKI_BOOT_SUCCESS_DIRECT_UKI"
build_initramfs "${DIRECT_INIT_BIN}" "${DIRECT_INITRD}"

# Build real UKI using ukify
ukify build \
    --linux "${KERNEL_BIN}" \
    --initrd "${DIRECT_INITRD}" \
    --cmdline "root=/dev/ram0 console=ttyS0 quiet panic=-1 init=/init" \
    --os-release "@${OS_RELEASE_FILE}" \
    --output "${DIRECT_UKI}" >/dev/null

echo "Assembled Direct UKI ($(ls -lh "${DIRECT_UKI}" | awk '{print $5}'))"

# Verify UKI is a valid PE32+ executable
if ! file "${DIRECT_UKI}" | grep -q "PE32+"; then
    echo "ERROR: Generated Direct UKI is not a valid PE32+ executable!"
    exit 1
fi

# Create 64MB FAT32 virtual ESP disk
truncate -s 64M "${DIRECT_DISK}"
mkfs.vfat -F 32 -n "ESP" "${DIRECT_DISK}" >/dev/null
mmd -i "${DIRECT_DISK}" ::EFI ::EFI/BOOT ::EFI/Linux

# Direct UKI: deploy UKI to both standard location and UEFI bootloader entry
mcopy -i "${DIRECT_DISK}" "${DIRECT_UKI}" ::EFI/BOOT/BOOTX64.EFI
mcopy -i "${DIRECT_DISK}" "${DIRECT_UKI}" "::EFI/Linux/linux-${KVER}.efi"

cp "${OVMF_VARS}" "${DIRECT_VARS}"

echo "Executing Direct UKI in QEMU via UEFI firmware boot (timeout: 35s)..."
if ! timeout 35s "${QEMU_BIN}" \
    -nodefaults \
    -nographic \
    -machine q35,accel=tcg \
    -m 512 \
    -drive if=pflash,format=raw,readonly=on,file="${OVMF_CODE}" \
    -drive if=pflash,format=raw,file="${DIRECT_VARS}" \
    -drive file="${DIRECT_DISK}",format=raw \
    -serial file:"${DIRECT_SERIAL}"; then
    echo "ERROR: QEMU execution timed out or crashed during Direct UKI boot."
    cat "${DIRECT_SERIAL}"
    exit 1
fi

cat "${DIRECT_SERIAL}"

if ! grep -q "RUSTYUKI_BOOT_SUCCESS_DIRECT_UKI" "${DIRECT_SERIAL}"; then
    echo "ERROR: Direct UKI boot failed. Expected marker 'RUSTYUKI_BOOT_SUCCESS_DIRECT_UKI' missing from serial console."
    exit 1
fi

if ! grep -q "RUNNING_KERNEL_RELEASE: ${KVER}" "${DIRECT_SERIAL}"; then
    echo "ERROR: Guest booted unexpected kernel release! Expected ${KVER}."
    exit 1
fi

echo "Direct UKI execution test: PASSED (guest reached userspace and verified kernel ${KVER})"

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
SDBOOT_UKI="${SDBOOT_DIR}/linux-${KVER}.efi"

compile_test_init "${SDBOOT_INIT_SRC}" "${SDBOOT_INIT_BIN}" "RUSTYUKI_BOOT_SUCCESS_SYSTEMD_BOOT_TYPE2"
build_initramfs "${SDBOOT_INIT_BIN}" "${SDBOOT_INITRD}"

# Build real UKI using ukify
ukify build \
    --linux "${KERNEL_BIN}" \
    --initrd "${SDBOOT_INITRD}" \
    --cmdline "root=/dev/ram0 console=ttyS0 quiet panic=-1 init=/init" \
    --os-release "@${OS_RELEASE_FILE}" \
    --output "${SDBOOT_UKI}" >/dev/null

echo "Assembled Type #2 UKI ($(ls -lh "${SDBOOT_UKI}" | awk '{print $5}'))"

# Verify UKI is a valid PE32+ executable
if ! file "${SDBOOT_UKI}" | grep -q "PE32+"; then
    echo "ERROR: Generated Type #2 UKI is not a valid PE32+ executable!"
    exit 1
fi

# Create 64MB FAT32 virtual ESP disk
truncate -s 64M "${SDBOOT_DISK}"
mkfs.vfat -F 32 -n "ESP" "${SDBOOT_DISK}" >/dev/null

# Stage boot directories (NO Type #1 loader/entries directory!)
mmd -i "${SDBOOT_DISK}" ::EFI ::EFI/BOOT ::EFI/Linux ::loader

# Copy systemd-boot as default UEFI loader
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_EFI}" ::EFI/BOOT/BOOTX64.EFI

# Copy Type #2 UKI under /EFI/Linux/ (systemd-boot native automatic discovery)
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_UKI}" "::EFI/Linux/linux-${KVER}.efi"

# Configure loader.conf
cat << 'LOADER_EOF' > "${SDBOOT_DIR}/loader.conf"
default linux-*
timeout 1
console-mode max
LOADER_EOF
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_DIR}/loader.conf" ::loader/loader.conf

# Stage RustyUKI Type #2 provenance manifest sidecar
cat << MANIFEST_EOF > "${SDBOOT_DIR}/manifest.json"
{
  "schema_version": 1,
  "generator": "rustyuki",
  "generator_version": "0.2.0",
  "kernel_version": "${KVER}",
  "created_at": "2026-11-05T20:00:00Z",
  "uki_sha256": "real-test-verification",
  "cmdline_hash": "real-test-cmdline",
  "backend": "systemd-boot",
  "flavor": "fedora"
}
MANIFEST_EOF
mcopy -i "${SDBOOT_DISK}" "${SDBOOT_DIR}/manifest.json" "::EFI/Linux/linux-${KVER}.efi.manifest.json"

cp "${OVMF_VARS}" "${SDBOOT_VARS}"

echo "Executing systemd-boot in QEMU via Type #2 UKI discovery (timeout: 35s)..."
if ! timeout 35s "${QEMU_BIN}" \
    -nodefaults \
    -nographic \
    -machine q35,accel=tcg \
    -m 512 \
    -drive if=pflash,format=raw,readonly=on,file="${OVMF_CODE}" \
    -drive if=pflash,format=raw,file="${SDBOOT_VARS}" \
    -drive file="${SDBOOT_DISK}",format=raw \
    -serial file:"${SDBOOT_SERIAL}"; then
    echo "ERROR: QEMU execution timed out or crashed during systemd-boot."
    cat "${SDBOOT_SERIAL}"
    exit 1
fi

cat "${SDBOOT_SERIAL}"

if ! grep -q "RUSTYUKI_BOOT_SUCCESS_SYSTEMD_BOOT_TYPE2" "${SDBOOT_SERIAL}"; then
    echo "ERROR: systemd-boot test failed. Expected marker 'RUSTYUKI_BOOT_SUCCESS_SYSTEMD_BOOT_TYPE2' missing from serial console."
    exit 1
fi

if ! grep -q "RUNNING_KERNEL_RELEASE: ${KVER}" "${SDBOOT_SERIAL}"; then
    echo "ERROR: Guest booted unexpected kernel release! Expected ${KVER}."
    exit 1
fi

echo "systemd-boot execution test: PASSED (Type #2 UKI discovered and booted kernel ${KVER})"

echo "=== All QEMU UEFI Integration Tests Completed Successfully (PASS) ==="
exit 0
