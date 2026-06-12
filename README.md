<div align="center">

# RustyUKI

**A Rust-native CLI for building and installing Unified Kernel Images on Fedora-based systems.**

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](./LICENSE)
[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org/)
[![Platform](https://img.shields.io/badge/Platform-Fedora%20%7C%20Nobara-lightblue?logo=fedora)](https://fedoraproject.org/)
[![Status](https://img.shields.io/badge/Status-Pre--release-red)]()
[![UEFI Only](https://img.shields.io/badge/Firmware-UEFI%20Only-purple)]()

</div>

---

> [!CAUTION]
> **RustyUKI modifies your EFI System Partition and boot entries. A misconfigured or failed build can leave your system completely unbootable.**
>
> - This is **pre-release software** under active development — not yet production-ready.
> - You **must** prepare a recovery method before running any command. See [§ Backups & Recovery](#%EF%B8%8F-backups--recovery).
> - **Never** run this on a machine you cannot physically access or recover via live USB.
> - Always preview changes with `--dry-run` before executing.

---

## Table of Contents

- [What is RustyUKI?](#what-is-rustyuki)
- [Requirements](#requirements)
- [Installation](#installation)
- [Configuration](#configuration)
- [Usage](#usage)
- [Safer First Boot Workflow](#safer-first-boot-workflow)
- [Example Workflow](#example-workflow)
- [⚠️ Backups & Recovery](#%EF%B8%8F-backups--recovery)
- [⚠️ Warnings & Known Failure Modes](#%EF%B8%8F-warnings--known-failure-modes)
- [Pre-flight Checklist](#pre-flight-checklist)
- [Project Structure](#project-structure)
- [Inspiration & Credits](#inspiration--credits)
- [Roadmap](#roadmap)
- [Contributing](#contributing)
- [License](#license)

---

## What is RustyUKI?

RustyUKI builds and installs [Unified Kernel Images (UKIs)](https://uapi-group.org/specifications/specs/unified_kernel_image/) using a two-stage pipeline:

1. **`dracut`** — generates the initramfs
2. **`ukify`** — assembles the final PE/EFI binary

The resulting UKI is a single self-contained EFI executable that bundles your kernel, initramfs, and kernel command line — allowing the UEFI firmware to boot directly into Linux, bypassing GRUB entirely.

**Benefits over GRUB:**

| Feature | GRUB | UKI (RustyUKI) |
|---|---|---|
| Boot chain stages | 2+ (GRUB → kernel) | 1 (UEFI → kernel) |
| Secure Boot | Complex, per-binary | Single signed blob |
| Boot management | `grub-mkconfig` | `bootctl` / `efibootmgr` |
| Reproducibility | Config-dependent | Atomic, versioned artifacts |

Also.........Huge theoretical boot speed improvements.

---

## Requirements

> [!IMPORTANT]
> RustyUKI is **UEFI-only**. It will not work on systems with legacy BIOS firmware. Do not attempt to run it on a BIOS system.

- Fedora Linux (or a Fedora-based distro — Nobara support in progress, see [Roadmap](#roadmap))
- `dracut` installed and functional
- `ukify` (`systemd-ukify`) available on `$PATH`
- UEFI firmware with a mounted EFI System Partition (ESP)
- Rust toolchain (`cargo`) to build from source

Verify your dependencies before proceeding:

```bash
which dracut && dracut --version
which ukify && ukify --version
mount | grep -i efi
```

---

## Installation

### Build from source

```bash
git clone https://github.com/GlitchSlayed/RustyUKI.git
cd RustyUKI
cargo build --release
sudo cp target/release/rustyuki /usr/local/bin/
```

### Verify

```bash
rustyuki --version
# rustyuki 0.2.0
```

### Activate automatic UKI rebuilds on kernel updates
```bash
sudo rustyuki install-hook
```

This installs a `kernel-install` plugin at `/usr/lib/kernel/install.d/90-rustyuki.install`. After this, every `dnf update kernel` or kernel package transaction will automatically rebuild or prune your UKIs without any manual intervention.

> [!NOTE]
> Pre-built binaries and RPM packages are planned for a future release. See [Roadmap](#roadmap).

---

## Configuration

RustyUKI reads from a TOML config file at `/etc/uki/uki.conf` by default. Override the path with `--config <PATH>`.

```bash
sudo mkdir -p /etc/uki
sudo nano /etc/uki/uki.conf
```

### Full config reference

```toml
[uki]
# Kernel version to build for. Defaults to `uname -r` if left empty.
kernel_version = ""

# Path to your EFI System Partition mountpoint
esp_path = "/boot/efi"

# Directory where the final .efi UKI will be written
output_dir = "/boot/efi/EFI/Linux"

# File containing kernel command line parameters — must not be empty
cmdline_file = "/etc/kernel/cmdline"

# Fallback cmdline when auto detection cannot find a usable value
configured_cmdline = "root=UUID=REPLACE-ME rw quiet rhgb"

# Enable cmdline auto-detection from /proc/cmdline, then cmdline_file
auto_detect_cmdline = true

# Metadata directory for detected cmdline state
cmdline_state_dir = "/var/lib/uki-build"

# Warn if cmdline has fewer tokens than this
cmdline_min_tokens = 3

# Optional: path to a splash/logo image to embed
splash = ""

# OS release metadata file
os_release = "/etc/os-release"

[dracut]
# Additional arguments passed directly to dracut
extra_args = []

[ukify]
# Additional arguments passed directly to ukify
extra_args = []
```

> [!TIP]
> CLI flags always override config file values. Use `sudo rustyuki status` to inspect the resolved effective configuration before building.

---

## Usage

```
rustyuki [OPTIONS] <COMMAND>
```

### Global options

| Flag | Default | Description |
|------|---------|-------------|
| `--config <PATH>` | `/etc/uki/uki.conf` | Path to config file |
| `--dry-run` | — | Print commands without executing |
| `-v` | — | Enable DEBUG logging |
| `-vv` | — | Enable TRACE logging |

### Subcommands

#### `status` — Inspect resolved runtime configuration

Prints the effective config after merging the config file and any CLI overrides. **Run this first before every build.**

```bash
sudo rustyuki status
```

#### `generate` — Build a UKI and register an EFI boot entry

Runs the full two-stage pipeline:

1. Invokes `dracut` to produce an initramfs
2. Invokes `ukify` to assemble the final `.efi` PE binary
3. Registers an EFI boot entry via `efibootmgr`

Output is written atomically (temp file → rename) to prevent partial writes from corrupting an existing working image.

```bash
sudo rustyuki generate
sudo rustyuki generate --kernel-version "$(uname -r)"
sudo rustyuki generate --boot-once
sudo rustyuki generate --dry-run   # preview only
```

Use `--boot-once` when you want firmware to trial the new UKI exactly once via `efibootmgr --bootnext` before you make it permanent with `rustyuki confirm`.

#### `install` — Generate and sync the ESP

`install` now always uses a trial-boot flow:

1. Generates and registers the UKI entry without rewriting permanent `BootOrder`
2. Sets the new entry as `BootNext` (one-time next boot)
3. Installs/enables `rustyuki-boot-confirm.service`
4. Runs `bootctl update` to synchronise the ESP

```bash
sudo rustyuki install
sudo rustyuki install --kernel-version "6.12.0-200.fc41.x86_64"
```

Use `generate --boot-once` if you want one-time trial semantics without running `bootctl update`.

#### `reconcile` — Rebuild all installed kernel UKIs and prune stale artifacts

Rebuilds UKIs for every kernel reported by `rpm -q kernel`, prunes stale `linux-*.efi` entries in the output directory, and runs `bootctl update`.

```bash
sudo rustyuki reconcile
```

#### `confirm` — Make a successful trial boot permanent

After booting the one-time UKI successfully, `rustyuki-boot-confirm.service` runs automatically early in userspace and calls `rustyuki confirm`. `confirm` reads `BootCurrent`, moves that entry to the front of `BootOrder`, and clears `BootNext`.

```bash
sudo rustyuki confirm
```

#### `install-hook` — Run reconcile automatically on kernel updates

Installs a `kernel-install` plugin so `rustyuki reconcile` runs on kernel add/remove events.

```bash
sudo rustyuki install-hook
```

---

## Safer First Boot Workflow

For first-time GRUB replacement or any risky UKI change, prefer a one-time boot trial instead of immediately changing your permanent firmware boot order.

```bash
# Build the UKI, refresh the ESP, and schedule it for the next boot only
sudo rustyuki install

# Reboot normally; firmware will use BootNext exactly once
sudo reboot

# Optional manual confirmation (normally done automatically by systemd service)
sudo rustyuki confirm
```

This workflow keeps your existing default entry in `BootOrder` as the fallback if the trial boot fails, while still letting you verify the UKI end-to-end before committing to it.

## Example Workflow

> [!WARNING]
> Do not skip steps 1 and 2. Running `install` without first verifying your config and doing a dry run is the most common cause of broken boot entries.

```bash
# Step 1 — inspect resolved config
sudo rustyuki status

# Step 2 — dry run: see every command that will execute, without running anything
sudo rustyuki generate --dry-run

# Step 3 — build and schedule a one-time trial boot
sudo rustyuki install

# Step 4 — reboot normally and let BootNext test the UKI once
sudo reboot

# Step 5 — optional manual confirmation (auto-confirm service handles this on success)
sudo rustyuki confirm
```

After booting into the UKI, verify it was used:

```bash
bootctl status
cat /proc/cmdline   # should match /etc/kernel/cmdline exactly
```

---

## ⚠️ Backups & Recovery

> [!CAUTION]
> **Do not skip this section.** Bootloader misconfiguration is one of the few failure modes that can leave a Linux system completely unreachable without physical intervention. Spend 10 minutes preparing before you touch anything.

### Step 1 — Prepare a live USB

> [!IMPORTANT]
> Have a **Fedora live USB** ready and **verified bootable** before running RustyUKI for the first time. If a broken UKI is installed and your system refuses to boot, a live USB is your only recovery path short of reinstalling.

Download: https://fedoraproject.org/workstation/download

Boot the USB now and confirm it reaches the desktop **before** you need it in an emergency.

---

### Step 2 — Back up your EFI System Partition

```bash
# Identify your ESP partition
lsblk -o NAME,PARTTYPE,MOUNTPOINT | grep -i efi

# Archive the entire ESP
sudo tar -czvf ~/esp-backup-$(date +%Y%m%d).tar.gz /boot/efi/
```

> [!CAUTION]
> A backup stored only on the machine you are about to modify is not a backup. **Copy it to a USB drive or external storage before proceeding.**

---

### Step 3 — Back up your EFI boot entries

```bash
# Dump all current boot entries
efibootmgr -v > ~/efibootmgr-backup-$(date +%Y%m%d).txt

# Confirm the dump looks correct
cat ~/efibootmgr-backup-$(date +%Y%m%d).txt
```

---

### Step 4 — Back up and verify your kernel cmdline

> [!WARNING]
> RustyUKI **bakes the kernel command line into the UKI at build time**. A wrong or empty `cmdline_file` will produce a UKI that builds without errors but **fails to boot** — most commonly dropping to an emergency shell because `root=` is missing or points to a wrong UUID.

```bash
# Back up the cmdline file
cp /etc/kernel/cmdline ~/cmdline-backup-$(date +%Y%m%d).txt

# Diff against what your system actually booted with
diff <(cat /etc/kernel/cmdline) <(cat /proc/cmdline)
```

If the diff is non-empty, resolve the discrepancy before building. Verify your `root=UUID=...` value:

```bash
# Find the correct UUID for your root partition
findmnt -n -o UUID /

# Cross-check with blkid
blkid | grep -E 'TYPE="(ext4|btrfs|xfs)"'
```

---

### Step 5 — Recovery procedure from a live USB

<details>
<summary><strong>▶ Click to expand: How to recover from a broken boot</strong></summary>

Boot your Fedora live USB, open a terminal, and run:

```bash
# 1. Identify your ESP partition
lsblk -o NAME,FSTYPE,PARTTYPE,SIZE,MOUNTPOINT

# 2. Mount the ESP
sudo mkdir -p /mnt/efi
sudo mount /dev/sdXY /mnt/efi   # replace sdXY with your actual ESP partition

# Option A — Restore ESP from backup (recommended)
sudo tar -xzvf /path/to/esp-backup-YYYYMMDD.tar.gz -C /

# Option B — Manually re-register a GRUB boot entry
sudo efibootmgr --create \
  --disk /dev/sdX \
  --part Y \
  --label "Fedora" \
  --loader "\EFI\fedora\grubx64.efi"

# 3. Verify entries were restored
efibootmgr -v

# 4. Remove a bad UKI entry if needed
efibootmgr -b XXXX -B   # replace XXXX with the entry number to delete
```

</details>

---

### Step 6 — Do not remove GRUB prematurely

> [!WARNING]
> **Keep GRUB intact until your UKI has booted successfully at least once.** RustyUKI registers a new EFI boot entry alongside your existing entries — it does not remove GRUB. Use your firmware's one-time boot menu to select the UKI entry for testing without changing the default boot order.

---

## ⚠️ Warnings & Known Failure Modes

### Things that will prevent booting

> [!CAUTION]

| Situation | Effect |
|-----------|--------|
| Wrong `root=` UUID in cmdline | UKI boots, cannot find root filesystem → drops to emergency shell |
| Empty or missing `cmdline_file` | UKI built with no kernel parameters → almost certainly won't boot |
| ESP not mounted or read-only | Build fails mid-pipeline or writes to wrong path |
| GRUB removed before UKI verified | No fallback if UKI doesn't boot |
| Legacy BIOS firmware | Not supported — do not attempt |
| Target kernel not installed | dracut fails or produces invalid initramfs |
| Wrong kernel version targeted | Initramfs / kernel mismatch → kernel panic at boot |

### Silent failures — build succeeds but system won't boot

> [!WARNING]

- **`/etc/kernel/cmdline` exists but is empty** — dracut and ukify succeed, UKI boots with no parameters. Always verify with `cat /etc/kernel/cmdline`.
- **`ukify` not on `$PATH`** — dracut stage succeeds, second stage crashes. ESP is unchanged but temp artifacts may linger in `/tmp`.
- **Secure Boot enabled without a signed UKI** — firmware silently refuses to load the binary. Disable Secure Boot or wait for signing support (see [Roadmap](#roadmap)).
- **Multiple kernels installed, wrong version targeted** — always verify `sudo rustyuki status` shows the intended kernel version.
- **Insufficient ESP free space** — UKIs are typically 50–150 MB. A failed write may leave a zero-byte or corrupt `.efi` file in the output directory.

---

## Pre-flight Checklist

> [!TIP]
> Run this before every `generate` or `install`. Save it as a local script if you use RustyUKI frequently.

```bash
#!/usr/bin/env bash
echo "=== RustyUKI Pre-flight Check ==="

echo -n "[1] ESP writable ......... "
touch /boot/efi/.rustyuki-write-test 2>/dev/null \
  && rm /boot/efi/.rustyuki-write-test \
  && echo "✓ OK" \
  || echo "✗ FAIL — mount your ESP first"

echo -n "[2] cmdline populated .... "
[[ -s /etc/kernel/cmdline ]] \
  && echo "✓ OK  ($(cat /etc/kernel/cmdline))" \
  || echo "✗ FAIL — /etc/kernel/cmdline is empty or missing"

echo -n "[3] dracut on PATH ....... "
command -v dracut &>/dev/null \
  && echo "✓ OK  ($(dracut --version 2>&1 | head -1))" \
  || echo "✗ FAIL — install dracut"

echo -n "[4] ukify on PATH ........ "
command -v ukify &>/dev/null \
  && echo "✓ OK" \
  || echo "✗ FAIL — install systemd-ukify"

echo    "[5] Running kernel ....... $(uname -r)"

echo -n "[6] ESP free space ....... "
df -h /boot/efi | awk 'NR==2 {print $4 " available on " $6}'

echo "==================================="
echo "Run 'sudo rustyuki status' to verify target kernel version."
```

---

## Project Structure

```
RustyUKI/
├── src/                  # Rust source
├── tests/                # Integration tests
├── .github/
│   └── workflows/        # CI pipelines
├── Cargo.toml
└── README.md
```

> [!NOTE]
> Legacy shell-script implementation has been removed; all supported behavior now lives in the Rust CLI.

---

## Inspiration & Credits

RustyUKI is its own Rust-native implementation, but the project is heavily informed by existing Fedora and Secure Boot tooling. Credit where it is due:

- **[kraxel/fedora-uki](https://github.com/kraxel/fedora-uki)** — strong reference point for Fedora-oriented UKI workflows, `kernel-install` integration, tentative boot entry handling, and broader production hardening ideas.
- **[rhboot/nmbl-builder](https://github.com/rhboot/nmbl-builder)** — inspiration for EFI boot entry management, Secure Boot signing flows, rollback-minded installation patterns, and practical builder ergonomics.
- **[dracut](https://github.com/dracutdevs/dracut)** — RustyUKI relies on `dracut` to generate initramfs images.
- **[systemd ukify / bootctl](https://github.com/systemd/systemd)** — `ukify` assembles the final UKI and `bootctl` is used for ESP synchronization where appropriate.
- **[`efibootmgr`](https://github.com/rhboot/efibootmgr)** — used for EFI boot entry creation and inspection.

These projects and tools shaped both the current implementation and the roadmap below.

---

## Roadmap

> [!NOTE]
> The roadmap below incorporates ideas derived from comparing RustyUKI against **kraxel/fedora-uki** and **rhboot/nmbl-builder**, alongside features already planned for RustyUKI itself. Shipped items were moved out so this section only tracks work that is still open.

### Recently shipped

- **BootNext trial boot workflow** — `--boot-once` now schedules a one-time trial boot and `rustyuki confirm` promotes the successful entry afterward.
- **ESP preflight validation** — generation now checks mount presence, mount state, free space, and output directory writability before writing a UKI.
- **Fedora `kernel-install` hook support** — RustyUKI can install a plugin so kernel add/remove events now trigger targeted UKI generation on adds and reconciliation on removals automatically.
- **Multi-kernel reconciliation and stale artifact cleanup** — `rustyuki reconcile` rebuilds installed kernels and prunes stale `linux-*.efi` outputs.
- **RPM workflow consolidation** — Fedora RPM CI and scheduled release automation now live in the single `rpm.yml` workflow.

### 1. Safety & Boot Entry Management

- [ ] **Secure Boot cmdline guard** — detect active Secure Boot, warn when cmdline changes require a rebuild/re-sign, and track cmdline hashes beside installed UKIs.

### 2. Fedora Integration

- [ ] **Boot environment awareness in `status`** — surface EFI boot entry details via `kernel-bootcfg --show` when available, with `efibootmgr -v` as a fallback.
- [ ] **GPT autodiscovery advisory** — detect discoverable root partitions and warn when `root=` is redundant or stale.
- [ ] **Supported architectures** — keep short-term guards in place for x86_64-only assumptions today, while planning full aarch64 support later.
- [ ] **Nobara Linux support** — detect Nobara-specific paths and dracut quirks; full pipeline testing.

### 3. Signing & Secure Boot

- [ ] **Secure Boot signing** — integrate `pesign` / `sbsign` into the build pipeline with optional configuration.
- [ ] **MOK provisioning helper** — add a guided `rustyuki enroll-mok` flow wrapping `efikeygen`, `certutil`, and `mokutil`.
- [ ] **TPM2 + measured boot** — integrate with `systemd-pcrphase` and PCR-sealed secret workflows.
- [ ] **UKI signing service** — explore a local signing daemon with audit logging for higher-assurance setups.

### 4. UKI Output & Build Pipeline

- [ ] **Atomic write with rollback slot** — preserve the previous UKI as a `.prev` image and add a `rustyuki rollback` command for recovery.
- [ ] **Multi-profile UKI support** — allow multiple named build profiles from one config for default, recovery, cloud, or hardware-specific UKIs.
- [ ] **Rootfs validation and fix-up** — cross-check `root=` against the running system in `status` and add a `rustyuki fix-cmdline` helper.
- [ ] **Fallback UKI pinning** — protect a designated "last known good" UKI from accidental replacement.
- [ ] **Automated pre-flight validation** — continue expanding built-in safety checks before every build.

### 5. CI, Packaging & Releases

- [ ] **Packaged releases** — publish pre-built binaries via GitHub Releases with SHA256 checksums.
- [ ] **RPM spec packaging** — complete `dnf install rustyuki` support for Fedora-based systems.
- [ ] **Clippy in CI** — add `cargo clippy --all-targets -- -D warnings` as a required check.
- [ ] **Pinned Fedora container digests** — pin workflow container images for better reproducibility.

### 6. UX & Longer-term Ideas

- [ ] **Broader distro support** — explore Arch Linux (with dracut), openSUSE Tumbleweed, and potentially Debian/Ubuntu-family systems via additional backends.
- [ ] **TUI dashboard** — interactive terminal UI for boot entries, ESP contents, and build history.
- [ ] **GUI frontend** — desktop application (GTK or Tauri) for non-CLI users.

---

## Contributing

Contributions, bug reports, and feature requests are welcome. Please [open an issue](https://github.com/GlitchSlayed/RustyUKI/issues) before submitting a large PR to align on approach first.

```bash
# Run tests
cargo test

# Check formatting
cargo fmt --check

# Lint
cargo clippy -- -D warnings
```

---

## License

MIT — see [LICENSE](./LICENSE) for details.
