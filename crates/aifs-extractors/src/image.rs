//! Bounded EXIF readers for still images.

use aifs_domain::{
    Confidence, EntryKind, Evidence, EvidenceSource, FileFamily, LockState, ObservedEntry,
    evidence::keys,
};
use std::fs::File;
use std::io::{self, Cursor, Read};
use std::path::Path;

/// EXIF APP1 lives near the start of JPEG/HEIC; RAW TIFF strings farther in may be missed.
const MAX_EXIF_BYTES: u64 = 2 * 1024 * 1024;

/// Reads capture date and camera for one observed image.
pub fn extract_image_entry(root: &Path, entry: &ObservedEntry) -> Option<Evidence> {
    if entry.kind != EntryKind::File {
        return None;
    }
    if !matches!(entry.family, FileFamily::Image | FileFamily::RawImage) {
        return None;
    }
    if matches!(entry.lock, LockState::Locked { .. }) {
        return None;
    }
    let path = entry.path.resolve(root);
    let fields = read_exif_fields(&path)?;
    let mut evidence = Evidence::new(entry.id, EvidenceSource::Exif, Confidence::CERTAIN);
    if let Some(captured_on) = fields.captured_on {
        evidence = evidence.with_fact(keys::IMAGE_CAPTURED_ON, captured_on);
    }
    if let Some(camera) = fields.camera {
        evidence = evidence.with_fact(keys::IMAGE_CAMERA, camera);
    }
    if let Some(latitude) = fields.latitude {
        evidence = evidence.with_fact(keys::IMAGE_LATITUDE, latitude);
    }
    if let Some(longitude) = fields.longitude {
        evidence = evidence.with_fact(keys::IMAGE_LONGITUDE, longitude);
    }
    if evidence.is_empty() {
        None
    } else {
        Some(evidence)
    }
}

#[derive(Default)]
struct ExifFields {
    captured_on: Option<String>,
    camera: Option<String>,
    latitude: Option<String>,
    longitude: Option<String>,
}

fn read_exif_fields(path: &Path) -> Option<ExifFields> {
    let file = File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_EXIF_BYTES).read_to_end(&mut bytes).ok()?;
    let mut cursor = Cursor::new(bytes);
    let exif = exif::Reader::new().read_from_container(&mut cursor).ok()?;
    let captured_on = field_display(&exif, exif::Tag::DateTimeOriginal)
        .or_else(|| field_display(&exif, exif::Tag::DateTime))
        .and_then(|value| normalize_captured_on(&value));
    let make = field_display(&exif, exif::Tag::Make);
    let model = field_display(&exif, exif::Tag::Model);
    let camera = match (make, model) {
        (Some(make), Some(model)) if model.starts_with(&make) => Some(model),
        (Some(make), Some(model)) => Some(format!("{make} {model}")),
        (Some(make), None) => Some(make),
        (None, Some(model)) => Some(model),
        (None, None) => None,
    };
    Some(ExifFields {
        captured_on,
        camera,
        latitude: gps_coord(&exif, exif::Tag::GPSLatitude, exif::Tag::GPSLatitudeRef),
        longitude: gps_coord(&exif, exif::Tag::GPSLongitude, exif::Tag::GPSLongitudeRef),
    })
}

fn field_display(exif: &exif::Exif, tag: exif::Tag) -> Option<String> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    let value = field.display_value().to_string();
    sanitize_exif(value.trim().trim_matches('"'))
}

