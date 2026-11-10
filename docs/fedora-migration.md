# Migrating an Existing Fedora Installation to RustyUKI

This guide walks you through migrating an existing, conventional Fedora Workstation or Server installation (Fedora 39, 40, 41, 42, 43, 44, or 45+) from traditional GRUB2/BLS boot to **RustyUKI** Unified Kernel Images (UKIs).

---

## Overview

### Traditional Fedora Boot vs. Unified Kernel Images

| Feature | Traditional Fedora (GRUB2 + BLS) | RustyUKI (Unified Kernel Image) |
|---|---|---|
| **Boot Chain** | Firmware → Shim → GRUB2 → Kernel + Initramfs | Firmware → Monolithic UKI (`.efi`) |
| **Artifacts on Disk** | Separate `vmlinuz`, `initramfs`, and BLS `.conf` entries in `/boot` | Single self-contained PE binary (`/boot/efi/EFI/Linux/linux-<version>.efi`) |
| **Command Line Security** | Stored in cleartext configuration files (`/boot/loader/entries/`, `/etc/default/grub`) | Embedded cryptographically inside the signed PE `.cmdline` section |
| **Firmware Execution** | Relies on GRUB's filesystem and partition drivers | Native UEFI PE/COFF loader (`efibootmgr`) or clean `systemd-boot` discovery |
| **Boot Time** | Slower; multiple boot stages, disk timeouts, menu delays | Near-instant; direct firmware execution without intermediate bootloaders |
| **Failure Recovery** | Manual GRUB menu selection | Automated `BootNext` trial booting and instant `.prev.efi` fallback slots |

### The Safety-First Approach

Migrating to RustyUKI is **non-destructive to your existing GRUB setup**. You do not need to delete GRUB, remove shim, or wipe your existing `/boot` partition. GRUB remains registered in your UEFI NVRAM as a permanent or fallback entry until you are completely satisfied with your new UKI configuration.

---

## The Core Challenge: EFI System Partition (ESP) Sizing

### Why Fedora's Default ESP is Too Small

On a standard Fedora installation, the Anaconda installer traditionally provisions an EFI System Partition (ESP) of only **200 MiB to 600 MiB** at `/boot/efi`.

A Unified Kernel Image bundles:
1. The compressed Linux kernel binary (`vmlinuz`, ~15 MiB)
2. The initramfs image including storage drivers, microcode, and systemd modules (~60–100 MiB)
3. Distribution metadata and kernel command line parameters

This produces a single `.efi` binary of roughly **75 MiB to 120 MiB** per kernel.

Fedora retains three kernels by default (`installonly_limit=3`). With RustyUKI's safety architecture:
- 3 active kernels: ~250–350 MiB
- Previous working fallback slots (`.prev.efi`): ~250–350 MiB
- Staging headroom during kernel upgrades (`dnf update kernel`): ~100 MiB

> [!IMPORTANT]
> **Minimum recommended ESP size**: **1.0 GiB** (1024 MiB).
> **Ideal recommended ESP size**: **2.0 GiB** (2048 MiB) for multi-kernel retention and peace of mind.
>
> If your existing ESP is only 200–600 MiB, you **must expand the ESP** before installing UKIs, or transactions will eventually fail with `ENOSPC` (No space left on device).

---

## Migration Workflow

```mermaid
flowchart TD
    A["Phase 1: Pre-Migration Backup (Running System)"] --> B["Phase 2: Expand ESP via Live USB"]
    B --> C["Phase 3: Boot Back & Install RustyUKI"]
    C --> D["Phase 4: Configure /etc/kernel/cmdline & config.toml"]
    D --> E["Phase 5: Generate & Install First UKI"]
    E --> F["Phase 6: Zero-Risk Trial Boot (BootNext)"]
    F --> G{"Trial Boot Successful?"}
    G -- "Yes" --> H["Phase 7: Confirm & Enable Kernel Hook"]
    G -- "No / Hangs" --> I["Reboot into GRUB & Inspect Logs"]
    I --> D
```

---

## Phase 1: Pre-Migration Backup & Assessment

