//! Where DLightroom keeps its settings, `ui.json`, and next to it folders of the user's own
//! (Swatches, Graphic Styles, Fonts, Data Recovery, logs). The desktop app and `vectorcraft-cli`
//! (which shares this file) read the same Fonts folder.

use std::path::PathBuf;

#[path = "profile.rs"]
mod profile;

/// The settings file `ui.json` of the app `name` (one of [`profile::FOLDERS`]; another name means
/// DLightroom's own): in a folder named `name` in ~/Library/Application Support (macOS) or
/// %APPDATA% (Windows), or `lower` in $XDG_CONFIG_HOME or ~/.config (Linux and BSD).
pub fn prefs_path_for(name: &str, lower: &str) -> Option<PathBuf> {
    let folder = profile::FOLDERS.iter().position(|(n, l)| *n == name || *l == lower).unwrap_or(0);
    profile::prefs_path(folder)
}

/// The settings files of the apps DLightroom reads settings from while it has none of its own,
/// newest first: the upstream VectorCraft's, then DrawCraft's.
#[allow(dead_code)] // The CLI shares this file and reads no UI settings.
pub fn legacy_prefs_paths() -> impl Iterator<Item = PathBuf> {
    (1..profile::FOLDERS.len()).filter_map(profile::prefs_path)
}

/// Have the font scans read DLightroom's own Fonts folder, next to the settings, which
/// `text.addFontFiles` copies font files into. The folder is set whether or not
/// `VECTORCRAFT_NO_PREFS` is set, as the Swatches and Graphic Styles folders are.
pub fn install_fonts() {
    if let Some(dir) = prefs_path_for("DLightroom", "dlightroom").and_then(|p| Some(p.parent()?.join("Fonts"))) {
        vectorcraft_text::set_app_font_dir(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_font_scans_read_the_fonts_folder_next_to_the_settings() {
        install_fonts();
        let fonts = prefs_path_for("DLightroom", "dlightroom").and_then(|p| Some(p.parent()?.join("Fonts")));
        assert!(fonts.is_some(), "a home folder to keep the settings in");
        assert_eq!(vectorcraft_text::app_font_dir().map(std::path::Path::to_path_buf), fonts);
        assert!(vectorcraft_text::system_font_dirs().contains(&fonts.unwrap_or_default()));
    }
}