fn sanitize_exif(value: &str) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    for ch in value.chars() {
        if ch == '\0' {
            continue;
        }
        if ch.is_control() {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    let trimmed = out.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

fn normalize_captured_on(value: &str) -> Option<String> {
    let digits: String = value.chars().filter(|ch| ch.is_ascii_digit()).collect();
    if digits.len() < 8 {
        return None;
    }
    let year: u32 = digits[0..4].parse().ok()?;
    let month: u32 = digits[4..6].parse().ok()?;
    let day: u32 = digits[6..8].parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1970 {
        return None;
    }
    Some(format!("{year:04}-{month:02}-{day:02}"))
}

fn gps_coord(exif: &exif::Exif, tag: exif::Tag, ref_tag: exif::Tag) -> Option<String> {
    let field = exif.get_field(tag, exif::In::PRIMARY)?;
    let hemi = field_display(exif, ref_tag).unwrap_or_default();
    let degrees = match &field.value {
        exif::Value::Rational(values) if values.len() >= 3 => {
            let deg = values[0].to_f64();
            let min = values[1].to_f64();
            let sec = values[2].to_f64();
            deg + min / 60.0 + sec / 3600.0
        }
        _ => return None,
    };
    let signed = if matches!(hemi.as_str(), "S" | "W") {
        -degrees
    } else {
        degrees
    };
    Some(format!("{signed:.6}"))
}

/// Writes a tiny JPEG with Make, Model, and DateTimeOriginal.
pub fn write_jpeg_exif_fixture(
    path: &Path,
    make: &str,
    model: &str,
    captured_on: &str,
) -> io::Result<()> {
    let tiff = build_exif_tiff(make, model, captured_on);
    let mut app1 = Vec::from(*b"Exif\0\0");
    app1.extend_from_slice(&tiff);
    let app1_len = (app1.len() + 2) as u16;
    let mut jpeg = vec![0xff, 0xd8, 0xff, 0xe1];
    jpeg.extend_from_slice(&app1_len.to_be_bytes());
    jpeg.extend(app1);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    std::fs::write(path, jpeg)
}

fn build_exif_tiff(make: &str, model: &str, captured_on: &str) -> Vec<u8> {
    let make_bytes = c_string(make);
    let model_bytes = c_string(model);
    let date_bytes = c_string(captured_on);
    let ifd0_offset = 8u32;
    let ifd0_size = 2 + 3 * 12 + 4;
    let exif_ifd_offset = ifd0_offset + ifd0_size;
    let exif_ifd_size = 2 + 12 + 4;
    let strings_offset = exif_ifd_offset + exif_ifd_size;
    let make_off = strings_offset;
    let model_off = make_off + make_bytes.len() as u32;
    let date_off = model_off + model_bytes.len() as u32;

    let mut out = Vec::new();
    out.extend_from_slice(b"II");
    out.extend_from_slice(&42u16.to_le_bytes());
    out.extend_from_slice(&ifd0_offset.to_le_bytes());

    out.extend_from_slice(&3u16.to_le_bytes());
    push_ascii_entry(&mut out, 0x010f, make_bytes.len() as u32, make_off);
    push_ascii_entry(&mut out, 0x0110, model_bytes.len() as u32, model_off);
    push_long_entry(&mut out, 0x8769, exif_ifd_offset);
    out.extend_from_slice(&0u32.to_le_bytes());

    out.extend_from_slice(&1u16.to_le_bytes());
    push_ascii_entry(&mut out, 0x9003, date_bytes.len() as u32, date_off);
    out.extend_from_slice(&0u32.to_le_bytes());

    out.extend_from_slice(&make_bytes);
    out.extend_from_slice(&model_bytes);
    out.extend_from_slice(&date_bytes);
    out
}

fn c_string(value: &str) -> Vec<u8> {
    let mut bytes = value.as_bytes().to_vec();
    bytes.push(0);
    bytes
}

fn push_ascii_entry(out: &mut Vec<u8>, tag: u16, count: u32, offset: u32) {
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&offset.to_le_bytes());
}

fn push_long_entry(out: &mut Vec<u8>, tag: u16, value: u32) {
    out.extend_from_slice(&tag.to_le_bytes());
    out.extend_from_slice(&4u16.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{AssetId, FileIdentity, ObservedEntry, RelativePath};

    fn image_entry(name: &str) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(name).unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::File,
            family: FileFamily::Image,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    #[test]
    fn jpeg_exif_date_and_camera() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        write_jpeg_exif_fixture(
            dir.path().join("shot.jpg").as_path(),
            "Canon",
            "EOS R5",
            "2021:07:15 09:30:00",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let entry = image_entry("shot.jpg");
        let evidence =
            extract_image_entry(dir.path(), &entry).unwrap_or_else(|| panic!("exif evidence"));
        assert_eq!(evidence.fact(keys::IMAGE_CAPTURED_ON), Some("2021-07-15"));
        assert_eq!(evidence.fact(keys::IMAGE_CAMERA), Some("Canon EOS R5"));
    }

    #[test]
    fn locked_images_are_skipped() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let mut entry = image_entry("locked.jpg");
        entry.lock = LockState::Locked {
            reason: "busy".into(),
        };
        assert!(extract_image_entry(dir.path(), &entry).is_none());
    }
}