Perform these steps from your **running Fedora installation** before touching any partitions.

### 1. Check Your Current Disk & Partition Layout

Inspect your partition layout, filesystem types, and current ESP usage:

```bash
lsblk -o NAME,SIZE,FSTYPE,LABEL,MOUNTPOINTS
df -h /boot/efi
```

Typically, on a UEFI Fedora machine, you will see:
- `/dev/nvme0n1p1` (or `sda1`): `vfat`, mounted at `/boot/efi` (ESP, usually 200–600M)
- `/dev/nvme0n1p2` (or `sda2`): `ext4`, mounted at `/boot` (usually 1G)
- `/dev/nvme0n1p3` (or `sda3`): `btrfs`, mounted at `/` and `/home` (root subvolumes)

### 2. Back Up the Existing ESP

Archive your entire current EFI directory to a safe location (e.g. your home directory or an external flash drive):

```bash
sudo tar -czvf /root/esp-backup-$(date +%Y%m%d).tar.gz /boot/efi
```

### 3. Back Up UEFI NVRAM Boot Entries

Export all existing firmware boot entries:

```bash
efibootmgr -v > /root/efibootmgr-backup-$(date +%Y%m%d).txt
```

### 4. Back Up `/etc/fstab`

```bash
sudo cp /etc/fstab /etc/fstab.bak
```

### 5. Capture Your Kernel Command Line

Verify and record your current working kernel command line parameters:

```bash
cat /proc/cmdline
```

Note the `root=UUID=...` and any specific parameters (e.g., `rhgb`, `quiet`, `rd.luks.uuid=...`).

### 6. Prepare a Fedora Live USB

Flash an official Fedora Workstation Live image (or GParted Live image) to a USB drive using Fedora Media Writer or `dd`. You will need this to resize the ESP without partition lock contention.

---

## Phase 2: Expanding the EFI System Partition (ESP)

Because the ESP is located at the physical beginning of your drive and is immediately followed by other active partitions (such as `/boot` or your root filesystem), it cannot be grown while those filesystems are mounted and in use.

### Step 1: Boot into the Live USB

1. Insert your Fedora Workstation Live USB.
2. Reboot your computer and press the boot menu key (`F12`, `F11`, `F10`, or `Esc` depending on your motherboard).
3. Select the Live USB and choose **"Try Fedora"**.
4. Open the **Terminal**.

---

### Method A: Graphical Resizing with GParted (Recommended)

1. In the Live desktop, install and launch GParted:
   ```bash
   sudo dnf install -y gparted && sudo gparted
   ```
2. Select your internal disk from the top-right dropdown (e.g. `/dev/nvme0n1` or `/dev/sda`).
3. Examine the partition order:
   - **Partition 1 (ESP)**: `vfat`, size ~200–600 MiB.
   - **Partition 2 (`/boot`)**: `ext4`, size ~1 GiB.
   - **Partition 3 (Root)**: `btrfs` or `ext4`/`xfs`.

4. **Create space for the ESP**:
   - If you have an adjacent `/boot` partition (`Partition 2`):
     - Right-click Partition 3 (Root) → **Resize/Move** → Shrink the start/left boundary by 1.5 GiB (freeing up 1536 MiB preceding space).
     - Right-click Partition 2 (`/boot`) → **Resize/Move** → Move Partition 2 to the right into the unallocated space.
   - If your root partition directly follows Partition 1 (no separate `/boot`):
     - Right-click Partition 2 (Root) → **Resize/Move** → Drag the left slider rightward to leave 1.5 GiB of unallocated space before it.

5. **Grow the ESP**:
   - Right-click Partition 1 (ESP) → **Resize/Move**.
   - Drag the right boundary to fill the newly freed unallocated space until the partition is **1.5 GiB to 2.0 GiB**.
   - Ensure the partition type remains `vfat` / `fat32`.

6. **Apply the Changes**:
   - Click the green checkmark (**Apply All Operations**).
   - GParted will relocate partition boundaries, move the filesystem blocks, and run `fatresize` to expand the FAT32 filesystem cleanly.

---

