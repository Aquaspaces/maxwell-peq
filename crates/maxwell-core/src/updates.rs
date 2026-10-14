//! What GitHub says about Maxwell PEQ's releases, and whether one is newer
//! than this build. Fetching is up to the caller; this only reads the reply.

use serde_json::{Value, json};

/// GitHub's list of the latest releases, newest first.
pub const RELEASES_URL: &str = "https://api.github.com/repos/Aquaspaces/maxwell-peq/releases?per_page=10";

/// Release pages the app may open: anything under the repo's releases.
pub const RELEASES_PAGE: &str = "https://github.com/Aquaspaces/maxwell-peq/releases";

/// `v1.2.3` or `1.2.3-beta` as numbers; anything after a `-` or `+` is ignored.
pub fn version(text: &str) -> Option<(u64, u64, u64)> {
    let text = text.trim().trim_start_matches(['v', 'V']);
    let core = text.split(['-', '+']).next()?;
    let mut parts = core.split('.').map(|part| part.parse::<u64>().ok());
    let major = parts.next()??;
    let minor = parts.next().unwrap_or(Some(0))?;
    let patch = parts.next().unwrap_or(Some(0))?;
    parts.next().is_none().then_some((major, minor, patch))
}

/// The releases in GitHub's reply that a user can install, newest first, each
/// marked `newer` than `current`; and the newest one if it is.
///
/// Drafts, prereleases and tags that aren't versions are left out.
pub fn read(current: &str, reply: &Value) -> Value {
    let mine = version(current);
    let mut releases: Vec<((u64, u64, u64), Value)> = reply
        .as_array()
        .into_iter()
        .flatten()
        .filter(|release| release["draft"] != json!(true) && release["prerelease"] != json!(true))
        .filter_map(|release| {
            let tag = release["tag_name"].as_str()?;
            let number = version(tag)?;
            let url =
                release["html_url"].as_str().filter(|url| url.starts_with(RELEASES_PAGE)).unwrap_or(RELEASES_PAGE);
            let entry = json!({
                "version": tag.trim_start_matches(['v', 'V']),
                "name": release["name"].as_str().filter(|name| !name.trim().is_empty()).unwrap_or(tag),
                "date": release["published_at"].as_str().and_then(|date| date.get(..10)),
                "notes": release["body"].as_str().unwrap_or(""),
                "url": url,
                "newer": mine.is_some_and(|mine| number > mine),
            });
            Some((number, entry))
        })
        .collect();
    releases.sort_by_key(|(number, _)| std::cmp::Reverse(*number));
    let latest = releases.first().map(|(_, entry)| entry.clone());
    let update = latest.as_ref().filter(|entry| entry["newer"] == json!(true));
    json!({
        "current": current,
        "latest": latest.as_ref().map(|entry| entry["version"].clone()),
        "update_available": update.is_some(),
        "url": update.map_or(json!(RELEASES_PAGE), |entry| entry["url"].clone()),
        "releases": releases.into_iter().map(|(_, entry)| entry).collect::<Vec<_>>(),
    })
}

/// Whether the app may open `url`: a page under the repo's releases.
pub fn is_release_page(url: &str) -> bool {
    url == RELEASES_PAGE || url.strip_prefix(RELEASES_PAGE).is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_parse_with_or_without_a_v() {
        assert_eq!(version("v0.1.0"), Some((0, 1, 0)));
        assert_eq!(version("1.2"), Some((1, 2, 0)));
        assert_eq!(version("2.0.1-beta.1"), Some((2, 0, 1)));
        assert_eq!(version("nightly"), None);
        assert_eq!(version("1.2.3.4"), None);
        assert!(version("0.10.0") > version("0.9.9"));
    }

    fn release(tag: &str, draft: bool, prerelease: bool) -> Value {
        json!({"tag_name": tag, "name": format!("Maxwell PEQ {tag}"), "draft": draft, "prerelease": prerelease,
               "published_at": "2026-10-01T12:00:00Z", "body": format!("Notes for {tag}"),
               "html_url": format!("{RELEASES_PAGE}/tag/{tag}")})
    }

    #[test]
    fn a_newer_release_is_an_update() {
        let reply = json!([
            release("v0.1.0", false, false),
            release("v0.3.0", true, false),
            release("v0.2.1", false, true),
            release("v0.2.0", false, false),
            release("latest", false, false)
        ]);
        let result = read("0.1.0", &reply);
        assert_eq!(result["update_available"], true);
        assert_eq!(result["latest"], "0.2.0");
        assert_eq!(result["url"], format!("{RELEASES_PAGE}/tag/v0.2.0"));
        let releases = result["releases"].as_array().unwrap();
        assert_eq!(releases.iter().map(|r| r["version"].as_str().unwrap()).collect::<Vec<_>>(), ["0.2.0", "0.1.0"]);
        assert_eq!((releases[0]["newer"].clone(), releases[1]["newer"].clone()), (json!(true), json!(false)));
        assert_eq!(releases[0]["date"], "2026-10-01");
        assert_eq!(releases[0]["notes"], "Notes for v0.2.0");
    }

    #[test]
    fn the_same_or_an_older_release_is_not_an_update() {
        let reply = json!([release("v0.1.0", false, false)]);
        let result = read("0.1.0", &reply);
        assert_eq!(result["update_available"], false);
        assert_eq!(result["url"], RELEASES_PAGE);
        assert_eq!(read("0.2.0", &reply)["update_available"], false);
        let empty = read("0.1.0", &json!({"message": "rate limited"}));
        assert_eq!((empty["latest"].clone(), empty["releases"].clone()), (Value::Null, json!([])));
    }

    #[test]
    fn only_release_pages_open() {
        assert!(is_release_page(RELEASES_PAGE));
        assert!(is_release_page(&format!("{RELEASES_PAGE}/tag/v0.2.0")));
        assert!(!is_release_page(&format!("{RELEASES_PAGE}evil")));
        assert!(!is_release_page("https://example.com/"));
        let mut odd = release("v0.2.0", false, false);
        odd["html_url"] = json!("https://example.com/");
        assert_eq!(read("0.1.0", &json!([odd]))["url"], RELEASES_PAGE);
    }
}
