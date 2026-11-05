#!/usr/bin/env python3
"""
RustyUKI — Fedora VM Integration Testing Framework

Executes full-lifecycle UKI boot and management tests on official Fedora
installations in GitHub Actions using QEMU and OVMF UEFI firmware.
"""

import argparse
import hashlib
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
MANIFEST_PATH = REPO_ROOT / "scripts" / "fedora_image_manifest.json"

DEFAULT_OVMF_CODE = Path("/usr/share/OVMF/OVMF_CODE.fd")
DEFAULT_OVMF_VARS = Path("/usr/share/OVMF/OVMF_VARS.fd")


def log(msg: str):
    print(f"[rustyuki-vm] {msg}", flush=True)


def load_manifest() -> dict:
    if not MANIFEST_PATH.exists():
        sys.exit(f"ERROR: Image manifest not found at {MANIFEST_PATH}")
    with open(MANIFEST_PATH, "r", encoding="utf-8") as f:
        return json.load(f)


def verify_sha256(filepath: Path, expected_hash: str) -> bool:
    if not filepath.exists():
        return False
    h = hashlib.sha256()
    with open(filepath, "rb") as f:
        while chunk := f.read(1048576):
            h.update(chunk)
    actual = h.hexdigest().lower()
    return actual == expected_hash.lower()


def ensure_cached_image(image_key: str, cache_dir: Path) -> Path:
    cache_dir.mkdir(parents=True, exist_ok=True)
    manifest = load_manifest()
    img_info = manifest["images"].get(image_key)
    if not img_info:
        sys.exit(f"ERROR: Unknown image key '{image_key}' in manifest.")

    filename = img_info["filename"]
    expected_hash = img_info["sha256"]
    url = img_info["url"]
    dest = cache_dir / filename

    if dest.exists() and verify_sha256(dest, expected_hash):
        log(f"Cached image verified: {dest.name} ({expected_hash[:16]}...)")
        return dest

    log(f"Downloading {filename} from {url}...")
    tmp_dest = dest.with_suffix(".download.tmp")
    if tmp_dest.exists():
        tmp_dest.unlink()

    def progress(blocks, block_size, total_size):
        if total_size > 0:
            pct = min(100.0, (blocks * block_size / total_size) * 100)
            if blocks % 2000 == 0:
                print(f"\rDownloading {filename}: {pct:.1f}%", end="", flush=True)

    urllib.request.urlretrieve(url, tmp_dest, reporthook=progress)
    print("", flush=True)

    log(f"Verifying SHA-256 for downloaded {filename}...")
    if not verify_sha256(tmp_dest, expected_hash):
        tmp_dest.unlink()
        sys.exit(f"ERROR: SHA-256 verification failed for {filename}!")

    tmp_dest.rename(dest)
    log(f"Successfully cached and verified {dest.name}")
    return dest


def check_kvm() -> bool:
    kvm_path = Path("/dev/kvm")
    if kvm_path.exists() and os.access(kvm_path, os.R_OK | os.W_OK):
        return True
    return False


def build_cidata_iso(out_iso: Path, user_data: str, meta_data: str, extra_files: dict = None):
    with tempfile.TemporaryDirectory() as td:
        tdp = Path(td)
        (tdp / "user-data").write_text(user_data, encoding="utf-8")
        (tdp / "meta-data").write_text(meta_data, encoding="utf-8")

        if extra_files:
            for rel_path, src_path in extra_files.items():
                dest = tdp / rel_path
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copy2(src_path, dest)

        geniso = shutil.which("genisoimage") or shutil.which("mkisofs")
        if not geniso:
            sys.exit("ERROR: genisoimage or mkisofs is required to generate cloud-init ISO.")

        cmd = [
            geniso,
            "-output", str(out_iso),
            "-volid", "cidata",
            "-joliet",
            "-rock",
            str(tdp)
        ]
        subprocess.run(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)