### Method B: Command-Line Resizing (For Headless or Advanced Setups)

If GParted is not available or you are working in a terminal environment:

#### 1. Identify Disk and Block Locations
```bash
sudo parted /dev/nvme0n1 unit MiB print
```

#### 2. Backup ESP Contents to Temporary Storage
```bash
sudo mkdir -p /tmp/esp /tmp/esp-backup
sudo mount /dev/nvme0n1p1 /tmp/esp
sudo cp -a /tmp/esp/* /tmp/esp-backup/
sudo umount /tmp/esp
```

#### 3. Adjust Adjacent Partition Boundaries
If Partition 2 (`/boot`) is immediately adjacent to Partition 1, you can shrink or move it using standard tools:
```bash
# Shrink ext4 /boot filesystem first
sudo e2fsck -f /dev/nvme0n1p2
sudo resize2fs /dev/nvme0n1p2 800M

# Use parted to resize and move partitions
sudo parted /dev/nvme0n1 unit MiB resizepart 1 2048MiB
```

#### 4. Resize or Recreate the FAT Filesystem
If the partition size was changed, expand the filesystem:
```bash
sudo fatresize -s 2048M /dev/nvme0n1p1 || sudo dosfsck -w -r -l -a -v -t /dev/nvme0n1p1
```

> [!CAUTION]
> If you recreate the FAT32 filesystem using `mkfs.fat -F32 /dev/nvme0n1p1`:
> 1. You **must restore** the backed-up files from `/tmp/esp-backup/`.
> 2. The partition will receive a **new UUID**.
> 3. You **must mount your root partition** and update `/etc/fstab` with the new UUID before rebooting:
>    ```bash
>    # Get new ESP UUID
>    NEW_UUID=$(lsblk -no UUID /dev/nvme0n1p1)
>    echo "New ESP UUID is: ${NEW_UUID}"
>
>    # Mount root and update fstab
>    sudo mount /dev/nvme0n1p3 /mnt -o subvol=root # for Btrfs
>    sudo sed -i "s/UUID=[A-Fa-f0-9-]\{9,\}\s*\/boot\/efi/UUID=${NEW_UUID} \/boot\/efi/" /mnt/etc/fstab
>    sudo umount /mnt
>    ```

---

## Phase 3: Boot Back & Install RustyUKI

Reboot your computer, remove the Live USB, and boot back into your installed Fedora system (using your standard GRUB bootloader).

### 1. Verify ESP Expansion

Log into your Fedora system and check that the ESP has expanded:

```bash
df -h /boot/efi
```

The `Size` column should now show **1.5G to 2.0G**, with ample free space.

### 2. Install Required Dependencies

Install the official toolchain packages required for UKI generation, assembly, and UEFI NVRAM management:

```bash
sudo dnf install -y \
  dracut \
  systemd-ukify \
  binutils \
  efibootmgr
```

If you plan to use the `systemd-boot` backend rather than `direct` UEFI booting, also install `systemd-boot`:

```bash
sudo dnf install -y systemd-boot
```

### 3. Install RustyUKI

Install RustyUKI using the prebuilt RPM package or compile from source:

#### Option A: From RPM Package (Recommended)
```bash
sudo dnf install -y ./rustyuki-*.x86_64.rpm
```

#### Option B: From Source
```bash
git clone https://github.com/GlitchSlayed/RustyUKI.git
cd RustyUKI
cargo build --release --locked
sudo install -Dpm0755 target/release/rustyuki /usr/bin/rustyuki
sudo install -Dpm0644 packaging/systemd/rustyuki-boot-confirm.service /usr/lib/systemd/system/rustyuki-boot-confirm.service
sudo mkdir -p /etc/rustyuki
sudo cp packaging/config.toml /etc/rustyuki/config.toml
```

---

## Phase 4: Configure Command Line & Settings

### 1. Configure Persistent Kernel Command Line

In traditional Fedora, GRUB generates command line arguments dynamically. RustyUKI embeds the kernel command line directly into the UKI binary for tamper resistance.

Inspect your running arguments and write them to `/etc/kernel/cmdline`:

