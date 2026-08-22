use std::{path::Path, time::Duration};

use anyhow::Context;
use serde::{Deserialize, Serialize};

pub(crate) const RELEASES_URL: &str = "https://github.com/iamazy/termua/releases/tag";
const RELEASES_API_URL: &str = "https://api.github.com/repos/iamazy/termua/releases?per_page=100";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CheckResult {
    UpdateAvailable { tag: String, url: String },
    UpToDate,
}

#[derive(Debug, Deserialize, Serialize)]
struct StartupUpdate {
    tag: String,
    url: String,
    #[serde(default)]
    suppressed: bool,
}

fn startup_update_path() -> std::path::PathBuf {
    crate::settings::settings_dir_path().join("update.json")
}

fn read_startup_state(path: &Path) -> Option<StartupUpdate> {
    let contents = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&contents).ok()
}

fn load_startup_update_at(path: &Path, current_version: &str) -> Option<CheckResult> {
    let update = read_startup_state(path)?;
    (!update.suppressed && is_newer_version(current_version, &update.tag)).then_some(
        CheckResult::UpdateAvailable {
            tag: update.tag,
            url: update.url,
        },
    )
}

pub(crate) fn load_startup_update() -> Option<CheckResult> {
    load_startup_update_at(&startup_update_path(), env!("CARGO_PKG_VERSION"))
}

fn persist_startup_result_at(path: &Path, result: &CheckResult) -> anyhow::Result<()> {
    match result {
        CheckResult::UpdateAvailable { tag, url } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("create update state directory {parent:?}"))?;
            }
            let suppressed = read_startup_state(path)
                .is_some_and(|previous| previous.tag == *tag && previous.suppressed);
            let contents = serde_json::to_string(&StartupUpdate {
                tag: tag.clone(),
                url: url.clone(),
                suppressed,
            })?;
            crate::atomic_write::write_string(path, &contents)
        }
        CheckResult::UpToDate => match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("remove update state {path:?}")),
        },
    }
}

pub(crate) fn persist_startup_result(result: &CheckResult) -> anyhow::Result<()> {
    persist_startup_result_at(&startup_update_path(), result)
}

fn set_startup_update_suppressed_at(
    path: &Path,
    tag: &str,
    url: &str,
    suppressed: bool,
) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut update = read_startup_state(path)
        .filter(|update| update.tag == tag)
        .unwrap_or_else(|| StartupUpdate {
            tag: tag.to_string(),
            url: url.to_string(),
            suppressed: false,
        });
    update.suppressed = suppressed;
    crate::atomic_write::write_string(path, &serde_json::to_string(&update)?)
}

pub(crate) fn set_startup_update_suppressed(
    tag: &str,
    url: &str,
    suppressed: bool,
) -> anyhow::Result<()> {
    set_startup_update_suppressed_at(&startup_update_path(), tag, url, suppressed)
}

pub(crate) fn parse_version_tag(tag: &str) -> Option<semver::Version> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    semver::Version::parse(version).ok()
}

pub(crate) fn is_newer_version(current: &str, latest: &str) -> bool {
    match (parse_version_tag(current), parse_version_tag(latest)) {
        (Some(current), Some(latest)) => latest > current,
        _ => false,
    }
}

fn highest_release_tag(body: &str) -> Option<String> {
    serde_json::from_str::<Vec<serde_json::Value>>(body)
        .ok()?
        .into_iter()
        .filter_map(|release| release.get("tag_name")?.as_str().map(ToOwned::to_owned))
        .filter_map(|tag| parse_version_tag(&tag).map(|version| (version, tag)))
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, tag)| tag)
}

