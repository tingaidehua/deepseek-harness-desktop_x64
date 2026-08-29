//! Profile plugin compatibility checks against one concrete DSH installation.
//!
//! The report is independent of the WebView and process lifecycle so startup,
//! core switching, diagnostics, and tests use the same decision.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use tauri::AppHandle;

use super::installed::{profile_dir, ProfilePackageJson};

const DSH_PACKAGE_PREFIX: &str = "@deepseek-ai/dsh-";
const LEGACY_CLIENT_RUNTIME: &str = "@deepseek-ai/dsh-client-runtime";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityIssue {
    pub code: String,
    pub package: String,
    pub dependency: String,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CompatibilityReport {
    pub compatible: bool,
    pub core_path: String,
    pub profile_path: String,
    pub checked_packages: Vec<String>,
    pub issues: Vec<CompatibilityIssue>,
}

#[derive(Debug, Default, Deserialize)]
struct PackageManifest {
    #[serde(default)]
    name: String,
    #[serde(default)]
    dependencies: BTreeMap<String, String>,
    #[serde(default)]
    dsh: PackageDsh,
}

#[derive(Debug, Default, Deserialize)]
struct PackageDsh {
    #[serde(default)]
    bundle: Option<serde_json::Value>,
    #[serde(default)]
    client: PackageClient,
}

#[derive(Debug, Default, Deserialize)]
struct PackageClient {
    #[serde(default)]
    inject: Vec<String>,
}

fn package_dir(node_modules: &Path, name: &str) -> PathBuf {
    node_modules.join(name)
}

fn package_exists(core_modules: &Path, profile_modules: &Path, name: &str) -> bool {
    package_dir(profile_modules, name)
        .join("package.json")
        .is_file()
        || package_dir(core_modules, name)
            .join("package.json")
            .is_file()
}

fn resolve_package_manifest(
    core_modules: &Path,
    profile_modules: &Path,
    name: &str,
) -> Option<(PathBuf, bool)> {
    let profile = package_dir(profile_modules, name).join("package.json");
    if profile.is_file() {
        return Some((profile, true));
    }
    let core = package_dir(core_modules, name).join("package.json");
    core.is_file().then_some((core, false))
}

/// Inspect a profile against the exact package inventory shipped by `core_dir`.
pub fn inspect(core_dir: &Path, profile_dir: &Path) -> Result<CompatibilityReport, String> {
    let profile_manifest_path = profile_dir.join("package.json");
    let raw = match std::fs::read_to_string(&profile_manifest_path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CompatibilityReport {
                compatible: true,
                core_path: core_dir.to_string_lossy().into_owned(),
                profile_path: profile_dir.to_string_lossy().into_owned(),
                checked_packages: Vec::new(),
                issues: Vec::new(),
            });
        }
        Err(error) => return Err(format!("PROFILE_MANIFEST_READ: {error}")),
    };
    let profile: ProfilePackageJson =
        serde_json::from_str(&raw).map_err(|error| format!("PROFILE_MANIFEST_PARSE: {error}"))?;
    let mut references: BTreeSet<String> = profile.dependencies.into_keys().collect();
    if let Some(profile) = profile.dsh.and_then(|dsh| dsh.profile) {
        references.extend(profile.bundles);
    }

    let core_modules = core_dir.join("node_modules");
    let profile_modules = profile_dir.join("node_modules");
    let legacy_core = package_exists(&core_modules, &profile_modules, LEGACY_CLIENT_RUNTIME);
    let mut checked_packages = Vec::new();
    let mut issues = Vec::new();

    for reference in references {
        let Some((manifest_path, profile_owned)) =
            resolve_package_manifest(&core_modules, &profile_modules, &reference)
        else {
            issues.push(CompatibilityIssue {
                code: "PLUGIN_PACKAGE_MISSING".into(),
                package: reference.clone(),
                dependency: reference,
                detail: "profile references a package that is absent from both the profile and the selected core".into(),
            });
            continue;
        };
        let raw = std::fs::read_to_string(&manifest_path).map_err(|error| {
            format!("PLUGIN_MANIFEST_READ: {}: {error}", manifest_path.display())
        })?;
        let manifest: PackageManifest = serde_json::from_str(&raw).map_err(|error| {
            format!(
                "PLUGIN_MANIFEST_PARSE: {}: {error}",
                manifest_path.display()
            )
        })?;
        let package = if manifest.name.is_empty() {
            reference.clone()
        } else {
            manifest.name
        };
        checked_packages.push(package.clone());

        if profile_owned && manifest.dsh.bundle.is_none() {
            issues.push(CompatibilityIssue {
                code: "PLUGIN_BUNDLE_MISSING".into(),
                package: package.clone(),
                dependency: reference.clone(),
                detail: format!(
                    "{package} is installed as a profile dependency but declares no dsh.bundle and therefore cannot activate"
                ),
            });
        }

        for injected in manifest.dsh.client.inject {
            if !package_exists(&core_modules, &profile_modules, &injected) {
                issues.push(CompatibilityIssue {
                    code: "PLUGIN_CLIENT_MODULE_MISSING".into(),
                    package: package.clone(),
                    dependency: injected.clone(),
                    detail: format!(
                        "{package} injects {injected}, but the selected core and profile do not provide it"
                    ),
                });
            }
        }
        for (dependency, range) in manifest.dependencies {
            if profile_owned && !legacy_core && dependency.starts_with(DSH_PACKAGE_PREFIX) {
                issues.push(CompatibilityIssue {
                    code: "PLUGIN_OWNS_CORE_DEPENDENCY".into(),
                    package: package.clone(),
                    dependency: dependency.clone(),
                    detail: format!(
                        "{package} declares {dependency}@{range} as a runtime dependency; DSH packages must come from the selected core"
                    ),
                });
            }
        }
    }

    checked_packages.sort();
    issues.sort_by(|left, right| {
        (&left.package, &left.code, &left.dependency).cmp(&(
            &right.package,
            &right.code,
            &right.dependency,
        ))
    });
    Ok(CompatibilityReport {
        compatible: issues.is_empty(),
        core_path: core_dir.to_string_lossy().into_owned(),
        profile_path: profile_dir.to_string_lossy().into_owned(),
        checked_packages,
        issues,
    })
}