```bash
# View active arguments
cat /proc/cmdline

# Create /etc/kernel/cmdline with required parameters:
# (Strip ephemeral parameters like BOOT_IMAGE=)
ROOT_UUID=$(findmnt -n -o UUID /)
echo "root=UUID=${ROOT_UUID} ro quiet rhgb" | sudo tee /etc/kernel/cmdline
```

> [!TIP]
> If you use Btrfs subvolumes, LUKS encryption, or resume devices, make sure they are included:
> ```bash
> root=UUID=<root-uuid> rootflags=subvol=root ro quiet rhgb rd.luks.uuid=<luks-uuid>
> ```

### 2. Configure `/etc/rustyuki/config.toml`

Edit `/etc/rustyuki/config.toml` to configure RustyUKI for your Fedora layout:

```toml
[uki]
# Standard Fedora ESP mount point
esp_path = "/boot/efi"

# Standard BLS Type #2 UKI directory
output_dir = "/boot/efi/EFI/Linux"

# Automatically read /etc/kernel/cmdline
auto_detect_cmdline = true
cmdline_file = "/etc/kernel/cmdline"

# Directory for state tracking
cmdline_state_dir = "/var/lib/uki-build"
cmdline_min_tokens = 3

[bootloader]
# Choose your backend: "direct" (recommended) or "systemd-boot"
# - "direct": Native UEFI firmware entry (efibootmgr). No intermediate bootloader.
# - "systemd-boot": Installs systemd-boot to ESP and discovers UKIs under /EFI/Linux.
backend = "direct"

# Safety: Always arm one-shot trial boots
trial_boot = true

# Durability: Maintain .prev.efi fallback slots
keep_backup = true

# Reconciliation: Prune older UKIs that have valid RustyUKI provenance manifests
prune_stale_images = true

[dracut]
# Additional dracut arguments if needed (e.g. force specific host drivers)
extra_args = []

[ukify]
extra_args = []

[signing]
# Leave disabled initially until first un-signed UKI boots cleanly
enabled = false
```

---

## Phase 5: Generate & Install Your First UKI

Now, build and install the UKI for your active running kernel.

### 1. Perform a Dry Run (Inspection)

Preview the generation and inspect parameters without touching disk:

```bash
sudo rustyuki generate --verbose
```

RustyUKI will invoke `dracut` to stage the initramfs, run `ukify` to assemble the PE binary, and verify that all PE sections (`.linux`, `.initrd`, `.cmdline`, `.osrel`) are present.

### 2. Install the UKI to the ESP

```bash
sudo rustyuki install
```

This stages the build, verifies ESP free space, copies the binary to `/boot/efi/EFI/Linux/linux-<version>.efi`, syncs filesystem blocks, and writes the sidecar provenance manifest (`linux-<version>.efi.manifest.json`).

### 3. Verify the Installed Artifacts

```bash
# Check filesystem
ls -lh /boot/efi/EFI/Linux/

# Check status via RustyUKI CLI
rustyuki status
```

You should see:
- `linux-<version>.efi`
- `linux-<version>.efi.manifest.json`
- `rustyuki status` reporting the UKI as installed, with correct kernel version, digest, and ESP capacity.

---

## Phase 6: Zero-Risk Trial Boot

RustyUKI uses UEFI `BootNext` (or `systemd-boot` oneshot) to implement a **strictly fail-safe trial boot workflow**.

### 1. Arm the Trial Boot

```bash
sudo rustyuki activate --trial
```

What this does:
1. Registers a new UEFI boot entry: `RustyUKI (Linux <version>)` pointing to `\EFI\Linux\linux-<version>.efi`.
2. Sets the UEFI `BootNext` NVRAM variable to point to that new entry.
3. Leaves your existing GRUB `BootOrder` intact.

> [!NOTE]
> The UEFI firmware consumes the `BootNext` pointer on power-on. It applies to **one boot only**.
> If the UKI fails to boot, crashes, or hangs, simply power-cycle or reset your computer. The firmware will automatically fall back to your normal GRUB bootloader!

### 2. Reboot the System

```bash
sudo systemctl reboot
```

