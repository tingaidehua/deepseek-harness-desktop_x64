use serde::Deserialize;

const BUNDLED_CONFIG: &str = include_str!("../../resources/dsh-distribution.json");
const VERSION_ENV: &str = "DSH_DESKTOP_VERSION";
const MANIFEST_URL_ENV: &str = "DSH_DESKTOP_MANIFEST_URL";

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum BundledSource {
    Github,
    Manifest { url: String },
}

#[derive(Debug, Clone, Deserialize)]
struct BundledDistribution {
    version: String,
    source: BundledSource,
}

/// 桌面端当前选择的 Harness 版本和可选 manifest 地址。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DshDistribution {
    pub version: String,
    pub manifest_url: Option<String>,
}

/// 读取捆绑配置，并允许环境变量覆盖版本和本地 manifest 地址。
pub fn dsh_distribution() -> Result<DshDistribution, String> {
    let bundled: BundledDistribution = serde_json::from_str(BUNDLED_CONFIG)
        .map_err(|error| format!("DSH_DISTRIBUTION_CONFIG_INVALID: {error}"))?;
    let version = std::env::var(VERSION_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(bundled.version);
    semver::Version::parse(&version)
        .map_err(|error| format!("DSH_DISTRIBUTION_VERSION_INVALID: {error}"))?;

    let manifest_url = match std::env::var(MANIFEST_URL_ENV) {
        Ok(value) if value.trim().is_empty() => None,
        Ok(value) => Some(value),
        Err(_) => match bundled.source {
            BundledSource::Github => None,
            BundledSource::Manifest { url } => Some(url),
        },
    };
    Ok(DshDistribution {
        version,
        manifest_url,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_distribution_selects_alpha_from_github() {
        let bundled: BundledDistribution = serde_json::from_str(BUNDLED_CONFIG).unwrap();
        assert_eq!(bundled.version, "0.1.2-alpha.1");
        assert!(matches!(bundled.source, BundledSource::Github));
    }
}