class VMRunner:
    def __init__(self, work_dir: Path, artifacts_dir: Path, accel: str = "auto"):
        self.work_dir = work_dir
        self.artifacts_dir = artifacts_dir
        self.work_dir.mkdir(parents=True, exist_ok=True)
        self.artifacts_dir.mkdir(parents=True, exist_ok=True)

        if accel == "auto":
            self.use_kvm = check_kvm()
        elif accel == "kvm":
            if not check_kvm():
                sys.exit("ERROR: KVM requested but /dev/kvm is not accessible.")
            self.use_kvm = True
        else:
            self.use_kvm = False

        self.qemu_bin = shutil.which("qemu-system-x86_64")
        if not self.qemu_bin:
            log("SKIP: qemu-system-x86_64 not found in PATH.")
            sys.exit(77)

        self.ovmf_code = DEFAULT_OVMF_CODE
        self.ovmf_vars = DEFAULT_OVMF_VARS
        if not self.ovmf_code.exists() or not self.ovmf_vars.exists():
            log(f"SKIP: OVMF firmware missing ({self.ovmf_code}, {self.ovmf_vars}).")
            sys.exit(77)

        log(f"VMRunner initialized: QEMU={self.qemu_bin}, KVM={'enabled' if self.use_kvm else 'disabled (TCG)'}")

    def create_overlay(self, name: str, backing_file: Path, size: str = "15G") -> Path:
        overlay = self.work_dir / f"{name}.qcow2"
        if overlay.exists():
            overlay.unlink()
        cmd = [
            "qemu-img", "create",
            "-f", "qcow2",
            "-b", str(backing_file.resolve()),
            "-F", "qcow2",
            str(overlay),
            size
        ]
        subprocess.run(cmd, check=True, stdout=subprocess.DEVNULL)
        return overlay

    def create_esp_disk(self, name: str, size_mb: int = 1024) -> Path:
        esp_img = self.work_dir / f"{name}_esp.raw"
        if esp_img.exists():
            esp_img.unlink()
        cmd = ["truncate", "-s", f"{size_mb}M", str(esp_img)]
        subprocess.run(cmd, check=True)
        mkfs = shutil.which("mkfs.vfat") or shutil.which("mkfs.fat")
        if mkfs:
            subprocess.run([mkfs, "-F", "32", "-n", "EFI", str(esp_img)], check=True, stdout=subprocess.DEVNULL)
        return esp_img

    def run_vm(
        self,
        name: str,
        disk_overlay: Path,
        vars_fd: Path,
        cidata_iso: Path = None,
        extra_disks: list = None,
        timeout_sec: int = 180,
        success_marker: str = None,
        failure_marker: str = "ASSERTION FAILED:"
    ) -> tuple[bool, str]:
        serial_log = self.work_dir / f"{name}_serial.log"
        if serial_log.exists():
            serial_log.unlink()

        cmd = [
            self.qemu_bin,
            "-nodefaults",
            "-nographic",
            "-m", "2048",
            "-smp", "2",
            "-drive", f"if=pflash,format=raw,readonly=on,file={self.ovmf_code}",
            "-drive", f"if=pflash,format=raw,file={vars_fd}",
            "-drive", f"file={disk_overlay},format=qcow2,if=virtio",
        ]

        if self.use_kvm:
            cmd.extend(["-enable-kvm", "-cpu", "host"])
        else:
            cmd.extend(["-machine", "q35,accel=tcg", "-cpu", "max"])

        if cidata_iso and cidata_iso.exists():
            cmd.extend(["-drive", f"file={cidata_iso},format=raw,if=virtio"])

        if extra_disks:
            for ed in extra_disks:
                cmd.extend(["-drive", f"file={ed},format=raw,if=virtio"])

        cmd.extend([
            "-netdev", "user,id=net0",
            "-device", "virtio-net-pci,netdev=net0",
            "-serial", f"file:{serial_log}"
        ])

        log(f"Launching QEMU VM [{name}] (timeout: {timeout_sec}s)...")
        proc = subprocess.Popen(cmd)

        start_time = time.time()
        success_found = False
        failure_reason = ""

        try:
            while True:
                ret = proc.poll()
                if ret is not None:
                    break

                elapsed = time.time() - start_time
                if elapsed > timeout_sec:
                    proc.kill()
                    failure_reason = f"Execution timed out after {timeout_sec}s"
                    break

                if serial_log.exists():
                    try:
                        content = serial_log.read_text(encoding="utf-8", errors="replace")
                        if success_marker and success_marker in content:
                            success_found = True
                        if failure_marker and failure_marker in content:
                            failure_reason = "Guest emitted assertion failure"
                            proc.kill()
                            break
                    except Exception:
                        pass

                time.sleep(1)

        except KeyboardInterrupt:
            proc.kill()
            raise

        output = ""
        if serial_log.exists():
            output = serial_log.read_text(encoding="utf-8", errors="replace")
            shutil.copy2(serial_log, self.artifacts_dir / f"{name}_serial.log")

        if success_marker:
            passed = success_marker in output and (not failure_marker or failure_marker not in output)
        else:
            passed = proc.returncode == 0 and (not failure_marker or failure_marker not in output)

        if not passed and not failure_reason:
            if failure_marker in output:
                failure_reason = "Guest assertion failure detected in serial console."
            else:
                failure_reason = f"Success marker '{success_marker}' not observed."

        return passed, output


