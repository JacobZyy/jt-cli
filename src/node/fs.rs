use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use crate::node::error::{AppError, Result};

pub fn read_optional(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(content) => Ok(Some(content)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(AppError::io("read file", Some(path.to_path_buf()), error)),
    }
}

pub fn atomic_write(
    home: &Path,
    path: &Path,
    expected: Option<&[u8]>,
    content: &[u8],
) -> Result<()> {
    if fs::symlink_metadata(path)
        .map(|metadata| metadata.file_type().is_symlink())
        .unwrap_or(false)
    {
        return Err(AppError::UnsafePath(format!(
            "refuse to replace symlinked file: {}",
            path.display()
        )));
    }
    let current = read_optional(path)?;
    if expected != current.as_deref() {
        return Err(AppError::Invalid(format!(
            "refuse to overwrite concurrently changed file: {}",
            path.display()
        )));
    }
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Invalid(format!("file has no parent: {}", path.display())))?;
    ensure_safe_home_write_parent(home, path)?;
    fs::create_dir_all(parent).map_err(|error| {
        AppError::io("create parent directory", Some(parent.to_path_buf()), error)
    })?;
    let mode = current
        .as_ref()
        .and_then(|_| fs::metadata(path).ok())
        .map(|metadata| metadata.permissions());
    let temp = unique_temp_path(path)?;
    let mut file = create_new(&temp)?;
    file.write_all(content)
        .and_then(|_| file.sync_all())
        .map_err(|error| AppError::io("write temporary file", Some(temp.clone()), error))?;
    if let Some(mode) = mode {
        fs::set_permissions(&temp, mode).map_err(|error| {
            AppError::io("preserve file permissions", Some(temp.clone()), error)
        })?;
    }
    fs::rename(&temp, path)
        .map_err(|error| AppError::io("replace file", Some(path.to_path_buf()), error))?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| {
            AppError::io("sync parent directory", Some(parent.to_path_buf()), error)
        })?;
    Ok(())
}

fn ensure_safe_home_write_parent(home: &Path, path: &Path) -> Result<()> {
    let (canonical_home, relative) = checked_home(home, path)?;
    let components = relative.components().collect::<Vec<_>>();
    let mut cursor = home.to_path_buf();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        cursor.push(component.as_os_str());
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::UnsafePath(format!(
                    "refuse to write through symlinked parent: {}",
                    cursor.display()
                )));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => break,
            Err(error) => {
                return Err(AppError::io(
                    "inspect file parent",
                    Some(cursor.clone()),
                    error,
                ));
            }
        }
    }
    let mut existing = path.parent();
    while let Some(candidate) = existing {
        if candidate.exists() {
            let resolved = candidate.canonicalize().map_err(|error| {
                AppError::io("resolve file parent", Some(candidate.to_path_buf()), error)
            })?;
            if !resolved.starts_with(&canonical_home) {
                return Err(AppError::UnsafePath(format!(
                    "file parent resolves outside HOME: {}",
                    resolved.display()
                )));
            }
            return Ok(());
        }
        existing = candidate.parent();
    }
    Err(AppError::Invalid(format!(
        "file has no existing parent: {}",
        path.display()
    )))
}

fn unique_temp_path(path: &Path) -> Result<PathBuf> {
    let file_name = path
        .file_name()
        .ok_or_else(|| AppError::Invalid(format!("file has no name: {}", path.display())))?
        .to_string_lossy();
    let parent = path.parent().expect("checked parent in atomic_write");
    for nonce in 0..1000 {
        let candidate = parent.join(format!(".{file_name}.jt-node-init-{nonce}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::Invalid(format!(
        "cannot create temporary sibling for {}",
        path.display()
    )))
}

fn create_new(path: &Path) -> Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| AppError::io("create temporary file", Some(path.to_path_buf()), error))
}

pub fn ensure_safe_home_target(home: &Path, target: &Path) -> Result<()> {
    let (canonical_home, relative) = checked_home(home, target)?;
    reject_symlink_components(home, relative.components(), true)?;
    let resolved = target.canonicalize().map_err(|error| {
        AppError::io("resolve cleanup target", Some(target.to_path_buf()), error)
    })?;
    if !resolved.starts_with(canonical_home) {
        return Err(AppError::UnsafePath(format!(
            "cleanup target resolves outside HOME: {}",
            resolved.display()
        )));
    }
    Ok(())
}

pub fn remove_dir_all_safe(home: &Path, target: &Path) -> Result<()> {
    if shared_home_directory(home, target) {
        return Err(AppError::UnsafePath(format!(
            "refuse to recursively remove shared directory: {}",
            target.display()
        )));
    }
    ensure_safe_home_target(home, target)?;
    fs::remove_dir_all(target)
        .map_err(|error| AppError::io("remove directory", Some(target.to_path_buf()), error))
}

fn shared_home_directory(home: &Path, target: &Path) -> bool {
    [
        ".local",
        ".local/bin",
        ".cargo",
        ".cargo/bin",
        ".cache",
        "Library",
        "Library/Caches",
        "Library/pnpm/store",
        ".local/share/pnpm/store",
        ".pnpm/store",
    ]
    .iter()
    .map(|relative| home.join(relative))
    .any(|shared| target == shared)
}