Watch the boot sequence:
- The motherboard firmware executes `linux-<version>.efi` directly.
- The Linux kernel initializes hardware, mounts root via the embedded command line, and brings up systemd.
- You should arrive at your standard Fedora login screen or desktop.

---

## Phase 7: Confirm Boot & Automate Kernel Updates

Once you have successfully booted into userspace via your new UKI:

### 1. Permanently Confirm the Bootloader Entry

Log into a terminal and run:

```bash
sudo rustyuki confirm
```

RustyUKI performs late-userspace health checks (verifying that `default.target` is active and system state is healthy) and permanently promotes the UKI to the primary slot in `BootOrder`.

### 2. Enable Automatic Boot Confirmation

Enable the confirmation systemd unit so future trial boots confirm automatically once userspace is verified healthy:

```bash
sudo systemctl enable --now rustyuki-boot-confirm.service
```

### 3. Install the Automatic `kernel-install` Hook

Integrate RustyUKI with Fedora's standard package manager lifecycle:

```bash
sudo rustyuki install-hook
```

This installs `/usr/lib/kernel/install.d/90-rustyuki.install`. Whenever `dnf` installs or removes a kernel (`dnf update kernel`):
- `90-rustyuki.install` builds the new UKI automatically.
- Preserves the previous working UKI as `linux-<version>.prev.efi`.
- Updates boot entries and trial-arms the new kernel for the next reboot.
- Prunes obsolete kernels and manifest records during package removal.

---

## Phase 8: Post-Migration Optimization & Cleanup (Optional)

Once you have completed several successful reboots and a kernel update using RustyUKI:

### 1. Keep GRUB as a Secondary Fallback

You do **not** need to uninstall GRUB. Keeping GRUB installed in `/boot/efi/EFI/fedora/` gives you a permanent backup method accessible from your motherboard's `F12` boot menu.

### 2. Disable Duplicate BLS Entry Creation (Optional)

If you do not want `kernel-install` to generate redundant legacy GRUB loader entries in `/boot/loader/entries/`, you can mask the default loaderentry plugin:

```bash
sudo systemctl mask 90-loaderentry.install
```

Or set the preferred layout in `/etc/kernel/install.conf`:
```ini
layout=uki
```

---

## Troubleshooting & Emergency Runbook

### Problem 1: Emergency Shell: `dracut-initqueue timeout` / `Cannot find root`
- **Cause**: Kernel command line in `/etc/kernel/cmdline` is missing `root=UUID=...` or has an incorrect UUID.
- **Fix**:
  1. Reboot (system will boot into GRUB automatically due to `BootNext` expiry).
  2. Verify your root UUID with `findmnt -no UUID /`.
  3. Update `/etc/kernel/cmdline`.
  4. Regenerate and reinstall: `sudo rustyuki generate --install && sudo rustyuki activate --trial`.

### Problem 2: Firmware Rejects Execution / Jumps to BIOS Setup
- **Cause**: UEFI Secure Boot is enabled, but the UKI is unsigned.
- **Fix**:
  - Either enroll your custom Machine Owner Key (MOK) and configure `[signing]` in `/etc/rustyuki/config.toml` (`tool = "sbsign"`), or temporarily disable Secure Boot in UEFI firmware setup to complete migration.

### Problem 3: `efibootmgr: No space left on device`
- **Cause**: Motherboard NVRAM variable storage is cluttered with stale or dead boot entries.
- **Fix**:
  1. Inspect entries: `efibootmgr -v`.
  2. Remove dead entries: `sudo efibootmgr -b <entry-number> -B`.

### Problem 4: Need to Revert to GRUB Completely
If at any point you wish to return to standard GRUB:
```bash
# View existing entries
efibootmgr

# Set GRUB (usually labeled 'Fedora') as top boot order
sudo efibootmgr -o <grub_entry_num>,...

# Remove RustyUKI kernel hook
sudo rm -f /usr/lib/kernel/install.d/90-rustyuki.install
sudo systemctl disable rustyuki-boot-confirm.service
```
Your original GRUB installation remains completely intact and functional.