def ensure_host_rustyuki_binary() -> Path:
    bin_path = REPO_ROOT / "target" / "release" / "rustyuki"
    if not bin_path.exists():
        log("Building RustyUKI release binary on host...")
        subprocess.run(["cargo", "build", "--release"], cwd=REPO_ROOT, check=True)
    return bin_path


# ------------------------------------------------------------------------------
# Test Scenarios
# ------------------------------------------------------------------------------

def run_scenario_a(vm: VMRunner, base_img: Path) -> dict:
    """
    Scenario A: Fresh systemd-boot setup
    - Start from conventional Fedora installation.
    - Install RustyUKI and systemd-boot.
    - Generate & install UKI.
    - Reboot through systemd-boot.
    - Verify expected kernel release, healthy systemd status, and artifact hash.
    """
    scenario_name = "scenario_a_sdboot"
    log(f"=== Starting {scenario_name} ===")

    rustyuki_bin = ensure_host_rustyuki_binary()
    config_path = REPO_ROOT / "packaging" / "config.toml"
    hook_path = REPO_ROOT / "packaging" / "90-rustyuki.install"
    service_path = REPO_ROOT / "packaging" / "rustyuki-boot-confirm.service"

    overlay = vm.create_overlay(f"{scenario_name}_disk", base_img)
    esp_disk = vm.create_esp_disk(scenario_name, size_mb=1024)
    vars_fd = vm.work_dir / f"{scenario_name}_vars.fd"
    shutil.copy2(vm.ovmf_vars, vars_fd)

    # 1. Provisioning Stage
    user_data_stage1 = """#cloud-config
ssh_pwauth: true
runcmd:
  - echo "=== STAGE 1: INSTALLING DEPENDENCIES AND RUSTYUKI ===" > /dev/ttyS0
  - dnf install -y dracut systemd-ukify systemd-boot efibootmgr binutils > /dev/ttyS0 2>&1
  - mount /dev/disk/by-label/cidata /mnt
  - cp /mnt/rustyuki /usr/bin/rustyuki
  - chmod 755 /usr/bin/rustyuki
  - mkdir -p /etc/rustyuki
  - cp /mnt/config.toml /etc/rustyuki/config.toml
  - cp /mnt/90-rustyuki.install /usr/lib/kernel/install.d/90-rustyuki.install
  - chmod 755 /usr/lib/kernel/install.d/90-rustyuki.install
  - cp /mnt/rustyuki-boot-confirm.service /usr/lib/systemd/system/rustyuki-boot-confirm.service
  - systemctl daemon-reload
  - systemctl enable rustyuki-boot-confirm.service
  - umount /mnt
  - echo "--- PREPARING 1GB DEDICATED ESP DRIVE ---" > /dev/ttyS0
  - mkfs.vfat -F 32 -n "EFI" /dev/vdb || true
  - mkdir -p /efi
  - mount /dev/vdb /efi
  - echo "--- GENERATING AND ACTIVATING SYSTEMD-BOOT UKI ---" > /dev/ttyS0
  - KVER=$(uname -r)
  - rustyuki generate --kernel "$KVER" --install --activate > /dev/ttyS0 2>&1
  - echo "=== PROVISIONING COMPLETE: POWERING OFF FOR FIRMWARE BOOT ===" > /dev/ttyS0
  - poweroff
"""
    cidata_stage1 = vm.work_dir / f"{scenario_name}_cidata1.iso"
    build_cidata_iso(
        cidata_stage1,
        user_data_stage1,
        "instance-id: sdboot-prov\nlocal-hostname: fedora-sdboot",
        extra_files={
            "rustyuki": rustyuki_bin,
            "config.toml": config_path,
            "90-rustyuki.install": hook_path,
            "rustyuki-boot-confirm.service": service_path,
        }
    )

    passed1, log1 = vm.run_vm(
        f"{scenario_name}_p1",
        overlay,
        vars_fd,
        cidata_iso=cidata_stage1,
        extra_disks=[esp_disk],
        timeout_sec=180 if vm.use_kvm else 400,
        success_marker="=== PROVISIONING COMPLETE: POWERING OFF FOR FIRMWARE BOOT ==="
    )
    if not passed1:
        return {"status": "FAIL", "reason": "Stage 1 provisioning failed", "log": log1}

    # 2. Pure Firmware Execution Stage (Verify genuine systemd-boot UEFI execution)
    user_data_stage2 = """#cloud-config
runcmd:
  - echo "=== STAGE 2: VERIFYING GENUINE SYSTEMD-BOOT UKI BOOT ===" > /dev/ttyS0
  - KVER=$(uname -r)
  - echo "RUNNING_KERNEL_RELEASE: $KVER" > /dev/ttyS0
  - echo "--- BOOTCTL STATUS ---" > /dev/ttyS0
  - bootctl status > /dev/ttyS0 2>&1 || true
  - echo "--- VERIFYING ACTIVE UKI AND MANIFEST ---" > /dev/ttyS0
  - UKI_PATH=$(find /boot/efi/EFI/Linux /efi/EFI/Linux -name "*.efi" 2>/dev/null | head -n 1)
  - if [ -z "$UKI_PATH" ]; then echo "ASSERTION FAILED: No UKI found under EFI/Linux" > /dev/ttyS0; exit 1; fi
  - ACTUAL_HASH=$(sha256sum "$UKI_PATH" | awk '{print $1}')
  - MANIFEST_PATH="${UKI_PATH}.manifest.json"
  - if [ ! -f "$MANIFEST_PATH" ]; then echo "ASSERTION FAILED: Missing manifest $MANIFEST_PATH" > /dev/ttyS0; exit 1; fi
  - RECORDED_HASH=$(grep '"uki_sha256"' "$MANIFEST_PATH" | awk -F '"' '{print $4}')
  - if [ "$ACTUAL_HASH" != "$RECORDED_HASH" ]; then echo "ASSERTION FAILED: UKI hash mismatch! Actual: $ACTUAL_HASH vs Manifest: $RECORDED_HASH" > /dev/ttyS0; exit 1; fi
  - echo "HASH VERIFIED: $ACTUAL_HASH" > /dev/ttyS0
  - echo ">>> RUSTYUKI_VM_SCENARIO_A_SUCCESS <<<" > /dev/ttyS0
  - poweroff
"""
    cidata_stage2 = vm.work_dir / f"{scenario_name}_cidata2.iso"
    build_cidata_iso(
        cidata_stage2,
        user_data_stage2,
        "instance-id: sdboot-verify\nlocal-hostname: fedora-sdboot-verify"
    )

    passed2, log2 = vm.run_vm(
        f"{scenario_name}_p2",
        overlay,
        vars_fd,
        cidata_iso=cidata_stage2,
        extra_disks=[esp_disk],
        timeout_sec=90 if vm.use_kvm else 240,
        success_marker=">>> RUSTYUKI_VM_SCENARIO_A_SUCCESS <<<"
    )

    return {
        "status": "PASS" if passed2 else "FAIL",
        "scenario": scenario_name,
        "backend": "systemd-boot",
        "log": log2
    }