pub fn remove_file_safe(home: &Path, target: &Path) -> Result<()> {
    let (canonical_home, relative) = checked_home(home, target)?;
    let metadata = reject_symlink_components(home, relative.components(), false)?;
    if !metadata.file_type().is_symlink() && !metadata.is_file() {
        return Err(AppError::UnsafePath(format!(
            "cleanup file target is not a file: {}",
            target.display()
        )));
    }
    if !metadata.file_type().is_symlink() {
        let resolved = target.canonicalize().map_err(|error| {
            AppError::io("resolve cleanup file", Some(target.to_path_buf()), error)
        })?;
        if !resolved.starts_with(canonical_home) {
            return Err(AppError::UnsafePath(format!(
                "cleanup file resolves outside HOME: {}",
                resolved.display()
            )));
        }
    }
    fs::remove_file(target)
        .map_err(|error| AppError::io("remove file", Some(target.to_path_buf()), error))
}

fn checked_home<'a>(home: &Path, target: &'a Path) -> Result<(PathBuf, &'a Path)> {
    if !target.is_absolute() {
        return Err(AppError::UnsafePath(format!(
            "cleanup target must be absolute: {}",
            target.display()
        )));
    }
    let canonical = home
        .canonicalize()
        .map_err(|error| AppError::io("resolve HOME", Some(home.to_path_buf()), error))?;
    let relative = target.strip_prefix(home).map_err(|_| {
        AppError::UnsafePath(format!(
            "cleanup target is outside HOME: {}",
            target.display()
        ))
    })?;
    if relative.as_os_str().is_empty() {
        return Err(AppError::UnsafePath("refuse to remove HOME".to_owned()));
    }
    Ok((canonical, relative))
}

fn reject_symlink_components(
    home: &Path,
    components: std::path::Components<'_>,
    include_target: bool,
) -> Result<fs::Metadata> {
    let all = components.collect::<Vec<_>>();
    let mut cursor = home.to_path_buf();
    let mut final_metadata = None;
    for (index, component) in all.iter().enumerate() {
        cursor.push(component.as_os_str());
        let metadata = fs::symlink_metadata(&cursor)
            .map_err(|error| AppError::io("inspect cleanup target", Some(cursor.clone()), error))?;
        if metadata.file_type().is_symlink() && (include_target || index + 1 != all.len()) {
            return Err(AppError::UnsafePath(format!(
                "cleanup target has symlink component: {}",
                cursor.display()
            )));
        }
        final_metadata = Some(metadata);
    }
    final_metadata.ok_or_else(|| AppError::UnsafePath("refuse to remove HOME".to_owned()))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{atomic_write, ensure_safe_home_target, remove_dir_all_safe, remove_file_safe};

    #[test]
    fn atomic_write_rejects_changed_content() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config");
        fs::write(&path, "old").unwrap();
        fs::write(&path, "other").unwrap();

        let error = atomic_write(directory.path(), &path, Some(b"old"), b"next").unwrap_err();

        assert!(error.to_string().contains("concurrently changed"));
        assert_eq!(fs::read_to_string(path).unwrap(), "other");
    }

    #[test]
    fn safe_target_rejects_path_outside_home() {
        let home = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let target = outside.path().join("target");
        fs::create_dir(&target).unwrap();

        assert!(ensure_safe_home_target(home.path(), &target).is_err());
    }

    #[test]
    fn recursive_removal_rejects_shared_bin_directory() {
        let home = tempdir().unwrap();
        let shared = home.path().join(".local/bin");
        fs::create_dir_all(&shared).unwrap();

        assert!(remove_dir_all_safe(home.path(), &shared).is_err());
        assert!(shared.is_dir());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_refuses_to_replace_a_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let target = directory.path().join("target");
        let path = directory.path().join("config");
        fs::write(&target, "old").unwrap();
        symlink(&target, &path).unwrap();

        assert!(atomic_write(directory.path(), &path, Some(b"old"), b"next").is_err());
        assert_eq!(fs::read_to_string(target).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_refuses_a_symlinked_parent() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let parent = directory.path().join("config");
        symlink(outside.path(), &parent).unwrap();

        let error =
            atomic_write(directory.path(), &parent.join("settings"), None, b"next").unwrap_err();

        assert!(error.to_string().contains("symlinked parent"));
        assert!(!outside.path().join("settings").exists());
    }

    #[cfg(unix)]
    #[test]
    fn safe_target_rejects_a_symlink_component_even_when_it_points_into_home() {
        use std::os::unix::fs::symlink;

        let home = tempdir().unwrap();
        let real = home.path().join("real");
        fs::create_dir(&real).unwrap();
        fs::create_dir(real.join("target")).unwrap();
        symlink(&real, home.path().join("alias")).unwrap();

        assert!(ensure_safe_home_target(home.path(), &home.path().join("alias/target")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn safe_file_removal_unlinks_a_leaf_symlink_without_following_it() {
        use std::os::unix::fs::symlink;

        let home = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let target = outside.path().join("fnm");
        let launcher = home.path().join(".local/bin/fnm");
        fs::create_dir_all(launcher.parent().unwrap()).unwrap();
        fs::write(&target, "outside tool").unwrap();
        symlink(&target, &launcher).unwrap();

        remove_file_safe(home.path(), &launcher).unwrap();

        assert!(!launcher.exists());
        assert_eq!(fs::read_to_string(target).unwrap(), "outside tool");
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_existing_mode() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let path = directory.path().join("config");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();

        atomic_write(directory.path(), &path, Some(b"old"), b"next").unwrap();

        assert_eq!(
            fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
}
