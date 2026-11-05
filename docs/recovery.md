# Backups & Emergency Recovery Runbook

Bootloader modifications directly impact system availability. This guide covers pre-flight backups, automatic rollback commands, diagnostic strategies, and recovery procedures from a Live USB.

---

## 1. Instant Fallback Slot Rollback (No Live USB Required)

If an updated UKI boots into a regressed state or fails to start userspace properly:

1. **Reboot**:
   Because RustyUKI arms trial boots via `BootNext` or `bootctl set-oneshot`, an interrupted boot or power cycle immediately falls back to your previous working bootloader or prior kernel entry.

2. **Roll Back via RustyUKI**:
   Once booted into any working shell or rescue prompt:
   ```bash
   sudo rustyuki rollback --kernel-version <kernel_version>
   ```
   This atomically restores `linux-<version>.prev.efi` over `linux-<version>.efi` without requiring kernel recompilation.

---

## 2. Pre-Flight Backups

Perform these backup steps before generating or installing your first UKI.

### 1. Back Up the EFI System Partition (ESP)

Archive your entire ESP partition to an external USB drive:

```bash
# Locate your ESP partition
lsblk -o NAME,PARTTYPE,FSTYPE,MOUNTPOINT | grep -i efi

# Create tarball archive
sudo tar -czvf /path/to/external-usb/esp-backup-$(date +%Y%m%d).tar.gz /efi/ # or /boot/efi/
```

> [!CAUTION]
> Storing backups solely on the internal disk you are modifying will not help if the system cannot boot. Copy the archive to a separate USB drive.

### 2. Back Up EFI NVRAM Boot Entries

Export all existing UEFI boot entries:

```bash
efibootmgr -v > /path/to/external-usb/efibootmgr-backup-$(date +%Y%m%d).txt
```

### 3. Verify Kernel Command Line and Root UUID

A missing or mismatched `root=` parameter will cause the kernel to drop into an `initramfs` emergency shell.

```bash
# Verify configured cmdline
cat /etc/kernel/cmdline

# Verify the root partition UUID matches findmnt
findmnt -n -o UUID /
```

---

## Failure Modes & Diagnostic Matrix

| Symptom | Root Cause | Fix |
|---|---|---|
| **Drop to emergency shell (`dracut-initqueue` timeout)** | Kernel cmdline has missing or incorrect `root=UUID=...` | Boot via fallback entry, edit `/etc/kernel/cmdline`, run `sudo rustyuki install`. |
| **Firmware skips entry and boots BIOS/Setup** | Secure Boot enabled with an unsigned UKI binary | Configure `[signing]` in `/etc/rustyuki/config.toml` (`tool = "sbsign"` or `"pesign"`), or enroll key. |
| **Kernel panic: `Unable to mount root fs`** | Initramfs was built without necessary storage drivers (e.g., missing NVMe or filesystem module) | Add missing drivers via `[dracut] extra_args = ["--force-drivers", "..."]` in `config.toml`. |
| **`efibootmgr` fails with `No space left on device`** | UEFI NVRAM variable storage is full | Delete stale or duplicate entries using `efibootmgr -b <XXXX> -B`. |
| **ESP out of space during build** | Multiple kernels or large initramfs images filled the ESP | Run `rustyuki reconcile` to prune stale UKIs with valid manifests, or clean older packages with `dnf remove`. |
| **`rustyuki confirm` fails with health error** | System booted into degraded or emergency target | Run `systemctl --failed` to inspect broken units. Resolve issues before permanent confirmation. |

---

## Live USB Recovery Runbook

If your machine fails to boot and cannot reach a fallback bootloader, follow this procedure using a Fedora Live USB.

### Step 1: Boot the Live USB
1. Insert your Fedora Workstation Live USB.
2. Power on the machine and press the firmware boot menu key (`F12`, `F11`, `F10`, or `Esc` depending on motherboard vendor).
3. Select the USB device and boot to the Live desktop.
4. Open a terminal.

### Step 2: Identify Disks and Partitions

```bash
lsblk -o NAME,FSTYPE,PARTTYPE,SIZE,MOUNTPOINT
```

Identify:
- **Root partition** (e.g. `/dev/nvme0n1p3`, often `btrfs`, `ext4`, or `xfs`)
- **ESP partition** (e.g. `/dev/nvme0n1p1`, `vfat`)

### Step 3: Mount the ESP Partition

```bash
sudo mkdir -p /mnt/target-esp
sudo mount /dev/nvme0n1p1 /mnt/target-esp
```

### Step 4: Restore Backup Slot or Archive

#### Option A: Restore the Fallback Slot Directly
If `linux-<version>.prev.efi` exists on the mounted ESP:
```bash
cd /mnt/target-esp/EFI/Linux/
sudo cp -a linux-<version>.prev.efi linux-<version>.efi
sudo sync
```

#### Option B: Restore the ESP from Archive
If you made a tarball backup of `/efi`:
```bash
sudo rm -rf /mnt/target-esp/*
sudo tar -xzvf /path/to/external-usb/esp-backup-YYYYMMDD.tar.gz -C /mnt/target-esp/ --strip-components=1
sudo sync
```

#### Option C: Recreate the Boot Entry in NVRAM
If firmware lost the entry:
```bash
sudo efibootmgr --create --disk /dev/nvme0n1 --part 1 --label "Linux UKI Fallback" --loader '\EFI\Linux\linux-<version>.efi'
```
Reboot and select the entry from firmware.