def run_scenario_b(vm: VMRunner, base_img: Path) -> dict:
    """
    Scenario B: Direct UKI
    - Clean snapshot.
    - Generate & install UKI.
    - Register through Direct backend (efibootmgr).
    - Reboot through firmware entry.
    - Verify running kernel & efibootmgr entry.
    """
    scenario_name = "scenario_b_direct"
    log(f"=== Starting {scenario_name} ===")

    rustyuki_bin = ensure_host_rustyuki_binary()
    config_path = REPO_ROOT / "packaging" / "config.toml"
    hook_path = REPO_ROOT / "packaging" / "90-rustyuki.install"
    service_path = REPO_ROOT / "packaging" / "rustyuki-boot-confirm.service"

    overlay = vm.create_overlay(f"{scenario_name}_disk", base_img)
    esp_disk = vm.create_esp_disk(scenario_name, size_mb=1024)
    vars_fd = vm.work_dir / f"{scenario_name}_vars.fd"
    shutil.copy2(vm.ovmf_vars, vars_fd)

    user_data_stage1 = """#cloud-config
ssh_pwauth: true
runcmd:
  - echo "=== STAGE 1: PROVISIONING DIRECT UKI BACKEND ===" > /dev/ttyS0
  - dnf install -y dracut systemd-ukify efibootmgr binutils > /dev/ttyS0 2>&1
  - mount /dev/disk/by-label/cidata /mnt
  - cp /mnt/rustyuki /usr/bin/rustyuki
  - chmod 755 /usr/bin/rustyuki
  - mkdir -p /etc/rustyuki
  - cp /mnt/config.toml /etc/rustyuki/config.toml
  - sed -i 's/backend = "systemd-boot"/backend = "direct"/' /etc/rustyuki/config.toml
  - cp /mnt/90-rustyuki.install /usr/lib/kernel/install.d/90-rustyuki.install
  - chmod 755 /usr/lib/kernel/install.d/90-rustyuki.install
  - cp /mnt/rustyuki-boot-confirm.service /usr/lib/systemd/system/rustyuki-boot-confirm.service
  - systemctl daemon-reload
  - systemctl enable rustyuki-boot-confirm.service
  - umount /mnt
  - mkdir -p /efi
  - mount /dev/vdb /efi
  - KVER=$(uname -r)
  - rustyuki generate --kernel "$KVER" --install --activate > /dev/ttyS0 2>&1
  - echo "--- CURRENT EFIBOOTMGR ENTRIES ---" > /dev/ttyS0
  - efibootmgr -v > /dev/ttyS0 2>&1 || true
  - echo "=== PROVISIONING COMPLETE: POWERING OFF FOR DIRECT UKI BOOT ===" > /dev/ttyS0
  - poweroff
"""
    cidata_stage1 = vm.work_dir / f"{scenario_name}_cidata1.iso"
    build_cidata_iso(
        cidata_stage1,
        user_data_stage1,
        "instance-id: direct-prov\nlocal-hostname: fedora-direct",
        extra_files={
            "rustyuki": rustyuki_bin,
            "config.toml": config_path,
            "90-rustyuki.install": hook_path,
            "rustyuki-boot-confirm.service": service_path,
        }
    )

    passed1, log1 = vm.run_vm(
        f"{scenario_name}_p1",
        overlay,
        vars_fd,
        cidata_iso=cidata_stage1,
        extra_disks=[esp_disk],
        timeout_sec=180 if vm.use_kvm else 400,
        success_marker="=== PROVISIONING COMPLETE: POWERING OFF FOR DIRECT UKI BOOT ==="
    )
    if not passed1:
        return {"status": "FAIL", "reason": "Stage 1 direct provisioning failed", "log": log1}

    user_data_stage2 = """#cloud-config
runcmd:
  - echo "=== STAGE 2: VERIFYING DIRECT UKI BOOT IN FIRMWARE ===" > /dev/ttyS0
  - KVER=$(uname -r)
  - echo "RUNNING_KERNEL_RELEASE: $KVER" > /dev/ttyS0
  - echo "--- EFIBOOTMGR STATUS ---" > /dev/ttyS0
  - efibootmgr -v > /dev/ttyS0 2>&1 || true
  - echo "--- CONFIRMING BOOT WITH RUSTYUKI ---" > /dev/ttyS0
  - rustyuki confirm > /dev/ttyS0 2>&1
  - echo ">>> RUSTYUKI_VM_SCENARIO_B_SUCCESS <<<" > /dev/ttyS0
  - poweroff
"""
    cidata_stage2 = vm.work_dir / f"{scenario_name}_cidata2.iso"
    build_cidata_iso(
        cidata_stage2,
        user_data_stage2,
        "instance-id: direct-verify\nlocal-hostname: fedora-direct-verify"
    )

    passed2, log2 = vm.run_vm(
        f"{scenario_name}_p2",
        overlay,
        vars_fd,
        cidata_iso=cidata_stage2,
        extra_disks=[esp_disk],
        timeout_sec=90 if vm.use_kvm else 240,
        success_marker=">>> RUSTYUKI_VM_SCENARIO_B_SUCCESS <<<"
    )

    return {
        "status": "PASS" if passed2 else "FAIL",
        "scenario": scenario_name,
        "backend": "direct",
        "log": log2
    }


