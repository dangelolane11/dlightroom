//! The product identity DLightroom ships under: one place for the app's display name, its macOS
//! bundle identifier, its project links and the upstream it is built on.
//!
//! DLightroom is an independently branded build of the open-source VectorCraft workspace (MIT OR
//! Apache-2.0). The crate and package names (`vectorcraft-*`), the native document format
//! (`.vectorcraft`, still opened as `.drawcraft` too) and its format identifiers keep their upstream
//! names on purpose, so documents stay compatible both ways.
//!
//! Menu tables hold `&'static str` labels, so a few of them spell the name out; a test checks that
//! they agree with [`APP_NAME`].

/// The name users see: window title, menus, About box, dialogs.
pub const APP_NAME: &str = "DLightroom";

/// The macOS bundle identifier. Temporary, for local testing: distinct from upstream's
/// `ai.storyteller.vectorcraft`, so both apps can be installed side by side.
pub const BUNDLE_ID: &str = "com.dangelolane11.dlightroom.dev";

/// DLightroom's own source repository (Help › DLightroom on GitHub).
pub const PROJECT_URL: &str = "https://github.com/dangelolane11/dlightroom";

/// The open-source project DLightroom is built on, credited in About and Help.
pub const UPSTREAM_NAME: &str = "VectorCraft";

/// The upstream project's source repository (Help › VectorCraft on GitHub).
pub const UPSTREAM_URL: &str = "https://github.com/storytold/vectorcraft";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_is_dlightroom_and_credits_vectorcraft() {
        assert_eq!(APP_NAME, "DLightroom");
        assert_ne!(BUNDLE_ID, "ai.storyteller.vectorcraft", "a bundle id of its own");
        assert!(BUNDLE_ID.split('.').all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')));
        assert!(PROJECT_URL.ends_with("/dlightroom"));
        assert_eq!(UPSTREAM_NAME, "VectorCraft");
        assert!(UPSTREAM_URL.ends_with("/vectorcraft"));
    }
}
