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

%changelog
* Fri Mar 13 2026 RustyUKI CI <ci@example.com> - 0.2.0-1
- Comprehensive rehabilitation and refactoring:
- Separated UKI generation, installation, and activation into distinct stages
- Added dual backends: Direct UEFI (default) and systemd-boot
- Added multi-stage system health assessment before boot confirmation
- Added fallback slot (.prev.efi) and sidecar provenance manifests
- Added support for CachyOS and Fedora kernels
- Added offline image building (--sysroot) without host state modification