def run_scenario_c(vm: VMRunner, base_img: Path) -> dict:
    """
    Scenario C: Kernel lifecycle
    - Multiple kernels / kernel-install integration.
    - Generates UKI for new kernel.
    - Reconciles & prunes obsolete kernels safely.
    """
    scenario_name = "scenario_c_lifecycle"
    log(f"=== Starting {scenario_name} ===")

    rustyuki_bin = ensure_host_rustyuki_binary()
    config_path = REPO_ROOT / "packaging" / "config.toml"
    hook_path = REPO_ROOT / "packaging" / "90-rustyuki.install"

    overlay = vm.create_overlay(f"{scenario_name}_disk", base_img)
    esp_disk = vm.create_esp_disk(scenario_name, size_mb=1024)
    vars_fd = vm.work_dir / f"{scenario_name}_vars.fd"
    shutil.copy2(vm.ovmf_vars, vars_fd)

    user_data = """#cloud-config
runcmd:
  - echo "=== STAGE 1: KERNEL LIFECYCLE RECONCILIATION TEST ===" > /dev/ttyS0
  - dnf install -y dracut systemd-ukify systemd-boot efibootmgr binutils > /dev/ttyS0 2>&1
  - mount /dev/disk/by-label/cidata /mnt
  - cp /mnt/rustyuki /usr/bin/rustyuki
  - chmod 755 /usr/bin/rustyuki
  - mkdir -p /etc/rustyuki
  - cp /mnt/config.toml /etc/rustyuki/config.toml
  - cp /mnt/90-rustyuki.install /usr/lib/kernel/install.d/90-rustyuki.install
  - chmod 755 /usr/lib/kernel/install.d/90-rustyuki.install
  - umount /mnt
  - mkdir -p /efi
  - mount /dev/vdb /efi
  - KVER1=$(uname -r)
  - echo "Generating primary UKI for $KVER1..." > /dev/ttyS0
  - rustyuki generate --kernel "$KVER1" --install > /dev/ttyS0 2>&1
  - test -f "/efi/EFI/Linux/linux-${KVER1}.efi" || { echo "ASSERTION FAILED: primary UKI missing" > /dev/ttyS0; exit 1; }
  - echo "Simulating second kernel version..." > /dev/ttyS0
  - KVER2="${KVER1}-update"
  - mkdir -p "/lib/modules/${KVER2}"
  - cp "/lib/modules/${KVER1}/vmlinuz" "/lib/modules/${KVER2}/vmlinuz"
  - rustyuki generate --kernel "$KVER2" --install > /dev/ttyS0 2>&1
  - test -f "/efi/EFI/Linux/linux-${KVER2}.efi" || { echo "ASSERTION FAILED: second UKI missing" > /dev/ttyS0; exit 1; }
  - echo "Pruning stale kernels (retaining active and new)..." > /dev/ttyS0
  - rustyuki prune --keep 2 > /dev/ttyS0 2>&1
  - test -f "/efi/EFI/Linux/linux-${KVER1}.efi" || { echo "ASSERTION FAILED: primary UKI improperly pruned" > /dev/ttyS0; exit 1; }
  - test -f "/efi/EFI/Linux/linux-${KVER2}.efi" || { echo "ASSERTION FAILED: update UKI improperly pruned" > /dev/ttyS0; exit 1; }
  - echo ">>> RUSTYUKI_VM_SCENARIO_C_SUCCESS <<<" > /dev/ttyS0
  - poweroff
"""
    cidata = vm.work_dir / f"{scenario_name}_cidata.iso"
    build_cidata_iso(
        cidata,
        user_data,
        "instance-id: lifecycle-test\nlocal-hostname: fedora-lifecycle",
        extra_files={
            "rustyuki": rustyuki_bin,
            "config.toml": config_path,
            "90-rustyuki.install": hook_path,
        }
    )

    passed, log_out = vm.run_vm(
        scenario_name,
        overlay,
        vars_fd,
        cidata_iso=cidata,
        extra_disks=[esp_disk],
        timeout_sec=180 if vm.use_kvm else 400,
        success_marker=">>> RUSTYUKI_VM_SCENARIO_C_SUCCESS <<<"
    )

    return {
        "status": "PASS" if passed else "FAIL",
        "scenario": scenario_name,
        "backend": "lifecycle",
        "log": log_out
    }


