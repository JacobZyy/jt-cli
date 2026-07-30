use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    path::{Path, PathBuf},
};

use crate::node::error::{AppError, Result};

#[derive(Clone, Debug)]
pub struct HomePaths {
    pub home: PathBuf,
    pub temp_root: PathBuf,
}

impl HomePaths {
    pub fn from_environment(environment: &BTreeMap<OsString, OsString>) -> Result<Self> {
        let home = value(environment, "HOME")
            .map(PathBuf::from)
            .ok_or_else(|| AppError::Invalid("HOME is required".to_owned()))?;
        if !home.is_absolute() {
            return Err(AppError::UnsafePath(format!(
                "HOME must be an absolute path: {}",
                home.display()
            )));
        }
        let home = home
            .canonicalize()
            .map_err(|error| AppError::io("resolve HOME", Some(home), error))?;
        if home.parent().is_none() || !home.is_dir() {
            return Err(AppError::UnsafePath(format!(
                "HOME must be an existing non-root directory: {}",
                home.display()
            )));
        }
        Ok(Self {
            home,
            temp_root: std::env::temp_dir(),
        })
    }

    pub fn vite_plus_home(&self) -> PathBuf {
        self.home.join(".vite-plus")
    }
}

pub fn value(environment: &BTreeMap<OsString, OsString>, key: &str) -> Option<String> {
    environment
        .get(&OsString::from(key))
        .map(|value| value.to_string_lossy().into_owned())
}

pub fn path_entries(environment: &BTreeMap<OsString, OsString>) -> Vec<PathBuf> {
    value(environment, "PATH")
        .map(|path| std::env::split_paths(&OsString::from(path)).collect())
        .unwrap_or_default()
}

pub fn executable_candidates(
    name: &str,
    environment: &BTreeMap<OsString, OsString>,
) -> Vec<PathBuf> {
    path_entries(environment)
        .into_iter()
        .map(|entry| entry.join(name))
        .filter(|candidate| is_regular_or_symlink_file(candidate))
        .collect()
}

pub fn first_executable(name: &str, environment: &BTreeMap<OsString, OsString>) -> Option<PathBuf> {
    executable_candidates(name, environment).into_iter().next()
}

pub fn is_regular_or_symlink_file(path: &Path) -> bool {
    fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_file() || metadata.file_type().is_symlink())
        .unwrap_or(false)
}

pub fn supported_platform() -> Result<()> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("macos" | "linux", "x86_64" | "aarch64") => Ok(()),
        (os, arch) => Err(AppError::Invalid(format!(
            "unsupported platform before mutation: {os}/{arch}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, ffi::OsString, fs};

    use tempfile::tempdir;

    use super::{HomePaths, executable_candidates};

    #[test]
    fn finds_every_path_candidate_not_only_the_first() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        fs::write(first.path().join("pnpm"), "").unwrap();
        fs::write(second.path().join("pnpm"), "").unwrap();
        let path = std::env::join_paths([first.path(), second.path()]).unwrap();
        let environment = BTreeMap::from([(OsString::from("PATH"), path)]);

        assert_eq!(executable_candidates("pnpm", &environment).len(), 2);
    }

    #[test]
    fn home_must_be_absolute_existing_and_non_root() {
        let relative = BTreeMap::from([(OsString::from("HOME"), OsString::from("relative"))]);
        let root = BTreeMap::from([(OsString::from("HOME"), OsString::from("/"))]);
        let valid = tempdir().unwrap();
        let valid_environment = BTreeMap::from([(
            OsString::from("HOME"),
            valid.path().as_os_str().to_os_string(),
        )]);

        assert!(HomePaths::from_environment(&relative).is_err());
        assert!(HomePaths::from_environment(&root).is_err());
        assert_eq!(
            HomePaths::from_environment(&valid_environment)
                .unwrap()
                .home,
            valid.path().canonicalize().unwrap()
        );
    }
}
