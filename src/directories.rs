use std::path::{Path, PathBuf};

pub fn default_directory() -> PathBuf {
    let current = std::env::current_dir().unwrap_or_default();
    if current.parent().is_none() {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or(current)
    } else {
        current
    }
}

pub fn resolve(input: &str) -> anyhow::Result<PathBuf> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    resolve_from(input, &default_directory(), home.as_deref())
}

fn resolve_from(input: &str, base: &Path, home: Option<&Path>) -> anyhow::Result<PathBuf> {
    let input = input.trim();
    anyhow::ensure!(!input.is_empty(), "Enter a working directory");
    anyhow::ensure!(
        !input.chars().any(char::is_control),
        "Working directory contains control characters"
    );
    let path = if input == "~" || input.starts_with("~/") {
        let home = home.ok_or_else(|| {
            anyhow::anyhow!("Home directory is unavailable; enter an absolute path")
        })?;
        home.join(
            input
                .strip_prefix("~/")
                .unwrap_or("")
                .trim_start_matches('/'),
        )
    } else {
        anyhow::ensure!(
            !input.starts_with('~'),
            "Use ~/ for your home directory, or enter an absolute path"
        );
        let path = PathBuf::from(input);
        if path.is_absolute() {
            path
        } else {
            base.join(path)
        }
    };
    Ok(path)
}

pub fn prepare(input: &str) -> anyhow::Result<(String, bool)> {
    let path = resolve(input)?;
    let directory = path
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Working directory is not valid UTF-8"))?
        .to_owned();
    if path.is_dir() {
        return Ok((directory, false));
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| anyhow::anyhow!(creation_error(&path, &error)))?;
    }
    let created = match std::fs::create_dir(&path) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => false,
        Err(error) => return Err(anyhow::anyhow!(creation_error(&path, &error))),
    };
    Ok((directory, created))
}

fn creation_error(path: &Path, error: &std::io::Error) -> String {
    let guidance = match error.kind() {
        std::io::ErrorKind::ReadOnlyFilesystem => {
            "This location is on a read-only filesystem. Choose a folder in your home directory (~/) or another writable location."
        }
        std::io::ErrorKind::PermissionDenied => {
            "You do not have permission to create a folder here. Choose a folder in your home directory (~/) or another writable location."
        }
        _ => "Check that the parent path is a directory and is writable.",
    };
    format!(
        "Cannot create working directory {}: {guidance} ({error})",
        path.display()
    )
}

pub fn root_alternative(input: &str) -> Option<String> {
    let path = Path::new(input.trim());
    if path.parent() == Some(Path::new("/")) && !path.exists() {
        Some(format!("~/{}", path.file_name()?.to_str()?))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_expands_home_and_preserves_absolute_paths() {
        for (input, expected) in [
            (" ~/projects/Terra ", "/users/test/projects/Terra"),
            ("~", "/users/test"),
            ("~//Terra", "/users/test/Terra"),
            ("Terra", "/workspace/Terra"),
            ("/Terra", "/Terra"),
        ] {
            assert_eq!(
                resolve_from(
                    input,
                    Path::new("/workspace"),
                    Some(Path::new("/users/test"))
                )
                .unwrap(),
                PathBuf::from(expected)
            );
        }
        assert!(resolve_from("~other", Path::new("/workspace"), None).is_err());
        assert!(resolve_from(" ", Path::new("/workspace"), None).is_err());
    }

    #[test]
    fn prepare_creates_nested_directory_and_preserves_existing_files() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("new workspace/nested");
        assert_eq!(
            prepare(folder.to_str().unwrap()).unwrap().0,
            folder.to_str().unwrap()
        );
        assert!(folder.is_dir());
        assert!(!prepare(folder.to_str().unwrap()).unwrap().1);
        let file = root.path().join("file");
        std::fs::write(&file, "keep").unwrap();
        assert!(prepare(file.to_str().unwrap()).is_err());
        assert_eq!(std::fs::read_to_string(file).unwrap(), "keep");
    }

    #[test]
    fn read_only_error_explains_recovery_without_elevation() {
        let message = creation_error(
            Path::new("/Terra"),
            &std::io::Error::from(std::io::ErrorKind::ReadOnlyFilesystem),
        );
        assert!(message.contains("read-only filesystem"));
        assert!(message.contains("home directory"));
        assert_eq!(
            root_alternative("/tessera-missing-workspace"),
            Some("~/tessera-missing-workspace".into())
        );
        assert!(root_alternative("/tmp/existing-parent").is_none());
    }
}