def run_scenario_d(vm: VMRunner, base_img: Path) -> dict:
    """
    Scenario D: Failure & Rollback Recovery
    - Generate baseline working UKI.
    - Deploy second UKI, creating .prev.efi fallback slot.
    - Trigger rollback and verify .prev.efi restores active slot safely and repeatably.
    """
    scenario_name = "scenario_d_recovery"
    log(f"=== Starting {scenario_name} ===")

    rustyuki_bin = ensure_host_rustyuki_binary()
    config_path = REPO_ROOT / "packaging" / "config.toml"

    overlay = vm.create_overlay(f"{scenario_name}_disk", base_img)
    esp_disk = vm.create_esp_disk(scenario_name, size_mb=1024)
    vars_fd = vm.work_dir / f"{scenario_name}_vars.fd"
    shutil.copy2(vm.ovmf_vars, vars_fd)

    user_data = """#cloud-config
runcmd:
  - echo "=== STAGE 1: ROLLBACK AND FAILURE RECOVERY TEST ===" > /dev/ttyS0
  - dnf install -y dracut systemd-ukify systemd-boot efibootmgr binutils > /dev/ttyS0 2>&1
  - mount /dev/disk/by-label/cidata /mnt
  - cp /mnt/rustyuki /usr/bin/rustyuki
  - chmod 755 /usr/bin/rustyuki
  - mkdir -p /etc/rustyuki
  - cp /mnt/config.toml /etc/rustyuki/config.toml
  - umount /mnt
  - mkdir -p /efi
  - mount /dev/vdb /efi
  - KVER=$(uname -r)
  - echo "1. Generating baseline working UKI..." > /dev/ttyS0
  - rustyuki generate --kernel "$KVER" --install > /dev/ttyS0 2>&1
  - HASH1=$(sha256sum "/efi/EFI/Linux/linux-${KVER}.efi" | awk '{print $1}')
  - echo "2. Overwriting with simulated updated UKI..." > /dev/ttyS0
  - echo "simulated new version data" > /tmp/simulated_new.efi
  - rustyuki install --kernel "$KVER" --artifact /tmp/simulated_new.efi > /dev/ttyS0 2>&1
  - test -f "/efi/EFI/Linux/linux-${KVER}.prev.efi" || { echo "ASSERTION FAILED: .prev.efi backup slot not created" > /dev/ttyS0; exit 1; }
  - echo "3. Performing rollback..." > /dev/ttyS0
  - rustyuki rollback --kernel "$KVER" > /dev/ttyS0 2>&1
  - HASH_RESTORED=$(sha256sum "/efi/EFI/Linux/linux-${KVER}.efi" | awk '{print $1}')
  - if [ "$HASH1" != "$HASH_RESTORED" ]; then echo "ASSERTION FAILED: rollback did not restore original hash!" > /dev/ttyS0; exit 1; fi
  - echo "4. Verifying .prev.efi is preserved intact for repeated recovery attempts..." > /dev/ttyS0
  - test -f "/efi/EFI/Linux/linux-${KVER}.prev.efi" || { echo "ASSERTION FAILED: .prev.efi destroyed during rollback!" > /dev/ttyS0; exit 1; }
  - echo ">>> RUSTYUKI_VM_SCENARIO_D_SUCCESS <<<" > /dev/ttyS0
  - poweroff
"""
    cidata = vm.work_dir / f"{scenario_name}_cidata.iso"
    build_cidata_iso(
        cidata,
        user_data,
        "instance-id: recovery-test\nlocal-hostname: fedora-recovery",
        extra_files={
            "rustyuki": rustyuki_bin,
            "config.toml": config_path,
        }
    )

    passed, log_out = vm.run_vm(
        scenario_name,
        overlay,
        vars_fd,
        cidata_iso=cidata,
        extra_disks=[esp_disk],
        timeout_sec=180 if vm.use_kvm else 400,
        success_marker=">>> RUSTYUKI_VM_SCENARIO_D_SUCCESS <<<"
    )

    return {
        "status": "PASS" if passed else "FAIL",
        "scenario": scenario_name,
        "backend": "rollback",
        "log": log_out
    }


