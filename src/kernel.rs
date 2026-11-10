pub use crate::cmdline::{resolve_cmdline, sanitize_cmdline, CmdlineSettings};

use crate::cmd::CommandRunner;
use crate::model::{KernelFlavor, KernelInfo};
use anyhow::Result;
use std::fs;
use std::path::{Path, PathBuf};

pub fn prune_stale_uki_artifacts(
    output_dir: &Path,
    installed_kernels: &[String],
) -> Result<Vec<PathBuf>> {
    crate::install::prune_stale_ukis(output_dir, installed_kernels, None)
}

/// Modular kernel discovery engine.
///
/// Discovers installed kernels by inspecting module trees under `/lib/modules`
/// and `/usr/lib/modules` (honoring any target sysroot), cross-referencing with
/// RPM package queries when available.
pub fn discover_installed_kernels(
    runner: &dyn CommandRunner,
    sysroot: Option<&Path>,
) -> Result<Vec<KernelInfo>> {
    let mut candidates = Vec::new();

    let root = sysroot.unwrap_or_else(|| Path::new("/"));
    let mut module_roots = Vec::new();
    if let Ok(custom) = std::env::var("RUSTYUKI_MODULES_DIR") {
        for path in std::env::split_paths(&custom) {
            if path.is_dir() {
                module_roots.push(path);
            }
        }
    }
    module_roots.push(root.join("usr/lib/modules"));
    module_roots.push(root.join("lib/modules"));

    for module_root in &module_roots {
        if !module_root.is_dir() {
            continue;
        }

        let entries = match fs::read_dir(module_root) {
            Ok(entries) => entries,
            Err(_) => continue,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let version = match entry.file_name().into_string() {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Check for kernel image: vmlinuz or vmlinux
            let vmlinuz = path.join("vmlinuz");
            let vmlinux = path.join("vmlinux");

            let kernel_binary = if vmlinuz.is_file() {
                vmlinuz
            } else if vmlinux.is_file() {
                vmlinux
            } else {
                continue;
            };

            if candidates.iter().any(|k: &KernelInfo| k.version == version) {
                continue;
            }

            let flavor = detect_flavor(&version, None);

            candidates.push(KernelInfo {
                version,
                flavor,
                vmlinuz_path: kernel_binary,
                modules_dir: path,
                package_name: None,
            });
        }
    }

    // If running in live host without sysroot, enrich with RPM metadata where available
    if sysroot.is_none() {
        enrich_with_rpm_metadata(runner, &mut candidates);
    }

    // Sort versions stably (lexicographic reverse or version sort)
    candidates.sort_by(|a, b| b.version.cmp(&a.version));
    Ok(candidates)
}

/// Detects kernel flavor from version string or package name.
pub fn detect_flavor(version: &str, package: Option<&str>) -> KernelFlavor {
    let lower_v = version.to_ascii_lowercase();
    let lower_p = package.map(str::to_ascii_lowercase).unwrap_or_default();

    if lower_v.contains("cachyos") || lower_p.contains("cachyos") {
        KernelFlavor::CachyOs
    } else if lower_v.contains(".fc") || lower_p.starts_with("kernel-") {
        KernelFlavor::Fedora
    } else {
        KernelFlavor::Generic
    }
}

fn enrich_with_rpm_metadata(runner: &dyn CommandRunner, kernels: &mut [KernelInfo]) {
    // Query rpm for all kernel-related packages
    let output = match runner.run(
        "rpm",
        &[
            "-qa",
            "--qf",
            "%{NAME} %{VERSION}-%{RELEASE}.%{ARCH}\n",
            "kernel*",
        ],
    ) {
        Ok(out) => out.stdout,
        Err(_) => return,
    };

    for line in output.lines() {
        let mut parts = line.split_whitespace();
        let Some(pkg_name) = parts.next() else {
            continue;
        };
        let Some(pkg_evr) = parts.next() else {
            continue;
        };

        for kernel in kernels.iter_mut() {
            if kernel.version.contains(pkg_evr) || pkg_evr.contains(&kernel.version) {
                kernel.package_name = Some(pkg_name.to_string());
                if kernel.flavor == KernelFlavor::Generic {
                    kernel.flavor = detect_flavor(&kernel.version, Some(pkg_name));
                }
            }
        }
    }
}

/// Backwards-compatible helper returning a list of installed kernel version strings.
pub fn list_installed_kernels(runner: &dyn CommandRunner) -> Result<Vec<String>> {
    let discovered = discover_installed_kernels(runner, None)?;
    let mut versions: Vec<String> = discovered.into_iter().map(|k| k.version).collect();
    versions.sort();
    versions.dedup();
    Ok(versions)
}

/// Resolves a single target kernel candidate.
pub fn resolve_target_kernel(
    runner: &dyn CommandRunner,
    explicit_version: Option<&str>,
    sysroot: Option<&Path>,
) -> Result<KernelInfo> {
    if let Some(target) = explicit_version {
        if let Ok(custom) = std::env::var("RUSTYUKI_MODULES_DIR") {
            for base in std::env::split_paths(&custom) {
                let custom_mod = base.join(target);
                if custom_mod.join("vmlinuz").is_file() {
                    return Ok(KernelInfo {
                        version: target.to_string(),
                        flavor: detect_flavor(target, None),
                        vmlinuz_path: custom_mod.join("vmlinuz"),
                        modules_dir: custom_mod,
                        package_name: None,
                    });
                }
            }
        }

        let root = sysroot.unwrap_or_else(|| Path::new("/"));
        let usr_mod = root.join("usr/lib/modules").join(target);
        let lib_mod = root.join("lib/modules").join(target);

        let chosen_mod = if lib_mod.join("vmlinuz").is_file() || lib_mod.join("vmlinux").is_file() {
            lib_mod
        } else if usr_mod.join("vmlinuz").is_file() || usr_mod.join("vmlinux").is_file() {
            usr_mod
        } else if lib_mod.is_dir() {
            lib_mod
        } else {
            usr_mod
        };

        let vmlinuz = if chosen_mod.join("vmlinuz").is_file() {
            chosen_mod.join("vmlinuz")
        } else if chosen_mod.join("vmlinux").is_file() {
            chosen_mod.join("vmlinux")
        } else if root
            .join("boot")
            .join(format!("vmlinuz-{}", target))
            .is_file()
        {
            root.join("boot").join(format!("vmlinuz-{}", target))
        } else {
            chosen_mod.join("vmlinuz")
        };

        let flavor = detect_flavor(target, None);
        return Ok(KernelInfo {
            version: target.to_string(),
            flavor,
            vmlinuz_path: vmlinuz,
            modules_dir: chosen_mod,
            package_name: None,
        });
    }

    let kernels = discover_installed_kernels(runner, sysroot)?;

    if let Some(first) = kernels.into_iter().next() {
        return Ok(first);
    }

    // Default to uname -r on live host
    let uname = runner
        .run("uname", &["-r"])
        .map(|out| out.stdout.trim().to_string())
        .unwrap_or_else(|_| "unknown".to_string());

    let root = sysroot.unwrap_or_else(|| Path::new("/"));
    let mod_dir = root.join("lib/modules").join(&uname);
    let vmlinuz = mod_dir.join("vmlinuz");
    let flavor = detect_flavor(&uname, None);

    Ok(KernelInfo {
        version: uname,
        flavor,
        vmlinuz_path: vmlinuz,
        modules_dir: mod_dir,
        package_name: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{detect_flavor, KernelFlavor};

    #[test]
    fn flavor_detection_identifies_cachyos() {
        assert_eq!(
            detect_flavor("6.17.4-1.cachyos.fc44.x86_64", None),
            KernelFlavor::CachyOs
        );
        assert_eq!(
            detect_flavor("6.17.4-1.fc44.x86_64", Some("kernel-cachyos-lto")),
            KernelFlavor::CachyOs
        );
    }

    #[test]
    fn flavor_detection_identifies_fedora() {
        assert_eq!(
            detect_flavor("6.17.4-100.fc44.x86_64", None),
            KernelFlavor::Fedora
        );
        assert_eq!(
            detect_flavor("6.17.4-200.fc45.x86_64", Some("kernel-core")),
            KernelFlavor::Fedora
        );
    }

    #[test]
    fn flavor_detection_identifies_generic() {
        assert_eq!(detect_flavor("6.17.0-custom", None), KernelFlavor::Generic);
    }
}
