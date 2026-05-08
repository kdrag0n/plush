use crate::error::{PlushError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Db {
    dirs: BTreeMap<String, Entry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Entry {
    score: f64,
    last: u64,
}

pub fn record(path: &Path) {
    let Ok(path) = path.canonicalize() else {
        return;
    };
    let mut db = load();
    let entry = db
        .dirs
        .entry(path.to_string_lossy().to_string())
        .or_default();
    entry.score += 1.0;
    entry.last = now();
    let _ = save(&db);
}

pub fn find(query: &str) -> Result<PathBuf> {
    let db = load();
    let needle = query.to_ascii_lowercase();
    db.dirs
        .into_iter()
        .filter(|(path, _)| path.to_ascii_lowercase().contains(&needle))
        .max_by(|(a_path, a), (b_path, b)| {
            rank(a_path, a)
                .partial_cmp(&rank(b_path, b))
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(path, _)| PathBuf::from(path))
        .ok_or_else(|| PlushError::msg(format!("z: no match for {query}")))
}

pub fn list() -> Vec<PathBuf> {
    let mut dirs = load().dirs.into_iter().collect::<Vec<_>>();
    dirs.sort_by(|(a_path, a), (b_path, b)| {
        rank(b_path, b)
            .partial_cmp(&rank(a_path, a))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    dirs.into_iter()
        .map(|(path, _)| PathBuf::from(path))
        .collect()
}

fn rank(path: &str, entry: &Entry) -> f64 {
    let age_hours = now().saturating_sub(entry.last) as f64 / 3600.0;
    entry.score / (1.0 + age_hours / 72.0) + path.matches('/').count() as f64 * 0.01
}

fn load() -> Db {
    let path = db_path();
    fs::read_to_string(path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

fn save(db: &Db) -> Result<()> {
    let path = db_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(
        path,
        toml::to_string(db).map_err(|err| PlushError::msg(err.to_string()))?,
    )?;
    Ok(())
}

fn db_path() -> PathBuf {
    if let Some(path) = std::env::var_os("PLUSH_Z_DATA") {
        return PathBuf::from(path);
    }
    dirs::data_dir()
        .unwrap_or_else(|| dirs::home_dir().unwrap_or_else(|| PathBuf::from(".")))
        .join("plush")
        .join("z.toml")
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
