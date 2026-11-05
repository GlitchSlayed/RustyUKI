# Packaging & Continuous Integration

This guide documents the RPM packaging specification, CI validation pipelines, and automated nightly release workflows for RustyUKI targeting **Fedora 44** and **Fedora 45**.

---

## RPM Specification

The RPM spec file is located at [`.github/rpm/rustyuki.spec`](../.github/rpm/rustyuki.spec):

```spec
Name:           rustyuki
Version:        0.2.0
Release:        1%{?dist}
Summary:        Reliable, distribution-independent Unified Kernel Image lifecycle manager

License:        MIT
URL:            https://github.com/GlitchSlayed/RustyUKI
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  systemd-rpm-macros

Requires:       dracut
Requires:       systemd-ukify
Requires:       efibootmgr
Requires:       systemd-boot
Requires:       systemd-udev
Requires:       binutils

Recommends:     sbsign
Recommends:     pesign

%{?systemd_requires}

%description
RustyUKI is a failure-resistant Unified Kernel Image (UKI) manager for Linux systems.
It provides distinct generation, installation, and activation stages, supporting both
Direct UEFI and systemd-boot backends, multi-stage health assessment, safe fallback slots,
CachyOS and Fedora kernel compatibility, and offline sysroot generation.

%prep
%autosetup -n %{name}-%{version}

%build
cargo build --release --locked

%install
# Binary
install -Dpm0755 target/release/rustyuki %{buildroot}%{_bindir}/rustyuki

# Configuration
install -Dpm0644 packaging/config.toml %{buildroot}%{_sysconfdir}/rustyuki/config.toml

# kernel-install hook
install -Dpm0755 packaging/90-rustyuki.install %{buildroot}%{_prefix}/lib/kernel/install.d/90-rustyuki.install

# Systemd boot confirmation unit
install -Dpm0644 packaging/rustyuki-boot-confirm.service %{buildroot}%{_unitdir}/rustyuki-boot-confirm.service

# State directory for failure tracking and cmdline hash cache
install -d -m 0755 %{buildroot}%{_sharedstatedir}/uki-build

%post
%systemd_post rustyuki-boot-confirm.service

%preun
%systemd_preun rustyuki-boot-confirm.service

%postun
%systemd_postun_with_restart rustyuki-boot-confirm.service

%files
%license LICENSE
%doc README.md
%{_bindir}/rustyuki
%dir %{_sysconfdir}/rustyuki
%config(noreplace) %{_sysconfdir}/rustyuki/config.toml
%{_prefix}/lib/kernel/install.d/90-rustyuki.install
%{_unitdir}/rustyuki-boot-confirm.service
%dir %{_sharedstatedir}/uki-build
```

### Local RPM Build

To build an RPM locally on a Fedora workstation:

```bash
# 1. Install packaging tools
sudo dnf install -y git rpm-build rpmdevtools cargo rust systemd-rpm-macros

# 2. Set up the RPM build tree
rpmdev-setuptree

# 3. Create the source archive
VERSION="0.2.0"
git archive --format=tar.gz --prefix="rustyuki-${VERSION}/" -o ~/rpmbuild/SOURCES/rustyuki-${VERSION}.tar.gz HEAD

# 4. Copy and build the spec
cp .github/rpm/rustyuki.spec ~/rpmbuild/SPECS/
rpmbuild -ba ~/rpmbuild/SPECS/rustyuki.spec

# 5. Resulting packages
ls -l ~/rpmbuild/RPMS/*/*.rpm
ls -l ~/rpmbuild/SRPMS/*.rpm
```

---

## Continuous Integration (`checks.yml`)

The repository runs continuous validation across a matrix of the latest two Fedora releases (**Fedora 44** and **Fedora 45**) on every push and pull request via [`.github/workflows/checks.yml`](../.github/workflows/checks.yml):

```mermaid
flowchart LR
    PushOrPR["Git Push / PR"] --> Matrix["Matrix Strategy:<br/>Fedora 44 & Fedora 45 Containers"]
    Matrix --> Fmt["cargo fmt --all --check"]
    Fmt --> Tests["cargo test --all-targets --locked"]
    Tests --> Clippy["cargo clippy --all-targets -- -D warnings"]
    Clippy --> Check["cargo check --all-targets --locked"]
    Check --> Build["cargo build --all-targets --locked"]
```