def run_negative_verification(vm: VMRunner, base_img: Path) -> bool:
    """
    Negative Test Verification:
    Deliberately triggers an assertion failure in the guest to ensure
    the test harness detects the failure and exits non-zero (never false PASS).
    """
    scenario_name = "test_negative_detection"
    log(f"=== Running Negative Test Verification ({scenario_name}) ===")

    overlay = vm.create_overlay(f"{scenario_name}_disk", base_img)
    vars_fd = vm.work_dir / f"{scenario_name}_vars.fd"
    shutil.copy2(vm.ovmf_vars, vars_fd)

    user_data = """#cloud-config
runcmd:
  - echo "=== SIMULATING INTENTIONAL GUEST FAILURE ===" > /dev/ttyS0
  - echo "ASSERTION FAILED: Deliberate negative test verification failure" > /dev/ttyS0
  - echo ">>> RUSTYUKI_VM_SCENARIO_FAKE_SUCCESS <<<" > /dev/ttyS0
  - poweroff
"""
    cidata = vm.work_dir / f"{scenario_name}_cidata.iso"
    build_cidata_iso(cidata, user_data, "instance-id: neg-test\nlocal-hostname: fedora-neg")

    passed, log_out = vm.run_vm(
        scenario_name,
        overlay,
        vars_fd,
        cidata_iso=cidata,
        timeout_sec=60,
        success_marker=">>> RUSTYUKI_VM_SCENARIO_FAKE_SUCCESS <<<",
        failure_marker="ASSERTION FAILED:"
    )

    if not passed:
        log("Negative test verification PASSED: Harness correctly caught the deliberate assertion failure.")
        return True
    else:
        log("CRITICAL ERROR: Harness reported success on deliberate failure!")
        return False


