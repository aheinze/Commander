use std::time::Duration;

use super::*;

const LATEST: &str = "https://api.github.com/repos/aheinze/Commander/releases/latest";

#[derive(Deserialize)]
struct Release {
    tag_name: String,
    draft: bool,
    prerelease: bool,
    body: Option<String>,
    assets: Vec<ReleaseAsset>,
}

#[derive(Deserialize)]
struct ReleaseAsset {
    name: String,
    size: u64,
    state: String,
}

pub(super) struct Response {
    pub status: u16,
    pub etag: Option<String>,
    pub retry_at: Option<u64>,
    pub body: Vec<u8>,
}

pub(super) trait Transport {
    fn get(&self, url: &str, etag: Option<&str>, maximum: u64) -> Result<Response>;
}

struct Github;

pub(super) fn agent(seconds: u64) -> ureq::Agent {
    ureq::Agent::config_builder()
        .https_only(true)
        .http_status_as_error(false)
        .max_redirects(5)
        .timeout_global(Some(Duration::from_secs(seconds)))
        .timeout_connect(Some(Duration::from_secs(10)))
        .timeout_recv_response(Some(Duration::from_secs(20)))
        .build()
        .into()
}

pub(super) fn response_failure(status: u16, retry_at: Option<u64>) -> Failure {
    let message = match status {
        403 | 429 => "GitHub is limiting update requests. Please try again later.",
        404 => "This release download is no longer available. Check for updates again.",
        _ => "GitHub could not complete the update request. Please try again later.",
    };
    Failure {
        message: message.into(),
        retry_at,
    }
}

impl Transport for Github {
    fn get(&self, url: &str, etag: Option<&str>, maximum: u64) -> Result<Response> {
        let client = agent(30);
        let mut request = client
            .get(url)
            .header(
                "User-Agent",
                concat!("Commander/", env!("CARGO_PKG_VERSION")),
            )
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2026-03-10");
        if let Some(etag) = etag {
            request = request.header("If-None-Match", etag);
        }
        let response = request.call().map_err(|_| {
            Failure::new("Could not reach GitHub. Check your connection and try again.")
        })?;
        let status = response.status().as_u16();
        let etag = response
            .headers()
            .get("etag")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let retry_at = if matches!(status, 403 | 429) {
            let seconds = response
                .headers()
                .get("retry-after")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok())
                .map(|seconds| now().saturating_add(seconds));
            let reset = response
                .headers()
                .get("x-ratelimit-reset")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<u64>().ok());
            Some(
                seconds
                    .into_iter()
                    .chain(reset)
                    .max()
                    .unwrap_or_else(|| now() + 60)
                    .max(now() + 60),
            )
        } else {
            None
        };
        let body = if status == 200 {
            read_bounded(response.into_body().into_reader(), maximum)?
        } else {
            Vec::new()
        };
        Ok(Response {
            status,
            etag,
            retry_at,
            body,
        })
    }
}

