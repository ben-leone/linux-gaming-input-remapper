use std::path::PathBuf;
use super::{AppSettings, Profile};

pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config")
        })
        .join("gameremap")
}

pub fn profiles_dir() -> PathBuf {
    config_dir().join("profiles")
}

// ── Settings ──────────────────────────────────────────────────────────────────

pub fn load_settings() -> AppSettings {
    let path = config_dir().join("settings.toml");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
        .unwrap_or_default()
}

pub fn save_settings(settings: &AppSettings) {
    let path = config_dir().join("settings.toml");
    if std::fs::create_dir_all(path.parent().unwrap()).is_err() {
        return;
    }
    if let Ok(s) = toml::to_string_pretty(settings) {
        let _ = std::fs::write(path, s);
    }
}

// ── Profiles ──────────────────────────────────────────────────────────────────

pub fn load_profiles() -> Vec<Profile> {
    let dir = profiles_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };

    let mut profiles: Vec<Profile> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().map_or(false, |ext| ext == "toml"))
        .filter_map(|e| {
            std::fs::read_to_string(e.path())
                .ok()
                .and_then(|s| toml::from_str(&s).ok())
        })
        .collect();

    profiles.sort_by(|a, b| a.name.cmp(&b.name));
    profiles
}

/// Filename is the profile UUID so renames don't leave orphan files.
pub fn save_profile(profile: &Profile) {
    let dir = profiles_dir();
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let path = dir.join(format!("{}.toml", profile.id));
    if let Ok(s) = toml::to_string_pretty(profile) {
        let _ = std::fs::write(path, s);
    }
}

pub fn delete_profile(profile: &Profile) {
    let path = profiles_dir().join(format!("{}.toml", profile.id));
    let _ = std::fs::remove_file(path);
}

pub fn load_profile_by_id(id: uuid::Uuid) -> Option<Profile> {
    let path = profiles_dir().join(format!("{id}.toml"));
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| toml::from_str(&s).ok())
}