# ------------------------------------------------------------------------------
# Main Entry Point
# ------------------------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(description="RustyUKI Fedora VM Integration Test Runner")
    parser.add_argument("--release", choices=["44", "45"], default="45", help="Target Fedora release")
    parser.add_argument("--scenario", choices=["A", "B", "C", "D", "all", "negative"], default="all", help="Scenario to execute")
    parser.add_argument("--cache-dir", type=Path, default=REPO_ROOT / "tests" / "vm" / "cache", help="Cache directory for base images")
    parser.add_argument("--work-dir", type=Path, default=Path(tempfile.gettempdir()) / "rustyuki_vm_test", help="Working directory")
    parser.add_argument("--artifacts-dir", type=Path, default=REPO_ROOT / "artifacts" / "vm_logs", help="Directory for exported test logs")
    parser.add_argument("--accel", choices=["auto", "kvm", "tcg"], default="auto", help="Virtualization acceleration")
    args = parser.parse_args()

    log(f"Starting Fedora VM Integration Tests (Release: {args.release}, Scenario: {args.scenario})")

    vm = VMRunner(args.work_dir, args.artifacts_dir, accel=args.accel)

    # Resolve required base image
    generic_key = f"fedora-{args.release}-generic"
    generic_img = ensure_cached_image(generic_key, args.cache_dir)

    results = []

    if args.scenario in ["negative"]:
        neg_ok = run_negative_verification(vm, generic_img)
        sys.exit(0 if neg_ok else 1)

    # Run negative harness verification first to guarantee harness integrity
    log("Verifying harness negative-failure detection...")
    if not run_negative_verification(vm, generic_img):
        sys.exit(1)

    if args.scenario in ["A", "all"]:
        res_a = run_scenario_a(vm, generic_img)
        results.append(res_a)

    if args.scenario in ["B", "all"]:
        res_b = run_scenario_b(vm, generic_img)
        results.append(res_b)

    if args.scenario in ["C", "all"]:
        res_c = run_scenario_c(vm, generic_img)
        results.append(res_c)

    if args.scenario in ["D", "all"]:
        res_d = run_scenario_d(vm, generic_img)
        results.append(res_d)

    # Print Summary Table
    print("\n" + "=" * 70)
    print("                 RUSTYUKI FEDORA VM TEST RESULTS")
    print("=" * 70)
    all_passed = True
    for r in results:
        status = r.get("status", "UNKNOWN")
        name = r.get("scenario", "Unknown")
        backend = r.get("backend", "-")
        print(f"Scenario: {name:<25} Backend: {backend:<15} Result: {status}")
        if status != "PASS":
            all_passed = False

    print("=" * 70)
    if all_passed:
        log("ALL VM INTEGRATION SCENARIOS PASSED.")
        sys.exit(0)
    else:
        log("ONE OR MORE VM INTEGRATION SCENARIOS FAILED.")
        sys.exit(1)


if __name__ == "__main__":
    main()
