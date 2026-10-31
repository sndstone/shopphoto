//! At-most-daily, read-only release check. No automatic download.

use serde::Deserialize;
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone, Deserialize)]
pub struct UpdateInfo {
    pub latest: String,
    pub url: String,
    pub notes: String,
}

pub fn check(url: &str, stamp: &Path) -> Result<Option<UpdateInfo>, String> {
    if let Ok(metadata) = std::fs::metadata(stamp) {
        if metadata
            .modified()
            .ok()
            .and_then(|time| time.elapsed().ok())
            .is_some_and(|elapsed| elapsed < Duration::from_secs(24 * 60 * 60))
        {
            return Ok(None);
        }
    }
    if let Some(parent) = stamp.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut response = ureq::get(url)
        .header("User-Agent", "ShopPhoto update check")
        .call()
        .map_err(|error| error.to_string())?;
    let info: UpdateInfo = response
        .body_mut()
        .read_json()
        .map_err(|error| error.to_string())?;
    let _ = std::fs::write(stamp, b"checked");
    let current = semver::Version::parse(env!("CARGO_PKG_VERSION")).map_err(|e| e.to_string())?;
    let latest =
        semver::Version::parse(info.latest.trim_start_matches('v')).map_err(|e| e.to_string())?;
    Ok((latest > current).then_some(info))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn payload_schema_is_stable() {
        let value: UpdateInfo = serde_json::from_str(
            r#"{"latest":"1.2.3","url":"https://example.test","notes":"Changes"}"#,
        )
        .expect("json");
        assert_eq!(value.latest, "1.2.3");
    }
}
