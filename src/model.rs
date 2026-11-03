use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct ChannelManifest {
    #[serde(rename = "channelItems")]
    pub channel_items: Vec<ChannelItem>,
}

#[derive(Debug, Deserialize)]
pub struct ChannelItem {
    pub id: String,
    #[serde(default)]
    pub payloads: Vec<Payload>,
    #[serde(default, rename = "localizedResources")]
    pub localized_resources: Vec<LocalizedResource>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Payload {
    #[serde(default, rename = "fileName")]
    pub file_name: String,
    pub url: String,
    #[serde(default)]
    pub sha256: String,
}

#[derive(Debug, Deserialize)]
pub struct LocalizedResource {
    pub language: String,
    #[serde(default)]
    pub license: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct VsManifest {
    pub packages: Vec<Package>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Package {
    pub id: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub payloads: Vec<Payload>,
    #[serde(default)]
    pub dependencies: Value,
}

impl Package {
    pub fn dependency_ids(&self) -> Vec<String> {
        match &self.dependencies {
            Value::Object(map) => map.keys().cloned().collect(),
            Value::Array(items) => items
                .iter()
                .filter_map(|v| v.as_str().map(ToOwned::to_owned))
                .collect(),
            _ => Vec::new(),
        }
    }
}

pub struct Catalog {
    packages: Vec<Package>,
    by_id: HashMap<String, usize>,
}

impl Catalog {
    pub fn new(packages: Vec<Package>) -> Self {
        let mut by_id = HashMap::new();
        for (i, p) in packages.iter().enumerate() {
            by_id.entry(p.id.to_ascii_lowercase()).or_insert(i);
        }
        Self { packages, by_id }
    }

    pub fn get(&self, id: &str) -> Option<&Package> {
        self.by_id
            .get(&id.to_ascii_lowercase())
            .map(|i| &self.packages[*i])
    }

    pub fn require(&self, id: &str) -> Result<&Package> {
        self.get(id)
            .ok_or_else(|| anyhow!("package not found in manifest: {id}"))
    }

    pub fn first_dependency(&self, pkg: &Package) -> Result<&Package> {
        let id = pkg
            .dependency_ids()
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("package {} has no dependencies", pkg.id))?;
        self.require(&id)
    }

    pub fn msvc_versions(&self) -> Vec<MsvcVersion> {
        let mut out: HashMap<String, MsvcVersion> = HashMap::new();
        for p in &self.packages {
            let Some(stem) = msvc_stem(&p.id) else { continue };
            let lower = p.id.to_ascii_lowercase();
            if lower.contains("preview") || lower.contains("premium") {
                continue;
            }
            let mut parts = stem.split('.');
            let major = parts.next().unwrap_or("").to_string();
            let minor = parts.next().unwrap_or("").to_string();
            if major.is_empty() || minor.is_empty() {
                continue;
            }
            let minor = format!("{major}.{minor}");
            let full = if p.version.is_empty() {
                stem.clone()
            } else {
                p.version.clone()
            };
            out.entry(stem.clone()).or_insert(MsvcVersion {
                stem,
                minor,
                full,
            });
        }
        let mut out: Vec<_> = out.into_values().collect();
        out.sort_by(|a, b| version_cmp(&a.full, &b.full).then_with(|| a.stem.cmp(&b.stem)));
        out
    }

    pub fn sdk_versions(&self) -> Result<Vec<SdkVersion>> {
        let mut out: HashMap<String, SdkVersion> = HashMap::new();
        for p in &self.packages {
            let lower = p.id.to_ascii_lowercase();
            let prefix = if lower.starts_with("microsoft.visualstudio.component.windows11sdk.") {
                "microsoft.visualstudio.component.windows11sdk."
            } else if lower.starts_with("microsoft.visualstudio.component.windows10sdk.") {
                "microsoft.visualstudio.component.windows10sdk."
            } else {
                continue;
            };
            let short = &p.id[prefix.len()..];
            if short.is_empty() || !short.chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let root = match self.first_dependency(p) {
                Ok(r) => r,
                Err(_) => continue,
            };
            let package_version = if root.version.is_empty() {
                format!("10.0.{short}.0")
            } else {
                root.version.clone()
            };
            out.entry(short.to_string()).or_insert(SdkVersion {
                short: short.to_string(),
                package_version,
                component_id: p.id.clone(),
                package_id: root.id.clone(),
            });
        }
        let mut out: Vec<_> = out.into_values().collect();
        out.sort_by(|a, b| version_cmp(&a.package_version, &b.package_version));
        Ok(out)
    }

    pub fn select_msvc(&self, query: &str) -> Result<SelectedMsvc> {
        let versions = self.msvc_versions();
        let q = query.trim();
        let latest = q.eq_ignore_ascii_case("latest");
        let found = versions
            .iter()
            .filter(|v| {
                latest
                    || q == v.minor
                    || q == v.full
                    || q == v.stem
                    || (q.starts_with(&format!("{}.", v.minor)) && q.split('.').count() == 3)
            })
            .max_by(|a, b| version_cmp(&a.full, &b.full));
        let v = found.ok_or_else(|| {
            anyhow!(
                "unknown MSVC version '{q}'; use list to see available versions"
            )
        })?;
        let package_id = format!("Microsoft.VC.{}.Tools.HostX64.TargetX64.base", v.stem);
        let package = self.require(&package_id)?;
        Ok(SelectedMsvc {
            stem: v.stem.clone(),
            full: v.full.clone(),
            package_id: package.id.clone(),
        })
    }

    pub fn select_sdk(&self, query: &str) -> Result<SelectedSdk> {
        let versions = self.sdk_versions()?;
        let q = query.trim();
        let latest = q.eq_ignore_ascii_case("latest");
        let found = versions
            .iter()
            .filter(|v| {
                latest
                    || q == v.short
                    || q == v.package_version
                    || q.starts_with(&format!("10.0.{}.", v.short))
            })
            .max_by(|a, b| version_cmp(&a.package_version, &b.package_version));
        let v = found.ok_or_else(|| {
            anyhow!(
                "unknown Windows SDK version '{q}'; use list to see available versions"
            )
        })?;
        let component = self.require(&v.component_id)?;
        let root = self.first_dependency(component)?;
        Ok(SelectedSdk {
            short: v.short.clone(),
            package_version: v.package_version.clone(),
            component_id: component.id.clone(),
            package_id: root.id.clone(),
            package: root.clone(),
        })
    }

    pub fn license_url(channel: &ChannelManifest) -> Result<String> {
        let item = channel
            .channel_items
            .iter()
            .find(|i| i.id.eq_ignore_ascii_case("Microsoft.VisualStudio.Product.BuildTools"))
            .ok_or_else(|| anyhow!("BuildTools product not found in channel manifest"))?;
        let resource = item
            .localized_resources
            .iter()
            .find(|r| r.language.eq_ignore_ascii_case("en-us") && r.license.is_some())
            .or_else(|| item.localized_resources.iter().find(|r| r.license.is_some()))
            .ok_or_else(|| anyhow!("license URL not found in channel manifest"))?;
        resource
            .license
            .clone()
            .ok_or_else(|| anyhow!("license URL empty in channel manifest"))
    }
}

#[derive(Debug, Clone)]
pub struct MsvcVersion {
    pub stem: String,
    pub minor: String,
    pub full: String,
}

#[derive(Debug, Clone)]
pub struct SdkVersion {
    pub short: String,
    pub package_version: String,
    pub component_id: String,
    pub package_id: String,
}

pub struct SelectedMsvc {
    pub stem: String,
    pub full: String,
    pub package_id: String,
}

pub struct SelectedSdk {
    pub short: String,
    pub package_version: String,
    pub component_id: String,
    pub package_id: String,
    pub package: Package,
}

fn msvc_stem(id: &str) -> Option<String> {
    let lower = id.to_ascii_lowercase();
    let rest = lower.strip_prefix("microsoft.vc.")?;
    let stem = rest.strip_suffix(".tools.hostx64.targetx64.base")?;
    if stem.is_empty() {
        return None;
    }
    Some(stem.to_string())
}

pub fn version_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let av: Vec<u64> = a
        .split('.')
        .map(|x| x.parse::<u64>().unwrap_or(0))
        .collect();
    let bv: Vec<u64> = b
        .split('.')
        .map(|x| x.parse::<u64>().unwrap_or(0))
        .collect();
    let n = av.len().max(bv.len());
    for i in 0..n {
        let x = av.get(i).copied().unwrap_or(0);
        let y = bv.get(i).copied().unwrap_or(0);
        match x.cmp(&y) {
            std::cmp::Ordering::Equal => {}
            other => return other,
        }
    }
    std::cmp::Ordering::Equal
}

pub fn channel_url(vs: &str, preview: bool) -> Result<&'static str> {
    let url = match (vs, preview) {
        ("2019", false) => "https://aka.ms/vs/16/release/channel",
        ("2019", true) => "https://aka.ms/vs/16/pre/channel",
        ("2022", false) => "https://aka.ms/vs/17/release/channel",
        ("2022", true) => "https://aka.ms/vs/17/pre/channel",
        ("2026", false) => "https://aka.ms/vs/18/release/channel",
        ("2026", true) => "https://aka.ms/vs/18/pre/channel",
        _ => bail!("unsupported VS version '{vs}' (use 2019, 2022 or 2026)"),
    };
    Ok(url)
}

pub fn manifest_item<'a>(
    channel: &'a ChannelManifest,
    preview: bool,
) -> Result<&'a ChannelItem> {
    let id = if preview {
        "Microsoft.VisualStudio.Manifests.VisualStudioPreview"
    } else {
        "Microsoft.VisualStudio.Manifests.VisualStudio"
    };
    channel
        .channel_items
        .iter()
        .find(|i| i.id.eq_ignore_ascii_case(id))
        .ok_or_else(|| anyhow!("channel manifest {id} not found"))
}

pub fn manifest_url(channel: &ChannelManifest, preview: bool) -> Result<String> {
    manifest_item(channel, preview)?
        .payloads
        .first()
        .map(|p| p.url.clone())
        .ok_or_else(|| anyhow!("manifest payload URL missing"))
}

pub fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8], what: &str) -> Result<T> {
    serde_json::from_slice(bytes).with_context(|| format!("invalid {what} JSON"))
}

