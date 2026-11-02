use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Serialize, de::DeserializeOwned};

const MAX_BYTES: usize = 32 * 1024 * 1024;

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

fn read(path: &Path) -> Result<Option<Vec<u8>>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    ensure!(
        file.metadata()?.len() <= MAX_BYTES as u64,
        "Snapshot exceeds 32 MiB"
    );
    let mut bytes = Vec::new();
    file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= MAX_BYTES, "Snapshot exceeds 32 MiB");
    Ok(Some(bytes))
}

fn decode<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    read(path)?
        .map(|bytes| serde_json::from_slice(&bytes).context("Invalid snapshot"))
        .transpose()
}

pub fn load<T: DeserializeOwned>(path: &Path) -> Result<Option<T>> {
    match decode(path) {
        Ok(Some(value)) => Ok(Some(value)),
        primary => match decode(&sibling(path, ".bak")) {
            Ok(Some(value)) => Ok(Some(value)),
            Ok(None) => primary,
            Err(backup) => match primary {
                Err(primary) => bail!("Snapshot cannot be read: {primary}; backup: {backup}"),
                _ => Err(backup.context("Snapshot backup cannot be read")),
            },
        },
    }
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let temporary = sibling(path, &format!(".{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(
            path.parent()
                .filter(|p| !p.as_os_str().is_empty())
                .unwrap_or(Path::new(".")),
        )?
        .sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn save<T: Serialize + DeserializeOwned>(path: &Path, value: &T) -> Result<()> {
    let mut encoded = BoundedSnapshot(Vec::new());
    serde_json::to_writer(&mut encoded, value)?;
    let bytes = encoded.0;
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    // A failed launch or truncated snapshot must not replace the recovery copy.
    if let Ok(Some(previous)) = read(path)
        && serde_json::from_slice::<T>(&previous).is_ok()
    {
        atomic_write(&sibling(path, ".bak"), &previous)?;
    }
    atomic_write(path, &bytes)
}

struct BoundedSnapshot(Vec<u8>);

impl Write for BoundedSnapshot {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > MAX_BYTES - self.0.len() {
            return Err(std::io::Error::other("Snapshot exceeds 32 MiB"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct State {
        panes: Vec<String>,
        #[serde(default)]
        version: u32,
    }

    fn state(name: &str) -> State {
        State {
            panes: vec![name.into()],
            version: 1,
        }
    }

    #[test]
    fn saves_and_restores_with_previous_generation_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("nested/state.json");
        assert_eq!(load::<State>(&path).unwrap(), None);
        save(&path, &state("first")).unwrap();
        save(&path, &state("second")).unwrap();
        assert_eq!(load::<State>(&path).unwrap(), Some(state("second")));
        fs::write(&path, b"{partial").unwrap();
        assert_eq!(load::<State>(&path).unwrap(), Some(state("first")));
        save(&path, &state("third")).unwrap();
        assert_eq!(
            decode::<State>(&sibling(&path, ".bak")).unwrap(),
            Some(state("first"))
        );
    }

    #[test]
    fn invalid_schema_does_not_replace_backup_and_old_fields_migrate() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(sibling(&path, ".bak"), br#"{"panes":["old"]}"#).unwrap();
        fs::write(&path, br#"{"panes":false}"#).unwrap();
        let migrated = State {
            panes: vec!["old".into()],
            version: 0,
        };
        assert_eq!(load::<State>(&path).unwrap(), Some(migrated));
        save(&path, &state("new")).unwrap();
        fs::remove_file(&path).unwrap();
        assert_eq!(load::<State>(&path).unwrap().unwrap().version, 0);
    }

    #[test]
    fn corrupt_snapshots_are_reported_and_partial_temps_ignored() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        fs::write(sibling(&path, ".interrupted.tmp"), b"partial").unwrap();
        assert_eq!(load::<State>(&path).unwrap(), None);
        fs::write(&path, b"partial").unwrap();
        fs::write(sibling(&path, ".bak"), b"partial").unwrap();
        assert!(load::<State>(&path).is_err());
    }

    #[test]
    fn oversized_snapshots_are_rejected_without_replacing_saved_state() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        save(&path, &state("safe")).unwrap();
        assert!(save(&path, &state(&"x".repeat(MAX_BYTES))).is_err());
        assert_eq!(load::<State>(&path).unwrap(), Some(state("safe")));
        File::create(&path)
            .unwrap()
            .set_len(MAX_BYTES as u64 + 1)
            .unwrap();
        assert!(load::<State>(&path).is_err());
    }

    #[test]
    fn serialization_stops_at_budget_before_materializing_the_whole_value() {
        use serde::ser::SerializeSeq;
        use std::cell::Cell;

        struct LargeSequence<'a>(&'a Cell<usize>);
        impl Serialize for LargeSequence<'_> {
            fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                let block = "x".repeat(1024 * 1024);
                let mut sequence = serializer.serialize_seq(Some(1024))?;
                for index in 0..1024 {
                    self.0.set(index + 1);
                    sequence.serialize_element(&block)?;
                }
                sequence.end()
            }
        }

        let visited = Cell::new(0);
        let mut encoded = BoundedSnapshot(Vec::new());
        let error = serde_json::to_writer(&mut encoded, &LargeSequence(&visited)).unwrap_err();
        assert!(error.to_string().contains("Snapshot exceeds 32 MiB"));
        assert!(encoded.0.len() <= MAX_BYTES);
        assert_eq!(visited.get(), 32);
    }

    #[cfg(unix)]
    #[test]
    fn output_files_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("state.json");
        save(&path, &state("first")).unwrap();
        save(&path, &state("second")).unwrap();
        for file in [&path, &sibling(&path, ".bak")] {
            assert_eq!(
                fs::metadata(file).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