pub fn require_compatible(core_dir: &Path, profile_dir: &Path) -> Result<(), String> {
    let report = inspect(core_dir, profile_dir)?;
    if report.compatible {
        return Ok(());
    }
    let detail = report
        .issues
        .iter()
        .map(|issue| format!("{}:{}->{}", issue.code, issue.package, issue.dependency))
        .collect::<Vec<_>>()
        .join(", ");
    Err(format!("PLUGIN_INCOMPATIBLE: {detail}"))
}

pub fn require_active_compatible(app_handle: &AppHandle) -> Result<(), String> {
    require_compatible(
        &crate::service::core::active_core_dir(app_handle),
        &profile_dir(app_handle),
    )
}

/// Validate one packaged plugin before installing it into a profile.
pub fn require_packaged_plugin_compatible(
    core_dir: &Path,
    plugin_dir: &Path,
) -> Result<(), String> {
    let path = plugin_dir.join("package.json");
    let raw = std::fs::read_to_string(&path)
        .map_err(|error| format!("PLUGIN_MANIFEST_READ: {}: {error}", path.display()))?;
    let manifest: PackageManifest = serde_json::from_str(&raw)
        .map_err(|error| format!("PLUGIN_MANIFEST_PARSE: {}: {error}", path.display()))?;
    let package = if manifest.name.is_empty() {
        plugin_dir.to_string_lossy().into_owned()
    } else {
        manifest.name
    };
    let core_modules = core_dir.join("node_modules");
    let legacy_core = package_dir(&core_modules, LEGACY_CLIENT_RUNTIME)
        .join("package.json")
        .is_file();
    let mut issues = Vec::new();
    if manifest.dsh.bundle.is_none() {
        issues.push(format!("PLUGIN_BUNDLE_MISSING:{package}"));
    }
    for injected in manifest.dsh.client.inject {
        if !package_dir(&core_modules, &injected)
            .join("package.json")
            .is_file()
        {
            issues.push(format!(
                "PLUGIN_CLIENT_MODULE_MISSING:{package}->{injected}"
            ));
        }
    }
    for dependency in manifest.dependencies.keys() {
        if !legacy_core && dependency.starts_with(DSH_PACKAGE_PREFIX) {
            issues.push(format!(
                "PLUGIN_OWNS_CORE_DEPENDENCY:{package}->{dependency}"
            ));
        }
    }
    if issues.is_empty() {
        Ok(())
    } else {
        Err(format!("PLUGIN_INCOMPATIBLE: {}", issues.join(", ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(label: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "dsh-plugin-compatibility-{}-{label}",
            std::process::id()
        ))
    }

    fn write(path: &Path, value: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, value).unwrap();
    }

    #[test]
    fn reports_removed_client_module_and_owned_core_dependency() {
        let root = root("incompatible");
        let core = root.join("core");
        let profile = root.join("profile");
        write(
            &profile.join("package.json"),
            r#"{"dependencies":{"plugin":"1.0.0"},"dsh":{"profile":{"bundles":["plugin"]}}}"#,
        );
        write(
            &profile.join("node_modules/plugin/package.json"),
            r#"{"name":"plugin","dependencies":{"@deepseek-ai/dsh-old":"^1"},"dsh":{"bundle":{"patch":"./patch.yml"},"client":{"inject":["@deepseek-ai/dsh-client-runtime"]}}}"#,
        );
        let report = inspect(&core, &profile).unwrap();
        assert!(!report.compatible);
        assert_eq!(report.issues.len(), 2);
        assert_eq!(report.issues[0].code, "PLUGIN_CLIENT_MODULE_MISSING");
        assert_eq!(report.issues[1].code, "PLUGIN_OWNS_CORE_DEPENDENCY");
    }

    #[test]
    fn accepts_injections_provided_by_selected_core() {
        let root = root("compatible");
        let core = root.join("core");
        let profile = root.join("profile");
        write(
            &profile.join("package.json"),
            r#"{"dependencies":{"plugin":"1.0.0"}}"#,
        );
        write(
            &profile.join("node_modules/plugin/package.json"),
            r#"{"name":"plugin","dsh":{"bundle":{"patch":"./patch.yml"},"client":{"inject":["@deepseek-ai/dsh-client-store"]}}}"#,
        );
        write(
            &core.join("node_modules/@deepseek-ai/dsh-client-store/package.json"),
            r#"{"name":"@deepseek-ai/dsh-client-store"}"#,
        );
        let report = inspect(&core, &profile).unwrap();
        assert!(report.compatible, "{:?}", report.issues);
    }

    #[test]
    fn rejects_plain_dependency_that_cannot_activate_as_plugin() {
        let root = root("plain-dependency");
        let core = root.join("core");
        let profile = root.join("profile");
        write(
            &profile.join("package.json"),
            r#"{"dependencies":{"plugin":"1.0.0"}}"#,
        );
        write(
            &profile.join("node_modules/plugin/package.json"),
            r#"{"name":"plugin"}"#,
        );
        let report = inspect(&core, &profile).unwrap();
        assert_eq!(report.issues.len(), 1);
        assert_eq!(report.issues[0].code, "PLUGIN_BUNDLE_MISSING");
    }
}