### Features Verified:
- **Matrix Testing**: Validates code and dependencies against Fedora 44 (`fedora:44`) and Fedora 45 (`fedora:45`).
- **Rustfmt Compliance**: Ensures consistent formatting across all modules and tests.
- **Unit and Integration Tests**: Runs tests in [`tests/integration_cli.rs`](../tests/integration_cli.rs) and [`tests/backend_and_safety.rs`](../tests/backend_and_safety.rs) using mocked system calls (`MockRunner`).
- **Strict Clippy**: Enforces `-D warnings` across all code.
- **Lockfile Integrity**: Enforces `--locked` flags when `Cargo.lock` is present.

---

## Automated RPM CI, Smoke Tests & Nightly Releases (`rpm.yml`)

The workflow in [`.github/workflows/rpm.yml`](../.github/workflows/rpm.yml) validates and builds the RPM package across Fedora 44 and Fedora 45:
1. **Pull Requests (`rpm-ci`)**: Automatically builds SRPMs and binary RPMs in clean Fedora 44 & 45 containers, followed by an immediate installation smoke test:
   - `dnf install -y dist/rustyuki-*.x86_64.rpm`
   - `rustyuki --version`
   - Verifies `/usr/lib/kernel/install.d/90-rustyuki.install`
   - Verifies `/etc/rustyuki/config.toml`
   - Verifies `/usr/lib/systemd/system/rustyuki-boot-confirm.service`
2. **Each Commit & Scheduled Releases (`plan-release` -> `build-release-rpms` -> `publish-release`)**:
   - On every commit push to `main` (as well as nightly schedule and manual dispatch), the pipeline builds binary RPMs and SRPMs targeting Fedora 44 and Fedora 45.
   - Runs full package installation and smoke tests.
   - Automatically publishes a GitHub Release tagged `rpm-nightly-YYYYMMDD-<short-sha>` with all built `.rpm` and `.src.rpm` artifacts attached.

---

## Real Fedora VM Boot Integration Testing (`fedora-boot-integration.yml`)

The workflow in [`.github/workflows/fedora-boot-integration.yml`](../.github/workflows/fedora-boot-integration.yml) executes full-lifecycle UKI boot and management tests on official Fedora installations using QEMU and OVMF UEFI firmware.

### Architecture & Caching Strategy
- **Base Image Pinned Manifest**: [`scripts/fedora_image_manifest.json`](../scripts/fedora_image_manifest.json) pins official Fedora 45 and Fedora 44 Cloud Base Generic image URLs and cryptographic SHA-256 digests.
- **Persistent Image Caching**: GitHub Actions restores the verified base `.qcow2` images via `actions/cache@v4` with cache key `fedora-vm-cache-v1-${{ matrix.fedora_version }}-${{ hashFiles('scripts/fedora_image_manifest.json') }}`. Cache misses trigger automatic download and cryptographic verification.
- **Disposable Overlays**: Tests never mutate the cached base image. Each scenario executes inside a disposable qcow2 overlay with an isolated, disposable copy of `OVMF_VARS.fd`.
- **Virtualization Acceleration**: Automatically utilizes KVM (`/dev/kvm`) on GitHub-hosted `ubuntu-24.04` runners, falling back to TCG emulation when KVM is unavailable.

### Scenarios Covered
| Scenario | Backend | Description |
|---|---|---|
| **Scenario A** | `systemd-boot` | Fresh conversion from conventional Fedora GRUB to systemd-boot Type #2 UKI, firmware reboot, kernel assertion, and health verification (Mandatory release gate). |
| **Scenario B** | `direct` | Registration and boot through Direct UEFI firmware entry via `efibootmgr`, trial boot assessment, and boot confirmation. |
| **Scenario C** | Lifecycle | Multiple kernel versions, automated UKI generation, reconciliation, and safe pruning retaining active and fallback slots. |
| **Scenario D** | Recovery | Rollback recovery preserving `.prev.efi` fallback slot intact for repeatable recovery operations. |
| **Negative Verification** | Safety Gate | Deliberately simulates guest assertion failures to verify the harness detects errors and never emits false passes. |