pub(crate) fn check(cache: &mut Cache, cancel: &CancelToken) -> Result<Outcome> {
    check_with(
        &Github,
        cache,
        cancel,
        PUBLIC_KEY,
        env!("CARGO_PKG_VERSION"),
        std::env::consts::ARCH,
        installation::host_glibc(),
        Installation::detect(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn check_with(
    client: &impl Transport,
    cache: &mut Cache,
    cancel: &CancelToken,
    key: Option<&str>,
    current: &str,
    architecture: &str,
    glibc: Option<Vec<u32>>,
    installation: Installation,
) -> Result<Outcome> {
    check_cancel(cancel)?;
    if let Some(retry_at) = cache.retry_at.filter(|time| *time > now()) {
        return Err(Failure {
            message: "Please wait before checking again.".into(),
            retry_at: Some(retry_at),
        });
    }
    // Throttle repeated manual checks too; unauthenticated limits are shared by IP.
    cache.retry_at = Some(now() + 60);
    let result = check_release(
        client,
        cache,
        cancel,
        key,
        current,
        architecture,
        glibc,
        installation,
    );
    if result.is_ok() {
        cache.checked_at = Some(now());
    }
    if let Err(error) = &result
        && let Some(retry_at) = error.retry_at
    {
        cache.retry_at = Some(retry_at);
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn check_release(
    client: &impl Transport,
    cache: &mut Cache,
    cancel: &CancelToken,
    key: Option<&str>,
    current: &str,
    architecture: &str,
    glibc: Option<Vec<u32>>,
    installation: Installation,
) -> Result<Outcome> {
    let response = client.get(LATEST, cache.etag.as_deref(), MAX_METADATA)?;
    check_cancel(cancel)?;
    let body = match response.status {
        200 => String::from_utf8(response.body)
            .map_err(|_| Failure::new("GitHub returned invalid release information."))?,
        304 => cache
            .release
            .clone()
            .ok_or_else(|| Failure::new("The update cache is incomplete. Please check again."))?,
        404 => {
            cache.etag = None;
            cache.release = None;
            return Ok(Outcome::NoRelease);
        }
        status => return Err(response_failure(status, response.retry_at)),
    };
    let release: Release = serde_json::from_str(&body)
        .map_err(|_| Failure::new("GitHub returned invalid release information."))?;
    let version = release
        .tag_name
        .strip_prefix('v')
        .and_then(|text| Version::parse(text).ok())
        .ok_or_else(|| Failure::new("The release has an unsupported version number."))?;
    let current = Version::parse(current)
        .map_err(|_| Failure::new("This build has an unsupported version number."))?;
    if release.draft
        || release.prerelease
        || !version.pre.is_empty()
        || !version.build.is_empty()
        || release.assets.len() > 64
    {
        return Err(Failure::new(
            "GitHub did not return a supported stable release.",
        ));
    }
    if response.status == 200 {
        cache.etag = response.etag;
        cache.release = Some(body);
    }
    if version.cmp_precedence(&current).is_le() {
        return Ok(Outcome::Current);
    }
    let mut available = Available {
        version,
        notes: release
            .body
            .unwrap_or_default()
            .chars()
            .take(16_000)
            .collect(),
        assets: Vec::new(),
        notice: String::new(),
        installation,
    };
    let Some(key) = key else {
        available.notice =
            "This build has no release verification key. View the release on GitHub for downloads."
                .into();
        return Ok(Outcome::Available(available));
    };
    let has = |name: &str| {
        release
            .assets
            .iter()
            .filter(|asset| asset.name == name && asset.state == "uploaded")
            .count()
            == 1
    };
    if !has("update.json") || !has("update.json.sig") {
        available.notice =
            "This release has no signed update metadata. View the release on GitHub for downloads."
                .into();
        return Ok(Outcome::Available(available));
    }
    let prefix = format!("{RELEASES_URL}/download/{}", release.tag_name);
    let metadata = client.get(&format!("{prefix}/update.json"), None, MAX_METADATA)?;
    check_cancel(cancel)?;
    if metadata.status != 200 {
        return Err(response_failure(metadata.status, metadata.retry_at));
    }
    let signature = client.get(&format!("{prefix}/update.json.sig"), None, 64)?;
    check_cancel(cancel)?;
    if signature.status != 200 {
        return Err(response_failure(signature.status, signature.retry_at));
    }
    let manifest = verified_manifest(&metadata.body, &signature.body, key, &available.version)?;
    for asset in &manifest.assets {
        if release
            .assets
            .iter()
            .filter(|remote| {
                remote.name == asset.name && remote.size == asset.size && remote.state == "uploaded"
            })
            .count()
            != 1
        {
            return Err(Failure::new(
                "The signed package list does not match the GitHub release.",
            ));
        }
    }
    available.assets = manifest
        .assets
        .into_iter()
        .filter(|asset| {
            asset.architecture == architecture
                && glibc.as_ref().is_some_and(|host| {
                    numeric_version(&asset.glibc_minimum).is_some_and(|minimum| host >= &minimum)
                })
        })
        .collect();
    if available.assets.is_empty() {
        available.notice = "No verified package matches this computer’s architecture and glibc version. See the release requirements on GitHub.".into();
    }
    Ok(Outcome::Available(available))
}
