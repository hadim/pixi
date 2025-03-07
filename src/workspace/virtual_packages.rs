use std::collections::HashMap;

use itertools::Itertools;
use miette::Diagnostic;
use pixi_default_versions::{
    default_glibc_version, default_linux_version, default_mac_os_version, default_windows_version,
};
use pixi_manifest::{FeaturesExt, LibCSystemRequirement, SystemRequirements};
use rattler_conda_types::{GenericVirtualPackage, Platform, Version};
use rattler_virtual_packages::{
    Archspec, Cuda, DetectVirtualPackageError, LibC, Linux, Osx, VirtualPackage,
    VirtualPackageOverrides,
};
use thiserror::Error;

use crate::workspace::{errors::UnsupportedPlatformError, Environment};

/// Returns a reasonable modern set of virtual packages that should be safe
/// enough to assume. At the time of writing, this is in sync with the
/// conda-lock set of minimal virtual packages. <https://github.com/conda/conda-lock/blob/3d36688278ebf4f65281de0846701d61d6017ed2/conda_lock/virtual_package.py#L175>
///
/// The method also takes into account system requirements specified in the
/// project manifest.
pub(crate) fn get_minimal_virtual_packages(
    platform: Platform,
    system_requirements: &SystemRequirements,
) -> Vec<VirtualPackage> {
    // TODO: How to add a default cuda requirements
    let mut virtual_packages: Vec<VirtualPackage> = vec![];

    // Match high level platforms
    if platform.is_unix() {
        virtual_packages.push(VirtualPackage::Unix);
    }
    if platform.is_linux() {
        let version = system_requirements
            .linux
            .clone()
            .unwrap_or(default_linux_version());
        virtual_packages.push(VirtualPackage::Linux(Linux { version }));

        let (family, version) = system_requirements
            .libc
            .as_ref()
            .map(LibCSystemRequirement::family_and_version)
            .map(|(family, version)| (family.to_string(), version.clone()))
            .unwrap_or(("glibc".parse().unwrap(), default_glibc_version()));
        virtual_packages.push(VirtualPackage::LibC(LibC { family, version }));
    }

    if platform.is_windows() {
        // todo: add windows to system requirements
        let version = Some(default_windows_version());
        virtual_packages.push(VirtualPackage::Win(rattler_virtual_packages::Windows {
            version,
        }));
    }

    // Add platform specific packages
    if platform.is_osx() {
        let version = system_requirements
            .macos
            .clone()
            .unwrap_or_else(|| default_mac_os_version(platform));
        virtual_packages.push(VirtualPackage::Osx(Osx { version }));
    }

    // Cuda
    if let Some(version) = system_requirements.cuda.clone() {
        virtual_packages.push(VirtualPackage::Cuda(Cuda { version }));
    }

    // Archspec is only based on the platform for now
    if let Some(spec) = Archspec::from_platform(platform) {
        virtual_packages.push(VirtualPackage::Archspec(spec));
    }

    virtual_packages
}

impl Environment<'_> {
    /// Returns the set of virtual packages to use for the specified platform.
    /// This method takes into account the system requirements specified in
    /// the project manifest.
    pub(crate) fn virtual_packages(&self, platform: Platform) -> Vec<VirtualPackage> {
        get_minimal_virtual_packages(platform, &self.system_requirements())
    }
}

