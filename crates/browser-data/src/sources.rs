//! The browsers an import can copy from, one per installed browser, in the order Settings lists
//! them.

use std::path::{Path, PathBuf};

use crate::{Family, Profile, app_dirs, discover_in};

/// The browsers listed first, by key, in this order; every other browser follows by name, and
/// the stores found only by their shape ("‹App› (Chromium)") come last.
const ORDER: &[&str] = &[
    "chrome", "chromium", "helium", "edge", "brave", "vivaldi", "opera", "arc", "dia", "firefox",
    "zen", "safari",
];

/// One installed browser and the profiles found for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    /// The browser's key, the part of its profiles' ids before the `/`, e.g. `dia`.
    pub key: String,
    /// Its display name, e.g. "Google Chrome" or "Comet (Chromium)".
    pub name: String,
    pub family: Family,
    /// Its app bundle, which exists.
    pub app_path: PathBuf,
    /// Its profiles, the browser's default one first.
    pub profiles: Vec<Profile>,
}

/// Where a browser sorts: its place in the fixed order, then other known browsers, then the
/// ones found by shape.
pub fn browser_rank(key: &str, name: &str) -> (usize, String) {
    let rank = match ORDER.iter().position(|known| *known == key) {
        Some(at) => at,
        None if name.ends_with(" (Chromium)") || name.ends_with(" (Firefox)") => ORDER.len() + 1,
        None => ORDER.len(),
    };
    (rank, name.to_lowercase())
}

/// The installed browsers with profiles under `home`, in Settings' order. A browser whose app
/// isn't installed is left out even when its folders remain.
pub fn sources(home: &Path) -> Vec<Source> {
    let dirs = app_dirs(home);
    sources_in(home, &dirs.iter().map(PathBuf::as_path).collect::<Vec<_>>())
}

/// [`sources`] with explicit folders to look for app bundles in.
pub fn sources_in(home: &Path, app_dirs: &[&Path]) -> Vec<Source> {
    let mut sources: Vec<Source> = Vec::new();
    for profile in discover_in(home, app_dirs) {
        let Some(app) = profile.app_path.clone().filter(|app| app.is_dir()) else {
            continue;
        };
        let key = profile.id.split('/').next().unwrap_or_default().to_owned();
        match sources.iter_mut().find(|source| source.key == key) {
            Some(source) => source.profiles.push(profile),
            None => sources.push(Source {
                key,
                name: profile.browser.clone(),
                family: profile.family,
                app_path: app,
                profiles: vec![profile],
            }),
        }
    }
    for source in &mut sources {
        // Stable: the rest keep discovery's order by name.
        source.profiles.sort_by_key(|profile| !profile.is_default);
    }
    sources.sort_by_cached_key(|source| browser_rank(&source.key, &source.name));
    sources
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_browsers_rank_in_order_then_others_then_found_by_shape() {
        let mut names = [
            ("comet-chromium", "Comet (Chromium)"),
            ("safari", "Safari"),
            ("thor", "Thor"),
            ("dia", "Dia"),
            ("chrome", "Google Chrome"),
            ("librewolf", "LibreWolf"),
            ("mercury", "Mercury (Firefox)"),
            ("helium", "Helium"),
        ];
        names.sort_by_cached_key(|(key, name)| browser_rank(key, name));
        assert_eq!(
            names.iter().map(|(_, name)| *name).collect::<Vec<_>>(),
            [
                "Google Chrome",
                "Helium",
                "Dia",
                "Safari",
                "LibreWolf",
                "Thor",
                "Comet (Chromium)",
                "Mercury (Firefox)",
            ]
        );
    }
}
