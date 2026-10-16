//! Help → project links: DLightroom's own repository and the upstream VectorCraft project it is
//! built on. The commands return the URL; the frontend opens it.

use serde_json::{Value, json};

use super::*;
use crate::brand::{PROJECT_URL, UPSTREAM_URL};

/// This app's page: its source repository (code, issues and releases).
pub fn app_page_url() -> String {
    PROJECT_URL.to_string()
}

/// The upstream project's source repository.
pub fn github_url() -> String {
    UPSTREAM_URL.to_string()
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "help.appPage", "DLightroom on GitHub", ["Help"], None, "{} → {url} this app's source code, issues and releases", always, |_, _| url(app_page_url())),
        cmd!(query "help.github", "VectorCraft on GitHub", ["Help"], None, "{} → {url} the upstream open-source project this app is built on", always, |_, _| url(github_url())),
        cmd!(query "help.links", "Links", [], None, "{} → {appPage, github}", always, |_, _| {
            Ok(json!({ "appPage": app_page_url(), "github": github_url() }))
        }),
    ]
}

fn url(u: String) -> Result<Value> {
    Ok(json!({ "url": u }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn links() {
        let mut s = Session::new();
        assert_eq!(s.execute("help.appPage", &json!({})).unwrap()["url"], "https://github.com/dangelolane11/dlightroom");
        assert_eq!(s.execute("help.github", &json!({})).unwrap()["url"], "https://github.com/storytold/vectorcraft");
        assert_eq!(s.execute("help.links", &json!({})).unwrap()["appPage"], "https://github.com/dangelolane11/dlightroom");
        // The ArtCraft community links are not part of DLightroom.
        assert!(s.execute("help.discord", &json!({})).is_err());
        assert!(s.execute("help.website", &json!({})).is_err());
    }
}
