//! When local describe may attach image pixels (never for hosted HTTP).

use aifs_domain::FileFamily;
use std::path::Path;

/// JPEG/PNG/WebP describe reads at most this many bytes.
pub const PIXEL_BYTES_CAP: u64 = 8 * 1024 * 1024;

/// Whether llama.cpp should attach a bitmap or stay on filename + EXIF.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelPlan {
    /// Decode JPEG/PNG/WebP through libmtmd.
    Attach,
    /// Text-only describe. `reason` is honest scan-log copy.
    TextOnly {
        /// Why pixels are not sent.
        reason: &'static str,
    },
}

/// Decides pixel attach for `path` given its family and on-disk size.
pub fn pixel_plan(path: &Path, family: FileFamily) -> PixelPlan {
    if family == FileFamily::RawImage {
        return PixelPlan::TextOnly {
            reason: "RAW describe uses EXIF and filename, not pixels",
        };
    }
    if family != FileFamily::Image {
        return PixelPlan::TextOnly {
            reason: "describe pixels only for images",
        };
    }
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if !matches!(ext.as_str(), "jpg" | "jpeg" | "png" | "webp") {
        return PixelPlan::TextOnly {
            reason: "describe pixels only for JPEG, PNG, and WebP",
        };
    }
    let len = std::fs::metadata(path)
        .ok()
        .filter(|meta| meta.is_file())
        .map(|meta| meta.len())
        .unwrap_or(0);
    if len == 0 {
        return PixelPlan::TextOnly {
            reason: "image file is empty or unreadable",
        };
    }
    if len > PIXEL_BYTES_CAP {
        return PixelPlan::TextOnly {
            reason: "image exceeds 8 MiB pixel cap",
        };
    }
    PixelPlan::Attach
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn jpeg_under_cap_attaches_pixels() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = dir.path().join("shot.jpg");
        fs::write(&path, [0_u8; 64]).unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(pixel_plan(&path, FileFamily::Image), PixelPlan::Attach);
    }

    #[test]
    fn webp_and_png_attach_pixels() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        for name in ["a.png", "b.webp"] {
            let path = dir.path().join(name);
            fs::write(&path, b"xxxx").unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(pixel_plan(&path, FileFamily::Image), PixelPlan::Attach);
        }
    }

    #[test]
    fn raw_and_oversize_stay_text_only() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let raw = dir.path().join("DSC.CR2");
        fs::write(&raw, b"raw").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            pixel_plan(&raw, FileFamily::RawImage),
            PixelPlan::TextOnly {
                reason: "RAW describe uses EXIF and filename, not pixels"
            }
        );
        let big = dir.path().join("huge.jpg");
        fs::File::create(&big)
            .unwrap_or_else(|error| panic!("{error}"))
            .set_len(PIXEL_BYTES_CAP + 1)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            pixel_plan(&big, FileFamily::Image),
            PixelPlan::TextOnly {
                reason: "image exceeds 8 MiB pixel cap"
            }
        );
        for name in ["anim.gif", "photo.heic", "scan.tiff", "scan.tif"] {
            let path = dir.path().join(name);
            fs::write(&path, b"xxxx").unwrap_or_else(|error| panic!("{error}"));
            assert_eq!(
                pixel_plan(&path, FileFamily::Image),
                PixelPlan::TextOnly {
                    reason: "describe pixels only for JPEG, PNG, and WebP"
                }
            );
        }
        let empty = dir.path().join("empty.jpg");
        fs::write(&empty, b"").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(
            pixel_plan(&empty, FileFamily::Image),
            PixelPlan::TextOnly {
                reason: "image file is empty or unreadable"
            }
        );
        let missing = dir.path().join("gone.jpg");
        assert_eq!(
            pixel_plan(&missing, FileFamily::Image),
            PixelPlan::TextOnly {
                reason: "image file is empty or unreadable"
            }
        );
        let at_cap = dir.path().join("cap.jpg");
        fs::File::create(&at_cap)
            .unwrap_or_else(|error| panic!("{error}"))
            .set_len(PIXEL_BYTES_CAP)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(pixel_plan(&at_cap, FileFamily::Image), PixelPlan::Attach);
    }
}