/// An error that occurs when the current platform does not satisfy the minimal
/// virtual package requirements.
#[derive(Debug, Error, Diagnostic)]
pub enum VerifyCurrentPlatformError {
    #[error("The current platform does not satisfy the minimal virtual package requirements")]
    UnsupportedPlatform(#[from] Box<UnsupportedPlatformError>),

    #[error(transparent)]
    DetectionVirtualPackagesError(#[from] DetectVirtualPackageError),

    #[error("The current system has a mismatching virtual package. The project requires '{required}' to be on build '{required_build_string}' but the system has build '{local_build_string}'")]
    MismatchingBuildString {
        required: String,
        required_build_string: String,
        local_build_string: String,
    },

    #[error("The current system has a mismatching virtual package. The project requires '{required}' to be at least version '{required_version}' but the system has version '{local_version}'")]
    MismatchingVersion {
        required: String,
        required_version: Box<Version>,
        local_version: Box<Version>,
    },

    #[error("The platform you are running on should at least have the virtual package {required} on version {required_version}, build_string: {required_build_string}")]
    MissingVirtualPackage {
        required: String,
        required_version: Box<Version>,
        required_build_string: String,
    },
}

/// Verifies if the current platform satisfies the minimal virtual package
/// requirements.
pub(crate) fn verify_current_platform_has_required_virtual_packages(
    environment: &Environment<'_>,
) -> Result<(), VerifyCurrentPlatformError> {
    let current_platform = environment.best_platform();

    // Is the current platform in the list of supported platforms?
    if !environment.platforms().contains(&current_platform) {
        return Err(VerifyCurrentPlatformError::from(Box::new(
            UnsupportedPlatformError {
                environments_platforms: environment.platforms().into_iter().collect_vec(),
                platform: current_platform,
                environment: environment.name().clone(),
            },
        )));
    }

    let system_virtual_packages = VirtualPackage::detect(&VirtualPackageOverrides::from_env())?
        .iter()
        .cloned()
        .map(GenericVirtualPackage::from)
        .map(|vpkg| (vpkg.name.clone(), vpkg))
        .collect::<HashMap<_, _>>();
    let required_pkgs = environment
        .virtual_packages(current_platform)
        .into_iter()
        .map(GenericVirtualPackage::from);

    // Check for every local minimum package if it is available and on the correct
    // version.
    for req_pkg in required_pkgs {
        if req_pkg.name.as_normalized() == "__archspec" {
            // Skip archspec packages completely for now.
            continue;
        }

        if let Some(local_vpkg) = system_virtual_packages.get(&req_pkg.name) {
            if req_pkg.build_string != local_vpkg.build_string {
                return Err(VerifyCurrentPlatformError::MismatchingBuildString {
                    required: req_pkg.name.as_source().to_string(),
                    required_build_string: req_pkg.build_string.clone(),
                    local_build_string: local_vpkg.build_string.clone(),
                });
            }

            if req_pkg.version > local_vpkg.version {
                // This case can simply happen because the default system requirements in
                // get_minimal_virtual_packages() is higher than required.
                return Err(VerifyCurrentPlatformError::MismatchingVersion {
                    required: req_pkg.name.as_source().to_string(),
                    required_version: Box::from(req_pkg.version),
                    local_version: Box::from(local_vpkg.version.clone()),
                });
            }
        } else {
            return Err(VerifyCurrentPlatformError::MissingVirtualPackage {
                required: req_pkg.name.as_source().to_string(),
                required_version: Box::from(req_pkg.version),
                required_build_string: req_pkg.build_string.clone(),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use insta::assert_debug_snapshot;
    use pixi_manifest::SystemRequirements;
    use rattler_conda_types::Platform;

    use super::*;

    // Regression test on the virtual packages so there is not accidental changes
    #[test]
    fn test_get_minimal_virtual_packages() {
        let platforms = vec![
            Platform::NoArch,
            Platform::Linux64,
            Platform::LinuxAarch64,
            Platform::LinuxPpc64le,
            Platform::Osx64,
            Platform::OsxArm64,
            Platform::Win64,
        ];

        let system_requirements = SystemRequirements::default();

        for platform in platforms {
            let packages = get_minimal_virtual_packages(platform, &system_requirements)
                .into_iter()
                .map(GenericVirtualPackage::from)
                .collect_vec();
            insta::with_settings!({snapshot_suffix => platform.as_str()}, {
                assert_debug_snapshot!(packages);
            });
        }
    }
}