pub(crate) fn check_latest() -> anyhow::Result<CheckResult> {
    let response = ureq::get(RELEASES_API_URL)
        .set("User-Agent", concat!("termua/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(10))
        .call()?;
    let body = response.into_string()?;
    let tag = highest_release_tag(&body)
        .ok_or_else(|| anyhow::anyhow!("GitHub response has no versioned release tag"))?;
    if is_newer_version(env!("CARGO_PKG_VERSION"), &tag) {
        Ok(CheckResult::UpdateAvailable {
            url: format!("{RELEASES_URL}/{tag}"),
            tag,
        })
    } else {
        Ok(CheckResult::UpToDate)
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn sanitize_filename_component(value: &str) -> String {
        let sanitized: String = value
            .chars()
            .map(|c| match c {
                '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
                c if c.is_control() => '-',
                c => c,
            })
            .collect();

        // Keep the component short to stay well below Windows MAX_PATH.
        let max_len = 80;
        let mut sanitized = if sanitized.len() > max_len {
            sanitized[..max_len].to_string()
        } else {
            sanitized
        };
        while sanitized.ends_with([' ', '.']) {
            sanitized.pop();
        }
        sanitized
    }

    fn unique_state_path(test_name: &str) -> std::path::PathBuf {
        let thread_name = std::thread::current()
            .name()
            .map(sanitize_filename_component)
            .unwrap_or_else(|| "test".to_string());

        std::env::temp_dir().join(format!(
            "termua-update-{}-{}-{}.json",
            sanitize_filename_component(test_name),
            std::process::id(),
            thread_name
        ))
    }

    #[test]
    fn parses_release_tags_and_compares_versions() {
        let version = semver::Version::new(1, 2, 3);
        assert_eq!(parse_version_tag("v1.2.3"), Some(version.clone()));
        assert_eq!(parse_version_tag("1.2.3"), Some(version));
        assert_eq!(parse_version_tag("release-1.2.3"), None);
        assert_eq!(parse_version_tag("1.2.3.4"), None);
        assert_eq!(parse_version_tag("1.2"), None);
        assert!(is_newer_version("0.1.5", "v0.1.6"));
        assert!(!is_newer_version("0.1.5", "v0.1.5"));
        assert!(!is_newer_version("0.1.5", "v0.1.4"));
    }

    #[test]
    fn finds_highest_release_tag_including_prereleases() {
        let body = r#"[
            {"tag_name":"v0.1.5"},
            {"tag_name":"v0.2.0-beta.1"},
            {"tag_name":"not-a-version"}
        ]"#;
        assert_eq!(highest_release_tag(body).as_deref(), Some("v0.2.0-beta.1"));
        assert!(is_newer_version("0.1.5", "v0.2.0-beta.1"));
    }

    #[test]
    fn update_available_result_is_saved_and_loaded_for_next_startup() {
        let path = unique_state_path("saved-result");
        let result = CheckResult::UpdateAvailable {
            tag: "v0.1.5".to_string(),
            url: "https://github.com/iamazy/termua/releases/tag/v0.1.5".to_string(),
        };
        persist_startup_result_at(&path, &result).unwrap();
        assert_eq!(load_startup_update_at(&path, "0.1.4"), Some(result));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn up_to_date_result_clears_previous_startup_update() {
        let path = unique_state_path("clear-result");
        std::fs::write(&path, r#"{"tag":"v0.1.5","url":"https://example.com"}"#).unwrap();
        persist_startup_result_at(&path, &CheckResult::UpToDate).unwrap();
        assert!(!Path::new(&path).exists());
    }

    #[test]
    fn cached_version_not_newer_than_current_is_ignored() {
        let path = unique_state_path("stale-result");
        std::fs::write(&path, r#"{"tag":"v0.1.5","url":"https://example.com"}"#).unwrap();
        assert_eq!(load_startup_update_at(&path, "0.1.5"), None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn suppressed_cached_version_is_not_loaded() {
        let path = unique_state_path("suppressed-result");
        std::fs::write(&path, r#"{"tag":"v0.1.5","url":"https://example.com"}"#).unwrap();
        set_startup_update_suppressed_at(&path, "v0.1.5", "https://example.com", true).unwrap();
        assert_eq!(load_startup_update_at(&path, "0.1.4"), None);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn refreshing_same_release_preserves_suppression_but_new_release_resets_it() {
        let path = unique_state_path("preserve-suppression");
        let release = |tag: &str| CheckResult::UpdateAvailable {
            tag: tag.to_string(),
            url: format!("https://example.com/{tag}"),
        };
        persist_startup_result_at(&path, &release("v0.1.5")).unwrap();
        set_startup_update_suppressed_at(&path, "v0.1.5", "https://example.com/v0.1.5", true)
            .unwrap();
        persist_startup_result_at(&path, &release("v0.1.5")).unwrap();
        assert_eq!(load_startup_update_at(&path, "0.1.4"), None);
        persist_startup_result_at(&path, &release("v0.1.6")).unwrap();
        assert_eq!(
            load_startup_update_at(&path, "0.1.4"),
            Some(release("v0.1.6"))
        );
        let _ = std::fs::remove_file(path);
    }
}
