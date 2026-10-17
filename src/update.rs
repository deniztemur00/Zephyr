use serde::Deserialize;

const LATEST_RELEASE: &str = "https://api.github.com/repos/deniztemur00/Zephyr/releases/latest";
const RELEASES_PAGE: &str = "https://github.com/deniztemur00/Zephyr/releases/";
const MAX_RESPONSE_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Checking,
    Current,
    Available(Release),
    Failed,
}

#[derive(Deserialize)]
struct GithubRelease {
    tag_name: String,
    html_url: String,
}

pub async fn check(http: reqwest::Client) -> Result<Option<Release>, String> {
    let mut response = http
        .get(LATEST_RELEASE)
        .header(
            reqwest::header::USER_AGENT,
            concat!("Zephyr/", env!("CARGO_PKG_VERSION")),
        )
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .map_err(|err| err.to_string())?;
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|err| err.to_string())? {
        if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
            return Err("release response too large".into());
        }
        body.extend_from_slice(&chunk);
    }
    newer_release(&body, env!("CARGO_PKG_VERSION"))
}

fn newer_release(body: &[u8], current: &str) -> Result<Option<Release>, String> {
    let release: GithubRelease = serde_json::from_slice(body).map_err(|err| err.to_string())?;
    // The link is opened by the shell, so it must point at this repository's releases.
    if !release.html_url.starts_with(RELEASES_PAGE) {
        return Err(format!("unexpected release link {}", release.html_url));
    }
    let latest = parse_version(&release.tag_name)
        .ok_or_else(|| format!("unexpected release tag {}", release.tag_name))?;
    let current = parse_version(current).ok_or("unexpected app version")?;
    Ok((latest > current).then(|| Release {
        version: release.tag_name.trim_start_matches('v').to_owned(),
        url: release.html_url,
    }))
}

fn parse_version(tag: &str) -> Option<[u64; 3]> {
    let mut parts = tag.trim_start_matches('v').split('.').map(str::parse);
    let version = [
        parts.next()?.ok()?,
        parts.next()?.ok()?,
        parts.next()?.ok()?,
    ];
    parts.next().is_none().then_some(version)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(tag: &str, url: &str) -> Vec<u8> {
        serde_json::json!({ "tag_name": tag, "html_url": url, "draft": false })
            .to_string()
            .into_bytes()
    }

    const URL: &str = "https://github.com/deniztemur00/Zephyr/releases/tag/v0.2.0";

    #[test]
    fn finds_a_newer_release() {
        let release = newer_release(&body("v0.2.0", URL), "0.1.0").unwrap();
        assert_eq!(
            release,
            Some(Release {
                version: "0.2.0".into(),
                url: URL.into()
            })
        );
    }

    #[test]
    fn ignores_same_or_older_releases() {
        assert_eq!(newer_release(&body("v0.1.0", URL), "0.1.0").unwrap(), None);
        assert_eq!(newer_release(&body("v0.9.9", URL), "0.10.0").unwrap(), None);
    }

    #[test]
    fn rejects_links_outside_the_repository() {
        let foreign = body("v9.0.0", "https://example.com/zephyr.exe");
        assert!(newer_release(&foreign, "0.1.0").is_err());
        let lookalike = body(
            "v9.0.0",
            "https://github.com/deniztemur00/Zephyr-fake/releases/",
        );
        assert!(newer_release(&lookalike, "0.1.0").is_err());
    }

    #[test]
    fn rejects_malformed_responses() {
        assert!(newer_release(b"", "0.1.0").is_err());
        assert!(newer_release(b"{\"message\":\"Not Found\"}", "0.1.0").is_err());
        assert!(newer_release(&body("nightly", URL), "0.1.0").is_err());
        assert!(newer_release(&body("v1.2.3-beta", URL), "0.1.0").is_err());
    }

    #[test]
    fn parses_versions() {
        assert_eq!(parse_version("v1.2.3"), Some([1, 2, 3]));
        assert_eq!(parse_version("0.10.0"), Some([0, 10, 0]));
        assert_eq!(parse_version("1.2"), None);
        assert_eq!(parse_version("1.2.3.4"), None);
    }
}
