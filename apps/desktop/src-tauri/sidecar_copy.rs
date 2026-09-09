// Copy sidecar binaries only when contents change so `tauri dev` does not loop.

/// Copies `src` to `dest` when the destination is missing or differs. Returns true if written.
fn copy_if_changed(src: &std::path::Path, dest: &std::path::Path) -> std::io::Result<bool> {
    if dest.is_file() && files_have_same_contents(src, dest) {
        return Ok(false);
    }
    std::fs::copy(src, dest)?;
    Ok(true)
}

/// Writes `contents` when the destination is missing or differs. Returns true if written.
fn write_if_changed(dest: &std::path::Path, contents: &[u8]) -> std::io::Result<bool> {
    if dest.is_file()
        && let Ok(existing) = std::fs::read(dest)
        && existing == contents
    {
        return Ok(false);
    }
    std::fs::write(dest, contents)?;
    Ok(true)
}

fn files_have_same_contents(src: &std::path::Path, dest: &std::path::Path) -> bool {
    let Ok(src_meta) = std::fs::metadata(src) else {
        return false;
    };
    let Ok(dest_meta) = std::fs::metadata(dest) else {
        return false;
    };
    if src_meta.len() != dest_meta.len() {
        return false;
    }
    let Ok(mut src_file) = std::fs::File::open(src) else {
        return false;
    };
    let Ok(mut dest_file) = std::fs::File::open(dest) else {
        return false;
    };
    let mut src_buf = [0_u8; 8192];
    let mut dest_buf = [0_u8; 8192];
    loop {
        let Ok(src_n) = std::io::Read::read(&mut src_file, &mut src_buf) else {
            return false;
        };
        let Ok(dest_n) = std::io::Read::read(&mut dest_file, &mut dest_buf) else {
            return false;
        };
        if src_n != dest_n || src_buf[..src_n] != dest_buf[..dest_n] {
            return false;
        }
        if src_n == 0 {
            return true;
        }
    }
}

#[cfg(test)]
mod sidecar_copy_tests {
    use super::{copy_if_changed, files_have_same_contents, write_if_changed};
    use std::fs;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    fn temp_dir() -> PathBuf {
        let n = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aifs-sidecar-copy-{}-{n}", std::process::id()));
        fs::create_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
        dir
    }

    #[test]
    fn copy_if_changed_skips_identical_bytes() {
        let dir = temp_dir();
        let src = dir.join("src.bin");
        let dest = dir.join("dest.bin");
        fs::write(&src, b"sidecar").unwrap_or_else(|error| panic!("{error}"));
        fs::write(&dest, b"sidecar").unwrap_or_else(|error| panic!("{error}"));
        let before = fs::metadata(&dest)
            .unwrap_or_else(|error| panic!("{error}"))
            .modified()
            .unwrap_or_else(|error| panic!("{error}"));
        assert!(!copy_if_changed(&src, &dest).unwrap_or_else(|error| panic!("{error}")));
        let after = fs::metadata(&dest)
            .unwrap_or_else(|error| panic!("{error}"))
            .modified()
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(before, after);
        fs::remove_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn copy_if_changed_replaces_different_bytes() {
        let dir = temp_dir();
        let src = dir.join("src.bin");
        let dest = dir.join("dest.bin");
        fs::write(&src, b"new").unwrap_or_else(|error| panic!("{error}"));
        fs::write(&dest, b"old").unwrap_or_else(|error| panic!("{error}"));
        assert!(copy_if_changed(&src, &dest).unwrap_or_else(|error| panic!("{error}")));
        assert_eq!(
            fs::read(&dest).unwrap_or_else(|error| panic!("{error}")),
            b"new"
        );
        fs::remove_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
    }

    #[test]
    fn write_if_changed_skips_matching_placeholder() {
        let dir = temp_dir();
        let dest = dir.join("empty.bin");
        fs::write(&dest, []).unwrap_or_else(|error| panic!("{error}"));
        assert!(!write_if_changed(&dest, &[]).unwrap_or_else(|error| panic!("{error}")));
        assert!(files_have_same_contents(&dest, &dest));
        fs::remove_dir_all(&dir).unwrap_or_else(|error| panic!("{error}"));
    }
}
