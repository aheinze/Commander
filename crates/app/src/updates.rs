//! GitHub release discovery and authenticated, non-installing downloads.

mod download;
mod github;
mod installation;
#[cfg(test)]
mod tests;

use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use dualpane_core::CancelToken;
use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::{Deserialize, Serialize};

pub(crate) use download::download;
pub(crate) use github::check;
pub(crate) use installation::Installation;

const REPOSITORY: &str = "aheinze/Commander";
pub(crate) const RELEASES_URL: &str = "https://github.com/aheinze/Commander/releases";
const MAX_METADATA: u64 = 1024 * 1024;
const MAX_DOWNLOAD: u64 = 2 * 1024 * 1024 * 1024;
const PUBLIC_KEY: Option<&str> = option_env!("COMMANDER_UPDATE_PUBLIC_KEY");

pub(crate) type Result<T> = std::result::Result<T, Failure>;

#[derive(Clone, Debug)]
pub(crate) struct Failure {
    pub message: String,
    pub retry_at: Option<u64>,
}

impl Failure {
    fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retry_at: None,
        }
    }
}

impl From<std::io::Error> for Failure {
    fn from(error: std::io::Error) -> Self {
        Self::new(format!("Could not read or save the update: {error}"))
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Format {
    AppImage,
    Deb,
    Rpm,
    Arch,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Self::AppImage => "AppImage",
            Self::Deb => "Debian / Ubuntu package",
            Self::Rpm => "Fedora / RPM package",
            Self::Arch => "Arch Linux package",
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::AppImage => ".AppImage",
            Self::Deb => ".deb",
            Self::Rpm => ".rpm",
            Self::Arch => ".pkg.tar.zst",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Asset {
    pub name: String,
    pub architecture: String,
    pub format: Format,
    pub size: u64,
    pub sha256: String,
    pub glibc_minimum: String,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    repository: String,
    version: Version,
    tag: String,
    commit: String,
    assets: Vec<Asset>,
}

#[derive(Clone, Debug)]
pub(crate) struct Available {
    pub version: Version,
    pub notes: String,
    pub assets: Vec<Asset>,
    pub notice: String,
    pub installation: Installation,
}

impl Available {
    pub fn url(&self) -> String {
        format!("{RELEASES_URL}/tag/v{}", self.version)
    }
    pub fn download_url(&self, asset: &Asset) -> String {
        format!("{RELEASES_URL}/download/v{}/{}", self.version, asset.name)
    }
}

#[derive(Clone, Debug)]
pub(crate) enum Outcome {
    Current,
    NoRelease,
    Available(Available),
}

/// This cache contains untrusted discovery data only. Signed manifests are
/// verified afresh; no cached digest can authorize a download.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default)]
pub(crate) struct Cache {
    pub checked_at: Option<u64>,
    pub retry_at: Option<u64>,
    pub skipped: Option<String>,
    etag: Option<String>,
    release: Option<String>,
}

impl Cache {
    pub fn path() -> Option<PathBuf> {
        crate::session::state_directory().map(|directory| directory.join("updates.json"))
    }

    pub fn load(path: &Path) -> Self {
        std::fs::File::open(path)
            .ok()
            .and_then(|file| read_bounded(file, MAX_METADATA * 2).ok())
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let parent = path
            .parent()
            .ok_or_else(|| Failure::new("Update state has no directory."))?;
        std::fs::create_dir_all(parent)?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        let bytes =
            serde_json::to_vec(self).map_err(|_| Failure::new("Could not save update state."))?;
        file.write_all(&bytes)?;
        file.as_file().sync_all()?;
        file.persist(path)
            .map_err(|error| Failure::from(error.error))?;
        Ok(())
    }
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn check_cancel(cancel: &CancelToken) -> Result<()> {
    cancel
        .check()
        .map_err(|_| Failure::new("Update cancelled."))
}

fn read_bounded(reader: impl Read, maximum: u64) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.take(maximum + 1).read_to_end(&mut bytes)?;
    if bytes.len() as u64 > maximum {
        return Err(Failure::new("The update response is too large."));
    }
    Ok(bytes)
}

fn hex<const N: usize>(text: &str) -> Result<[u8; N]> {
    if text.len() != N * 2 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Failure::new("The update verification data is invalid."));
    }
    let mut bytes = [0; N];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16)
            .map_err(|_| Failure::new("The update verification data is invalid."))?;
    }
    Ok(bytes)
}

fn numeric_version(text: &str) -> Option<Vec<u32>> {
    let parts: Vec<_> = text
        .split('.')
        .map(str::parse)
        .collect::<std::result::Result<_, _>>()
        .ok()?;
    (parts.len() >= 2 && parts.len() <= 4).then_some(parts)
}

fn verified_manifest(
    bytes: &[u8],
    signature: &[u8],
    key: &str,
    version: &Version,
) -> Result<Manifest> {
    if bytes.len() > MAX_METADATA as usize {
        return Err(Failure::new("The update manifest is too large."));
    }
    let key = VerifyingKey::from_bytes(&hex(key)?)
        .map_err(|_| Failure::new("The release signing key is invalid."))?;
    let signature = Signature::from_slice(signature)
        .map_err(|_| Failure::new("The update signature is invalid."))?;
    key.verify_strict(bytes, &signature).map_err(|_| {
        Failure::new("Update verification failed. The release signature does not match.")
    })?;
    let manifest: Manifest = serde_json::from_slice(bytes).map_err(|_| {
        Failure::new("The update manifest is not supported by this version of Commander.")
    })?;
    if manifest.schema != 1
        || manifest.repository != REPOSITORY
        || &manifest.version != version
        || manifest.tag != format!("v{version}")
        || manifest.commit.len() != 40
        || !manifest.commit.bytes().all(|byte| byte.is_ascii_hexdigit())
        || manifest.assets.is_empty()
        || manifest.assets.len() > 16
    {
        return Err(Failure::new(
            "The update manifest does not match this release.",
        ));
    }
    let mut names = HashSet::new();
    let mut targets = HashSet::new();
    for asset in &manifest.assets {
        if !safe_name(&asset.name)
            || !asset.name.ends_with(asset.format.suffix())
            || !matches!(asset.architecture.as_str(), "x86_64" | "aarch64")
            || asset.size == 0
            || asset.size > MAX_DOWNLOAD
            || hex::<32>(&asset.sha256).is_err()
            || numeric_version(&asset.glibc_minimum).is_none()
            || !names.insert(&asset.name)
            || !targets.insert((&asset.architecture, asset.format))
        {
            return Err(Failure::new(
                "The update manifest contains an invalid or duplicate package.",
            ));
        }
    }
    Ok(manifest)
}

fn safe_name(name: &str) -> bool {
    !name.starts_with('.')
        && name.len() <= 200
        && !name.is_empty()
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-+".contains(&byte))
}
