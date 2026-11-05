# Backups & Emergency Recovery Runbook

Bootloader modifications directly impact system availability. This guide covers mandatory pre-flight backups, diagnostic strategies, and step-by-step recovery procedures from a Live USB.

---

## Pre-Flight Backups

Perform these backup steps before generating or installing your first UKI.

### 1. Back Up the EFI System Partition (ESP)

Archive your entire `/boot/efi` partition to an external USB drive:

```bash
# Locate your ESP partition
lsblk -o NAME,PARTTYPE,FSTYPE,MOUNTPOINT | grep -i efi

# Create tarball archive
sudo tar -czvf /path/to/external-usb/esp-backup-$(date +%Y%m%d).tar.gz /boot/efi/
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
# Diff configured cmdline against active running cmdline
diff -u <(cat /etc/kernel/cmdline) <(cat /proc/cmdline)

# Verify the root partition UUID matches findmnt
findmnt -n -o UUID /
```

---

## Failure Modes & Diagnostic Matrix

| Symptom | Root Cause | Fix |
|---|---|---|
| **Drastic drop to emergency shell (`dracut-initqueue` timeout)** | Kernel cmdline has missing or incorrect `root=UUID=...` | Boot via GRUB fallback or Live USB, fix `/etc/kernel/cmdline`, rebuild UKI. |
| **Firmware skips entry and boots BIOS/Setup** | Secure Boot enabled with an unsigned UKI binary | Disable Secure Boot in UEFI Setup, or sign the UKI binary before booting. |
| **Kernel panic: `Unable to mount root fs`** | Initramfs was built without necessary storage drivers (e.g., missing NVMe or filesystem module) | Add missing drivers via `[dracut] extra_args = ["--force-drivers", "..."]` in `/etc/uki/uki.conf`. |
| **`efibootmgr` fails with `No space left on device`** | UEFI NVRAM variable storage is full | Delete old or duplicate entries using `efibootmgr -b <XXXX> -B`. |
| **ESP out of space during build** | Multiple kernels or large initramfs images filled the ESP | Run `rustyuki reconcile` to prune stale UKIs, or remove unnecessary kernels via `dnf remove`. |

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
- **Root partition** (e.g. `/dev/nvme0n1p3` or `/dev/sda3`, often `btrfs`, `ext4`, or `xfs`)
- **ESP partition** (e.g. `/dev/nvme0n1p1` or `/dev/sda1`, `vfat` with partition type `c12a7328-f81f-11d2-ba4b-00a0c93ec93b`)

### Step 3: Mount the ESP Partition

```bash
sudo mkdir -p /mnt/target-esp
sudo mount /dev/nvme0n1p1 /mnt/target-esp
```

### Step 4: Choose a Recovery Path

#### Option A: Restore the ESP from Archive (Recommended)

If you made a tarball backup of `/boot/efi`:

```bash
# Restore ESP files
sudo rm -rf /mnt/target-esp/*
sudo tar -xzvf /path/to/external-usb/esp-backup-YYYYMMDD.tar.gz -C /mnt/target-esp/ --strip-components=2
```

#### Option B: Recreate the Fedora GRUB Boot Entry in NVRAM

If GRUB binaries are intact in the ESP (`\EFI\fedora\shimx64.efi` or `grubx64.efi`), restore the UEFI NVRAM boot entry directly:

```bash
sudo efibootmgr --create \
  --disk /dev/nvme0n1 \
  --part 1 \
  --label "Fedora" \
  --loader "\EFI\fedora\shimx64.efi"
```

#### Option C: Remove a Broken UKI Boot Entry

If an invalid entry is preventing clean boot sequences:

```bash
# List all entries
sudo efibootmgr -v

# Delete the problematic entry (e.g. Boot0007)
sudo efibootmgr -b 0007 -B
```

### Step 5: Unmount and Reboot

```bash
sudo umount /mnt/target-esp
sudo reboot
```
Remove the Live USB and verify the machine boots normally.
