//! `cargo xtask bundle`: build the release app and wrap it as `dist/DLightroom.app` (macOS), with the
//! committed app icon `assets/app-icon/dlightroom.icns` (regenerate it with `packaging/icons.sh`).
//! `packaging/macos/build-app.sh` makes the same bundle for a chosen architecture and zips it.

use std::path::Path;
use std::process::Command;

pub fn run(root: &Path) -> Result<(), String> {
    let status = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .current_dir(root)
        .args(["build", "--release", "-p", "vectorcraft", "-p", "vectorcraft-cli"])
        .status()
        .map_err(|e| e.to_string())?;
    if !status.success() {
        return Err("release build failed".into());
    }
    let target = std::env::var("CARGO_TARGET_DIR").map(std::path::PathBuf::from).unwrap_or_else(|_| root.join("target"));
    let app = root.join(format!("dist/{APP_BUNDLE}/Contents"));
    let _ = std::fs::remove_dir_all(root.join(format!("dist/{APP_BUNDLE}")));
    std::fs::create_dir_all(app.join("MacOS")).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(app.join("Resources")).map_err(|e| e.to_string())?;
    std::fs::copy(target.join("release/vectorcraft"), app.join(format!("MacOS/{APP_NAME}"))).map_err(|e| format!("copy app: {e}"))?;
    std::fs::copy(target.join("release/vectorcraft-cli"), app.join("MacOS/vectorcraft-cli")).map_err(|e| format!("copy cli: {e}"))?;
    std::fs::copy(root.join("assets/app-icon/dlightroom.icns"), app.join(format!("Resources/{APP_NAME}.icns")))
        .map_err(|e| format!("copy icon: {e}"))?;
    let sha = std::env::var("VECTORCRAFT_BUILD_SHA").unwrap_or_else(|_| "unknown".into());
    std::fs::write(app.join("Info.plist"), info_plist(env!("CARGO_PKG_VERSION"), &sha)).map_err(|e| e.to_string())?;
    println!("built {}", root.join(format!("dist/{APP_BUNDLE}")).display());
    Ok(())
}

/// The app's display name: the bundle, its executable and its icon file are named after it
/// (`vectorcraft_engine::brand::APP_NAME`; xtask doesn't depend on the engine, a test checks they agree).
const APP_NAME: &str = "DLightroom";
const APP_BUNDLE: &str = "DLightroom.app";

/// The release `Info.plist` template, which `packaging/macos/package.sh` fills in the same way: the
/// development bundle declares the same document types (Finder opens them with the app).
const INFO_PLIST: &str = include_str!("../../packaging/macos/Info.plist.in");

/// The bundle's `Info.plist`: [`INFO_PLIST`] for `version` and the build commit `sha`.
fn info_plist(version: &str, sha: &str) -> String {
    let short = version.split('-').next().unwrap_or(version);
    INFO_PLIST.replace("@VERSION@", version).replace("@SHORT_VERSION@", short).replace("@BUILD_SHA@", sha)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// File types and wording a raster image editor's packaging carries (lowercase).
    const RASTER_EDITOR: &[&str] = &["pcraft", "psd", "psb", "qoi", "photo", "rastergraphics", "image editor"];

    fn text_files(dir: &Path, out: &mut Vec<(String, String)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                text_files(&p, out);
            } else if let Ok(t) = std::fs::read_to_string(&p) {
                out.push((p.display().to_string(), t.to_ascii_lowercase()));
            }
        }
    }

    #[test]
    fn packaging_describes_a_vector_app() {
        let mut files = vec![];
        text_files(&crate::root().join("packaging"), &mut files);
        assert!(files.len() > 5, "packaging files found");
        for (path, text) in &files {
            for w in RASTER_EDITOR {
                assert!(!text.contains(w), "{path} mentions `{w}`");
            }
        }
        let read = |p: &str| std::fs::read_to_string(crate::root().join(p)).unwrap();
        let plist = read("packaging/macos/Info.plist.in");
        assert!(plist.contains("<array><string>vectorcraft</string><string>drawcraft</string></array>"), "native format extensions");
        assert!(read("packaging/linux/ai.storyteller.vectorcraft.desktop").contains("Categories=Graphics;2DGraphics;VectorGraphics;"));
        assert!(read("packaging/linux/ai.storyteller.vectorcraft.metainfo.xml.in").contains("<category>VectorGraphics</category>"));
    }

    /// The bundle, its executable, icon and identifier are DLightroom's, the document formats stay
    /// VectorCraft's, and every packaging script names the app the same way.
    #[test]
    fn bundle_is_branded_dlightroom() {
        let read = |p: &str| std::fs::read_to_string(crate::root().join(p)).unwrap();
        let brand = read("crates/engine/src/brand.rs");
        assert!(brand.contains(&format!("pub const APP_NAME: &str = \"{APP_NAME}\";")), "xtask and the engine agree on the name");
        let plist = info_plist("1.0.0", "abc");
        for key in ["CFBundleName", "CFBundleDisplayName", "CFBundleExecutable", "CFBundleIconFile"] {
            assert!(plist.contains(&format!("<key>{key}</key>\n  <string>{APP_NAME}</string>")), "{key}");
        }
        assert!(plist.contains("<string>com.dangelolane11.dlightroom.dev</string>"), "a bundle id of its own");
        assert!(brand.contains("\"com.dangelolane11.dlightroom.dev\""), "the engine knows the bundle id");
        assert!(crate::root().join("assets/app-icon/dlightroom.icns").is_file());
        for script in ["packaging/macos/build-app.sh", "packaging/macos/package.sh"] {
            let text = read(script);
            assert!(text.contains("DLightroom.app") && text.contains("dlightroom.icns"), "{script}");
            assert!(!text.contains("VectorCraft.app"), "{script} names the old bundle");
        }
        let workflow = read(".github/workflows/macos-intel.yml");
        assert!(workflow.contains("runs-on: macos-15-intel") && workflow.contains("packaging/macos/build-app.sh"));
        assert!(!workflow.contains("secrets."), "the macOS build needs no signing credentials");
    }

    /// The distributed app carries no ArtCraft logo files.
    #[test]
    fn no_artcraft_logos_ship() {
        let mut files = vec![];
        text_or_binary(&crate::root().join("assets"), &mut files);
        text_or_binary(&crate::root().join("docs/brand"), &mut files);
        text_or_binary(&crate::root().join("packaging"), &mut files);
        for f in &files {
            let name = f.to_ascii_lowercase();
            assert!(!name.contains("artcraft"), "{f}");
        }
    }

    fn text_or_binary(dir: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                text_or_binary(&p, out);
            } else {
                out.push(p.file_name().unwrap().to_string_lossy().to_string());
            }
        }
    }

    #[test]
    fn dev_bundle_plist_is_the_release_one() {
        let p = info_plist("1.2.3-rc.1", "abc123");
        assert!(p.contains("<string>1.2.3</string>") && p.contains("<string>1.2.3-rc.1</string>"), "{p}");
        assert!(p.contains("<string>abc123</string>"));
        assert!(!p.contains('@'), "every placeholder filled");
        assert!(p.contains("<key>CFBundleDocumentTypes</key>") && p.contains("<string>pdf</string>"), "Finder opens files with it");
    }
}
