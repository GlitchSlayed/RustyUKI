# RustyUKI

**RustyUKI** is a Rust rewrite of the original Fedora UKI setup script. It builds and installs Unified Kernel Images (UKIs) using a two-stage pipeline:

1. **dracut** for initramfs generation
2. **ukify** for final PE/EFI UKI assembly

This project is focused on Fedora-style systems with UEFI firmware and an EFI System Partition (ESP).

## What changed

This repository previously centered around `uki-setup.sh`. It now includes a Rust CLI with modular code and stronger error handling.

- Subcommand-driven CLI (`generate`, `install`, `status`)
- TOML config loading from `/etc/uki/uki.conf`
- Shared command runner with dry-run support and typed command failures
- Root privilege enforcement at startup
- Atomic UKI output write (temp file + rename)

## CLI

```bash
rustyuki [OPTIONS] <COMMAND>
```

Global options:

- `--config <PATH>` (default `/etc/uki/uki.conf`)
- `--dry-run`
- `-v` (DEBUG), `-vv` (TRACE)

Subcommands:

- `rustyuki generate` – generate a UKI and register EFI boot entry
- `rustyuki install` – generate and run `bootctl update`
- `rustyuki status` – print resolved runtime status

## Config file

Default path: `/etc/uki/uki.conf`

```toml
[uki]
kernel_version = ""         # defaults to uname -r if empty
esp_path = "/boot/efi"
output_dir = "/boot/efi/EFI/Linux"
cmdline_file = "/etc/kernel/cmdline"
splash = ""
os_release = "/etc/os-release"

[dracut]
extra_args = []

[ukify]
extra_args = []
```

CLI flags override config values for `generate` and `install`.

## Build

```bash
cargo build --release
```

Binary:

```bash
target/release/rustyuki
```

## Example usage

```bash
sudo target/release/rustyuki status
sudo target/release/rustyuki generate
sudo target/release/rustyuki install --kernel-version "$(uname -r)"
```

## Safety notes

- Root is required.
- Ensure your ESP is mounted and writable before generating UKIs.
- Keep a known-good boot entry until new UKIs are confirmed bootable.

## Legacy script

The original `uki-setup.sh` remains in the repository for reference/migration context.
