use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct PathCache {
    path: String,
    cwd: PathBuf,
    commands: BTreeMap<String, Option<PathBuf>>,
}

impl PathCache {
    pub fn resolve(&mut self, name: &str, path: Option<&str>, cwd: &Path) -> Option<PathBuf> {
        self.refresh_generation(path.unwrap_or_default(), cwd);
        if let Some(cached) = self.commands.get(name) {
            return cached.clone();
        }
        let found = which::which_in(name, Some(path.unwrap_or_default()), cwd).ok();
        self.commands.insert(name.to_string(), found.clone());
        found
    }

    pub fn clear(&mut self) {
        self.commands.clear();
    }

    pub fn entries(&self) -> impl Iterator<Item = (&String, &PathBuf)> {
        self.commands
            .iter()
            .filter_map(|(name, path)| path.as_ref().map(|path| (name, path)))
    }

    fn refresh_generation(&mut self, path: &str, cwd: &Path) {
        if self.path == path && self.cwd == cwd {
            return;
        }
        self.path = path.to_string();
        self.cwd = cwd.to_path_buf();
        self.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::PathCache;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn caches_positive_lookup_until_cleared() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("meow");
        fs::write(&exe, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.path().to_string_lossy().to_string();
        let cwd = std::env::current_dir().unwrap();
        let mut cache = PathCache::default();

        assert_eq!(cache.resolve("meow", Some(&path), &cwd), Some(exe.clone()));
        fs::remove_file(&exe).unwrap();
        assert_eq!(cache.resolve("meow", Some(&path), &cwd), Some(exe));

        cache.clear();
        assert_eq!(cache.resolve("meow", Some(&path), &cwd), None);
    }

    #[test]
    fn changing_path_invalidates_cache() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        let first_exe = first.path().join("meow");
        let second_exe = second.path().join("meow");
        for exe in [&first_exe, &second_exe] {
            fs::write(exe, "#!/bin/sh\nexit 0\n").unwrap();
            fs::set_permissions(exe, fs::Permissions::from_mode(0o755)).unwrap();
        }
        let first_path = first.path().to_string_lossy().to_string();
        let second_path = second.path().to_string_lossy().to_string();
        let cwd = std::env::current_dir().unwrap();
        let mut cache = PathCache::default();

        assert_eq!(
            cache.resolve("meow", Some(&first_path), &cwd),
            Some(first_exe)
        );
        assert_eq!(
            cache.resolve("meow", Some(&second_path), &cwd),
            Some(second_exe)
        );
    }
}
