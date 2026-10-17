//! The data directory and its persistent volume id.

use std::io::{self, Write};
use std::path::Path;

use uuid::Uuid;

/// File under `DATA_DIR` holding the volume id.
pub const VOLUME_ID_FILE: &str = ".volume-id";

/// Prepare `data_dir` and return its volume id.
///
/// Creates `data_dir` and `data_dir/blobs` if needed, then reads
/// `data_dir/.volume-id`. If that file is absent, a fresh UUIDv4 is written to
/// a temporary file and renamed into place, so a crash never leaves a partial
/// id behind. The id is stable for as long as the directory persists, which
/// lets `/health` reveal a redeploy that lost its volume.
///
/// An existing file that does not hold a UUID is an error rather than being
/// overwritten: it means `DATA_DIR` points somewhere unexpected.
pub fn init_data_dir(data_dir: &Path) -> io::Result<String> {
    std::fs::create_dir_all(data_dir.join("blobs"))?;

    let path = data_dir.join(VOLUME_ID_FILE);
    match std::fs::read_to_string(&path) {
        Ok(s) => {
            let id = Uuid::parse_str(s.trim()).map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{} is not a UUID: {e}", path.display()),
                )
            })?;
            return Ok(id.to_string());
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }

    let id = Uuid::new_v4().to_string();
    let tmp = data_dir.join(format!("{VOLUME_ID_FILE}.tmp-{}", Uuid::new_v4()));
    let write = || -> io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        writeln!(f, "{id}")?;
        f.sync_all()?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(e) = write() {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_layout_and_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("nested/data");
        let a = init_data_dir(&root).unwrap();
        assert!(root.join("blobs").is_dir());
        assert!(Uuid::parse_str(&a).is_ok());
        assert_eq!(init_data_dir(&root).unwrap(), a);
        // Only the id file and blobs/ remain; no temp files.
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 2);
    }

    #[test]
    fn rejects_garbage_id() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(VOLUME_ID_FILE), "not-a-uuid").unwrap();
        let err = init_data_dir(dir.path()).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
    }
}
