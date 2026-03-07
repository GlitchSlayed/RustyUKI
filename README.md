# Fedora UKI Setup Script

`uki-setup.sh` automates creation and lifecycle management of **Unified Kernel Images (UKIs)** on Fedora and other Linux systems that provide compatible `dracut` + `kernel-install` tooling.

It performs a one-time setup that:

- Installs required tooling (`dracut`, `efibootmgr`, `binutils`, `systemd-boot-unsigned`).
- Writes a UKI rebuild helper script to `/usr/local/sbin/uki-build.sh`.
- Installs a `kernel-install` plugin that rebuilds/removes UKIs when kernels are added/removed.
- Disables default Fedora kernel-install plugins that generate GRUB/BLS entries to avoid conflicts.
- Builds and registers a UKI for the currently running kernel.

---

## What this is for

A UKI bundles the kernel, initramfs, and kernel command line into one EFI executable. This can simplify boot flows and makes kernel updates predictable when combined with `kernel-install` hooks.

This project is intended for systems booting in **UEFI mode** with a mounted **EFI System Partition (ESP)** (typically `/boot/efi`).

---

## Requirements

- Linux distribution with `dracut`, `kernel-install`, and `efibootmgr` available.
- UEFI boot mode.
- Root privileges.
- ESP mounted at `/boot/efi` or `/efi`.

> [!DANGER]
> **Make a full system backup before running this script.** A tested restore path (snapshot rollback, rescue image, or offline backup) is strongly recommended.
>
> This script is **invasive and experimental**: it modifies bootloader behavior, writes kernel-install hooks, manages EFI entries, and disables default kernel-install plugins. A misconfiguration can leave your system unbootable.
>
> Only proceed if you understand your boot stack and are prepared to recover from a failed boot.

---

## Safety and backup-first workflow (recommended)

Before setup:

1. Create a full backup or snapshot of your current system.
2. Confirm you have working rescue media.
3. Record current EFI entries:

```bash
sudo efibootmgr -v
```

4. Confirm your root filesystem parameters (`root=UUID=...`, encryption/LVM args, etc.).
5. Keep at least one known-good boot entry in firmware boot order until you've validated the new UKI boots.

The setup script also creates local file backups under:

```bash
/var/backups/uki-setup/
```

for any existing files it overwrites.

---

## Quick start

1. Clone this repository.
2. Edit configuration values at the top of `uki-setup.sh` (especially `CMDLINE`).
3. Run:

```bash
sudo bash uki-setup.sh
```

After setup, future kernel install/remove operations should automatically update UKIs.

> [!NOTE]
> The script auto-detects `dnf`, `apt`, `zypper`, or `pacman` and attempts to install needed dependencies with the detected package manager.

---

## Configuration options

Configuration is in the **USER CONFIGURATION** section of `uki-setup.sh`.

### `EFI_DIR`
Directory on the ESP where UKIs are written.

Default:

```bash
EFI_DIR="/boot/efi/EFI/Linux"
```

### `CMDLINE`
Kernel command line embedded into the UKI.

Default placeholder:

```bash
CMDLINE="rw quiet rhgb"
```

You should typically include your root device details (`root=UUID=...`, and any encryption/LVM/btrfs args needed by your setup).

### `AUTO_DETECT_CMDLINE`
When set to `1`, command line is derived from `/proc/cmdline` (with boot-loader-specific items removed).

Default:

```bash
AUTO_DETECT_CMDLINE=0
```

### `EFI_STUB`
Optional explicit path to the EFI stub used by dracut.

Default:

```bash
EFI_STUB=""
```

Leave empty to auto-detect common Fedora paths.

---


## Continuous integration checks

GitHub Actions now runs checks in a Fedora container (`fedora:41`) on every push and pull request. The workflow verifies:

- Bash syntax for `uki-setup.sh` and test scripts.
- `shellcheck` linting.
- A project check script that sources `uki-setup.sh` with `UKI_SETUP_SKIP_MAIN=1` and validates the generated helper/plugin templates.

Run the same checks locally with:

```bash
bash -n uki-setup.sh tests/test_uki_setup.sh
shellcheck -P . uki-setup.sh tests/test_uki_setup.sh
bash tests/test_uki_setup.sh
```

---

## Files created by setup

Running `uki-setup.sh` creates/updates:

- `/usr/local/sbin/uki-build.sh` — manual/automated UKI rebuild helper.
- `/usr/lib/kernel/install.d/90-uki-dracut.install` — kernel-install plugin.
- `/etc/kernel/install.d/*.install -> /dev/null` overrides for selected default plugins.

---

## Manual operations

### Rebuild UKI for current kernel

```bash
sudo /usr/local/sbin/uki-build.sh "$(uname -r)"
```

### Rebuild UKI for a specific installed kernel

```bash
sudo /usr/local/sbin/uki-build.sh 6.11.4-200.fc40.x86_64
```

### Check EFI boot entries

```bash
efibootmgr -v
```

### List generated UKIs

```bash
ls -lh /boot/efi/EFI/Linux/*.efi
```

---

## Updating configuration after first run

After initial setup, edit settings in:

```bash
/usr/local/sbin/uki-build.sh
```

The setup script templates values into this file during installation.

---

## Rollback / uninstall (manual)

If you want to return to your previous boot flow, you can:

1. Remove custom plugin and helper script:

```bash
sudo rm -f /usr/lib/kernel/install.d/90-uki-dracut.install
sudo rm -f /usr/local/sbin/uki-build.sh
```

2. Remove override symlinks created by this setup:

```bash
sudo rm -f \
  /etc/kernel/install.d/20-grub.install \
  /etc/kernel/install.d/50-depmod.install \
  /etc/kernel/install.d/90-loaderentry.install \
  /etc/kernel/install.d/92-crashkernel.install \
  /etc/kernel/install.d/95-kernel-install.install
```

3. Optionally delete generated UKIs from ESP and remove corresponding EFI entries with `efibootmgr`.

---

## Troubleshooting

- **UEFI not detected**: Ensure firmware boot mode is UEFI and ESP is mounted.
- **UKI fails to boot**: Re-check `CMDLINE` and storage-related boot args.
- **Missing EFI stub**: Install your distro's systemd-boot package and verify stub path.
- **No Secure Boot signing**: Install `sbsigntools`; this script only warns when absent.

---

## License

MIT — see [LICENSE](LICENSE).
