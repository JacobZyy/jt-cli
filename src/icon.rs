use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

enum Content {
    Png(&'static [u8]),
    Svg(&'static str),
}

struct Icon {
    file_name: &'static str,
    content: Content,
}

pub fn download(selector: &OsStr, directory: &Path) -> Result<PathBuf, String> {
    let icon = select(selector)?;
    fs::create_dir_all(directory)
        .map_err(|error| format!("cannot create {}: {error}", directory.display()))?;

    let path = directory.join(icon.file_name);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::AlreadyExists {
                format!(
                    "{} already exists; refusing to overwrite it",
                    path.display()
                )
            } else {
                format!("cannot create {}: {error}", path.display())
            }
        })?;

    let result = match icon.content {
        Content::Png(source) => file.write_all(source),
        Content::Svg(source) => file.write_all(source.as_bytes()),
    }
    .and_then(|()| file.sync_all());

    if let Err(error) = result {
        drop(file);
        let _ = fs::remove_file(&path);
        return Err(format!("cannot write {}: {error}", path.display()));
    }

    Ok(path)
}

fn select(selector: &OsStr) -> Result<Icon, String> {
    let icon = match selector.to_str() {
        Some("16") => png("jt-16.png", include_bytes!("../jt-icon/png/jt-16.png")),
        Some("24") => png("jt-24.png", include_bytes!("../jt-icon/png/jt-24.png")),
        Some("32") => png("jt-32.png", include_bytes!("../jt-icon/png/jt-32.png")),
        Some("48") => png("jt-48.png", include_bytes!("../jt-icon/png/jt-48.png")),
        Some("64") => png("jt-64.png", include_bytes!("../jt-icon/png/jt-64.png")),
        Some("128") => png("jt-128.png", include_bytes!("../jt-icon/png/jt-128.png")),
        Some("256") => png("jt-256.png", include_bytes!("../jt-icon/png/jt-256.png")),
        Some("512") => png("jt-512.png", include_bytes!("../jt-icon/png/jt-512.png")),
        Some("1024") => png("jt-1024.png", include_bytes!("../jt-icon/png/jt-1024.png")),
        Some("svg") => Icon {
            file_name: "jt.svg",
            content: Content::Svg(include_str!("../jt-icon/jt.svg")),
        },
        Some("animated") => Icon {
            file_name: "jt-animated.svg",
            content: Content::Svg(include_str!("../jt-icon/jt-animated.svg")),
        },
        _ => {
            return Err(format!(
                "unsupported icon {}; expected svg, animated, or PNG size 16, 24, 32, 48, 64, 128, 256, 512, or 1024",
                selector.to_string_lossy()
            ));
        }
    };

    Ok(icon)
}

fn png(file_name: &'static str, source: &'static [u8]) -> Icon {
    Icon {
        file_name,
        content: Content::Png(source),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[test]
    fn writes_png_and_svg_sources() {
        let directory = TestDirectory::new();
        let output = directory.path.join("public");

        let png_path = download(OsStr::new("64"), &output).unwrap();
        assert_eq!(png_path, output.join("jt-64.png"));
        assert_eq!(
            fs::read(png_path).unwrap(),
            include_bytes!("../jt-icon/png/jt-64.png")
        );

        let svg_path = download(OsStr::new("svg"), &output).unwrap();
        assert_eq!(svg_path, output.join("jt.svg"));
        assert_eq!(
            fs::read_to_string(svg_path).unwrap(),
            include_str!("../jt-icon/jt.svg")
        );

        let animated_path = download(OsStr::new("animated"), &output).unwrap();
        assert_eq!(animated_path, output.join("jt-animated.svg"));
        assert_eq!(
            fs::read_to_string(animated_path).unwrap(),
            include_str!("../jt-icon/jt-animated.svg")
        );
    }

    #[test]
    fn refuses_to_overwrite_existing_icon() {
        let directory = TestDirectory::new();
        let output = directory.path.join("public");
        fs::create_dir(&output).unwrap();
        let path = output.join("jt-64.png");
        fs::write(&path, b"custom icon").unwrap();

        assert!(
            download(OsStr::new("64"), &output)
                .unwrap_err()
                .contains("refusing")
        );
        assert_eq!(fs::read(path).unwrap(), b"custom icon");
    }

    #[test]
    fn rejects_unknown_selector_without_creating_directory() {
        let directory = TestDirectory::new();
        let output = directory.path.join("public");

        assert!(download(OsStr::new("63"), &output).is_err());
        assert!(!output.exists());
    }

    struct TestDirectory {
        path: PathBuf,
    }

    impl TestDirectory {
        fn new() -> Self {
            static NEXT: AtomicUsize = AtomicUsize::new(0);
            let path = std::env::temp_dir().join(format!(
                "jt-icon-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self { path }
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.path).unwrap();
        }
    }
}
