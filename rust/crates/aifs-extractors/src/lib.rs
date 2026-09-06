//! Bounded readers for media tags, documents, and image EXIF.
//!
//! Media: ID3v1/v2, FLAC Vorbis comments, Ogg Vorbis/Opus comments, and MP4/M4A
//! `ilst` atoms. Documents: PDF strings, Office/EPUB zip members, and plain text.
//! Reads are size-capped so a corrupt file cannot pull a multi-gigabyte payload
//! into memory.

use aifs_domain::{
    Confidence, EntryKind, Evidence, EvidenceSource, FileFamily, LockState, ObservedEntry,
    WorkspaceSnapshot, evidence::keys,
};
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

mod document;
mod image;

pub use document::{
    extract_document_entry, write_docx_fixture, write_pdf_fixture, write_plain_text_fixture,
};
pub use image::{extract_image_entry, write_jpeg_exif_fixture};

const MAX_ID3_TAG_BYTES: u32 = 2 * 1024 * 1024;
const MAX_BLOCK_BYTES: u32 = 2 * 1024 * 1024;
const MAX_MP4_TEXT_BYTES: u64 = 64 * 1024;

/// Fills `snapshot.evidence` with media tags for readable audio/video files.
pub fn extract_into(snapshot: &mut WorkspaceSnapshot) {
    extract_into_with_progress(snapshot, |_, _| {});
}

/// Like [`extract_into`], reporting `(current, total)` entries as each file is visited.
pub fn extract_into_with_progress(
    snapshot: &mut WorkspaceSnapshot,
    mut on_progress: impl FnMut(u64, u64),
) {
    let root = snapshot.root.clone();
    let total = snapshot.entries.len() as u64;
    let mut bags = Vec::new();
    for (index, entry) in snapshot.entries.iter().enumerate() {
        on_progress(index as u64 + 1, total);
        if let Some(evidence) = extract_entry(&root, entry) {
            bags.push(evidence);
        }
    }
    snapshot.evidence.extend(bags);
}

/// Reads tags for a single observed file. Returns `None` when the file is not media,
/// is locked, or has no usable tags.
pub fn extract_entry(root: &Path, entry: &ObservedEntry) -> Option<Evidence> {
    if entry.kind != EntryKind::File {
        return None;
    }
    if !matches!(entry.family, FileFamily::Audio | FileFamily::Video) {
        return None;
    }
    if matches!(entry.lock, LockState::Locked { .. }) {
        return None;
    }
    let path = entry.path.resolve(root);
    let fields = read_media_fields(&path, entry.extension().as_deref()?)?;
    let mut evidence = Evidence::new(entry.id, EvidenceSource::MediaTags, Confidence::CERTAIN);
    if let Some(title) = fields.title {
        evidence = evidence.with_fact(keys::MEDIA_TITLE, title);
    }
    if let Some(artist) = fields.artist {
        evidence = evidence.with_fact(keys::MEDIA_ARTIST, artist);
    }
    if let Some(album) = fields.album {
        evidence = evidence.with_fact(keys::MEDIA_ALBUM, album);
    }
    if let Some(year) = fields.year {
        evidence = evidence.with_fact(keys::MEDIA_YEAR, normalize_year(&year));
    }
    if let Some(genre) = fields.genre {
        evidence = evidence.with_fact(keys::MEDIA_GENRE, genre);
    }
    if let Some(track) = fields.track {
        evidence = evidence.with_fact(keys::MEDIA_TRACK, track);
    }
    if evidence.is_empty() {
        None
    } else {
        Some(evidence)
    }
}

#[derive(Default, Clone)]
struct MediaFields {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    year: Option<String>,
    genre: Option<String>,
    track: Option<String>,
}

impl MediaFields {
    fn has_any(&self) -> bool {
        self.title.is_some() || self.artist.is_some() || self.album.is_some() || self.year.is_some()
    }

    fn assign_if_missing(target: &mut Option<String>, value: Option<String>) {
        if target.is_none()
            && let Some(value) = value
            && !value.is_empty()
        {
            *target = Some(value);
        }
    }
}

fn read_media_fields(path: &Path, extension: &str) -> Option<MediaFields> {
    let ext = extension.to_ascii_lowercase();
    let mut fields = MediaFields::default();
    match ext.as_str() {
        "mp3" => {
            let _ = parse_id3v2(path, &mut fields);
            if !fields.has_any() {
                let _ = parse_id3v1(path, &mut fields);
            }
        }
        "flac" => {
            let _ = parse_flac(path, &mut fields);
        }
        "ogg" | "oga" | "opus" => {
            let _ = parse_ogg(path, &mut fields);
        }
        "m4a" | "mp4" | "m4v" | "mov" | "3gp" => {
            let _ = parse_mp4(path, &mut fields);
        }
        "aac" | "wav" | "wma" | "aif" | "aiff" | "alac" | "ape" | "wv" => {
            let _ = parse_id3v2(path, &mut fields);
        }
        _ => return None,
    }
    if fields.has_any() { Some(fields) } else { None }
}

fn normalize_year(value: &str) -> String {
    let digits: String = value
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .take(4)
        .collect();
    if digits.len() == 4 {
        digits
    } else {
        value.trim().to_owned()
    }
}

fn sanitize(value: &str) -> Option<String> {
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

fn read_exact(file: &mut File, buf: &mut [u8]) -> io::Result<()> {
    file.read_exact(buf)
}

fn read_u32_be(bytes: &[u8]) -> u32 {
    u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_u32_le(bytes: &[u8]) -> u32 {
    u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
}

fn read_u24_be(bytes: &[u8]) -> u32 {
    (u32::from(bytes[0]) << 16) | (u32::from(bytes[1]) << 8) | u32::from(bytes[2])
}

fn read_u64_be(bytes: &[u8]) -> u64 {
    u64::from_be_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ])
}

fn read_synchsafe_u32(bytes: &[u8]) -> u32 {
    (u32::from(bytes[0] & 0x7f) << 21)
        | (u32::from(bytes[1] & 0x7f) << 14)
        | (u32::from(bytes[2] & 0x7f) << 7)
        | u32::from(bytes[3] & 0x7f)
}

fn parse_id3v2(path: &Path, fields: &mut MediaFields) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut header = [0u8; 10];
    read_exact(&mut file, &mut header)?;
    if &header[0..3] != b"ID3" {
        return Ok(false);
    }
    let major = header[3];
    if !(2..=4).contains(&major) {
        return Ok(false);
    }
    let flags = header[5];
    let tag_size = read_synchsafe_u32(&header[6..10]);
    if tag_size == 0 || tag_size > MAX_ID3_TAG_BYTES {
        return Ok(false);
    }
    let mut data = vec![0u8; tag_size as usize];
    read_exact(&mut file, &mut data)?;
    if flags & 0x80 != 0 {
        data = remove_unsynchronization(&data);
    }
    parse_id3v2_frames(&data, major, flags, fields);
    Ok(fields.has_any())
}

fn remove_unsynchronization(input: &[u8]) -> Vec<u8> {
    let mut decoded = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        decoded.push(input[i]);
        if input[i] == 0xff && i + 1 < input.len() && input[i + 1] == 0x00 {
            i += 2;
        } else {
            i += 1;
        }
    }
    decoded
}

fn parse_id3v2_frames(data: &[u8], major: u8, flags: u8, fields: &mut MediaFields) {
    let mut offset = 0usize;
    if flags & 0x40 != 0 && data.len() >= 4 {
        offset = if major == 3 {
            let ext = read_u32_be(&data[0..4]) as usize;
            ext.saturating_add(4)
        } else {
            read_synchsafe_u32(&data[0..4]) as usize
        };
        if offset > data.len() {
            return;
        }
    }
    while offset < data.len() {
        let (frame_id, frame_size, header_size) = if major == 2 {
            if offset + 6 > data.len() {
                break;
            }
            if data[offset..offset + 3] == [0, 0, 0] {
                break;
            }
            let id = data[offset..offset + 3].to_vec();
            let size = read_u24_be(&data[offset + 3..offset + 6]) as usize;
            (id, size, 6usize)
        } else {
            if offset + 10 > data.len() {
                break;
            }
            if data[offset..offset + 4] == [0, 0, 0, 0] {
                break;
            }
            let id = data[offset..offset + 4].to_vec();
            let size = if major == 4 {
                read_synchsafe_u32(&data[offset + 4..offset + 8]) as usize
            } else {
                read_u32_be(&data[offset + 4..offset + 8]) as usize
            };
            (id, size, 10usize)
        };
        if frame_size == 0 || offset + header_size + frame_size > data.len() {
            break;
        }
        let body = &data[offset + header_size..offset + header_size + frame_size];
        if let Ok(id) = std::str::from_utf8(&frame_id) {
            apply_id3_frame(id, body, fields);
        }
        offset += header_size + frame_size;
    }
}

fn apply_id3_frame(frame_id: &str, body: &[u8], fields: &mut MediaFields) {
    let value = decode_id3_text_frame(body);
    match frame_id {
        "TIT2" | "TT2" => MediaFields::assign_if_missing(&mut fields.title, value),
        "TPE1" | "TP1" | "TPE2" | "TP2" => {
            MediaFields::assign_if_missing(&mut fields.artist, value)
        }
        "TALB" | "TAL" => MediaFields::assign_if_missing(&mut fields.album, value),
        "TDRC" | "TYER" | "TYE" | "TDOR" | "TDRL" => {
            MediaFields::assign_if_missing(&mut fields.year, value)
        }
        "TCON" | "TCO" => MediaFields::assign_if_missing(&mut fields.genre, value),
        "TRCK" | "TRK" => MediaFields::assign_if_missing(&mut fields.track, value),
        _ => {}
    }
}

fn decode_id3_text_frame(bytes: &[u8]) -> Option<String> {
    if bytes.len() <= 1 {
        return None;
    }
    let encoding = bytes[0];
    let payload = &bytes[1..];
    let decoded = match encoding {
        0 => payload.iter().map(|&b| b as char).collect::<String>(),
        3 => String::from_utf8_lossy(payload).into_owned(),
        1 | 2 => decode_utf16(payload, encoding == 1),
        _ => return None,
    };
    let cut = decoded.split('\0').next().unwrap_or(&decoded);
    sanitize(cut)
}

fn decode_utf16(payload: &[u8], default_le: bool) -> String {
    let mut little_endian = default_le;
    let mut offset = 0;
    if payload.len() >= 2 {
        if payload[0] == 0xff && payload[1] == 0xfe {
            little_endian = true;
            offset = 2;
        } else if payload[0] == 0xfe && payload[1] == 0xff {
            little_endian = false;
            offset = 2;
        }
    }
    let mut decoded = String::new();
    let mut index = offset;
    while index + 1 < payload.len() {
        let unit = if little_endian {
            u16::from(payload[index]) | (u16::from(payload[index + 1]) << 8)
        } else {
            (u16::from(payload[index]) << 8) | u16::from(payload[index + 1])
        };
        if unit == 0 {
            break;
        }
        match char::from_u32(u32::from(unit)) {
            Some(ch) if !ch.is_control() => decoded.push(ch),
            Some(_) => decoded.push(' '),
            None => decoded.push(' '),
        }
        index += 2;
    }
    decoded
}

fn parse_id3v1(path: &Path, fields: &mut MediaFields) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let len = file.seek(SeekFrom::End(0))?;
    if len < 128 {
        return Ok(false);
    }
    file.seek(SeekFrom::End(-128))?;
    let mut tag = [0u8; 128];
    read_exact(&mut file, &mut tag)?;
    if &tag[0..3] != b"TAG" {
        return Ok(false);
    }
    let field = |offset: usize, length: usize| -> Option<String> {
        sanitize(std::str::from_utf8(&tag[offset..offset + length]).unwrap_or(""))
    };
    MediaFields::assign_if_missing(&mut fields.title, field(3, 30));
    MediaFields::assign_if_missing(&mut fields.artist, field(33, 30));
    MediaFields::assign_if_missing(&mut fields.album, field(63, 30));
    MediaFields::assign_if_missing(&mut fields.year, field(93, 4));
    Ok(fields.has_any())
}

fn parse_vorbis_comment_payload(bytes: &[u8], fields: &mut MediaFields) -> bool {
    if bytes.len() < 8 {
        return false;
    }
    let mut offset = 0usize;
    let Some(vendor_len) = read_u32_at(bytes, &mut offset) else {
        return false;
    };
    let vendor_len = vendor_len as usize;
    if offset + vendor_len > bytes.len() {
        return false;
    }
    offset += vendor_len;
    let Some(comment_count) = read_u32_at(bytes, &mut offset) else {
        return false;
    };
    for _ in 0..comment_count {
        let Some(comment_len) = read_u32_at(bytes, &mut offset) else {
            return false;
        };
        let comment_len = comment_len as usize;
        if offset + comment_len > bytes.len() {
            return false;
        }
        let entry = String::from_utf8_lossy(&bytes[offset..offset + comment_len]);
        offset += comment_len;
        if let Some((key, value)) = entry.split_once('=') {
            assign_vorbis_field(key.trim(), value, fields);
        }
    }
    fields.has_any()
}

fn read_u32_at(bytes: &[u8], offset: &mut usize) -> Option<u32> {
    if *offset + 4 > bytes.len() {
        return None;
    }
    let value = read_u32_le(&bytes[*offset..*offset + 4]);
    *offset += 4;
    Some(value)
}

fn assign_vorbis_field(key: &str, value: &str, fields: &mut MediaFields) {
    let Some(value) = sanitize(value) else {
        return;
    };
    match key.to_ascii_lowercase().as_str() {
        "title" => MediaFields::assign_if_missing(&mut fields.title, Some(value)),
        "artist" | "albumartist" | "album artist" => {
            MediaFields::assign_if_missing(&mut fields.artist, Some(value))
        }
        "album" => MediaFields::assign_if_missing(&mut fields.album, Some(value)),
        "date" | "year" | "originaldate" => {
            MediaFields::assign_if_missing(&mut fields.year, Some(value))
        }
        "genre" => MediaFields::assign_if_missing(&mut fields.genre, Some(value)),
        "tracknumber" | "track" => MediaFields::assign_if_missing(&mut fields.track, Some(value)),
        _ => {}
    }
}

fn parse_flac(path: &Path, fields: &mut MediaFields) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut sig = [0u8; 4];
    read_exact(&mut file, &mut sig)?;
    if &sig != b"fLaC" {
        return Ok(false);
    }
    loop {
        let mut header = [0u8; 4];
        read_exact(&mut file, &mut header)?;
        let is_last = header[0] & 0x80 != 0;
        let block_type = header[0] & 0x7f;
        let length = read_u24_be(&header[1..4]);
        if length > MAX_BLOCK_BYTES {
            return Ok(false);
        }
        if block_type == 4 {
            let mut block = vec![0u8; length as usize];
            read_exact(&mut file, &mut block)?;
            parse_vorbis_comment_payload(&block, fields);
        } else {
            file.seek(SeekFrom::Current(i64::from(length)))?;
        }
        if is_last {
            break;
        }
    }
    Ok(fields.has_any())
}

fn parse_ogg(path: &Path, fields: &mut MediaFields) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let mut codec = OggCodec::Unknown;
    let mut packet = Vec::new();
    let mut packet_index = 0usize;
    loop {
        let mut page_header = [0u8; 27];
        if read_exact(&mut file, &mut page_header).is_err() {
            break;
        }
        if &page_header[0..4] != b"OggS" || page_header[4] != 0 {
            return Ok(false);
        }
        let segment_count = page_header[26] as usize;
        let mut segment_table = vec![0u8; segment_count];
        if segment_count > 0 {
            read_exact(&mut file, &mut segment_table)?;
        }
        let body_size: usize = segment_table.iter().map(|s| *s as usize).sum();
        let mut body = vec![0u8; body_size];
        if body_size > 0 {
            read_exact(&mut file, &mut body)?;
        }
        let mut body_offset = 0usize;
        for segment in segment_table {
            let end = body_offset + segment as usize;
            packet.extend_from_slice(&body[body_offset..end]);
            body_offset = end;
            if segment == 255 {
                continue;
            }
            if packet_index == 0 {
                codec = sniff_ogg_codec(&packet);
                if codec == OggCodec::Unknown {
                    return Ok(false);
                }
            } else if packet_index == 1 {
                match codec {
                    OggCodec::Vorbis
                        if packet.len() >= 7 && packet[0] == 0x03 && &packet[1..7] == b"vorbis" =>
                    {
                        return Ok(parse_vorbis_comment_payload(&packet[7..], fields));
                    }
                    OggCodec::Opus if packet.len() >= 8 && &packet[0..8] == b"OpusTags" => {
                        return Ok(parse_vorbis_comment_payload(&packet[8..], fields));
                    }
                    _ => return Ok(false),
                }
            }
            packet.clear();
            packet_index += 1;
        }
    }
    Ok(fields.has_any())
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum OggCodec {
    Unknown,
    Vorbis,
    Opus,
}

fn sniff_ogg_codec(packet: &[u8]) -> OggCodec {
    if packet.len() >= 7 && packet[0] == 0x01 && &packet[1..7] == b"vorbis" {
        OggCodec::Vorbis
    } else if packet.len() >= 8 && &packet[0..8] == b"OpusHead" {
        OggCodec::Opus
    } else {
        OggCodec::Unknown
    }
}

fn fourcc(bytes: &[u8; 4]) -> u32 {
    read_u32_be(bytes)
}

fn parse_mp4(path: &Path, fields: &mut MediaFields) -> io::Result<bool> {
    let mut file = File::open(path)?;
    let file_size = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    parse_mp4_atoms(&mut file, 0, file_size, None, 0, fields)?;
    Ok(fields.has_any())
}

fn parse_mp4_atoms(
    file: &mut File,
    range_start: u64,
    range_end: u64,
    current_tag: Option<u32>,
    depth: usize,
    fields: &mut MediaFields,
) -> io::Result<()> {
    if depth > 12 || range_start >= range_end {
        return Ok(());
    }
    let mut offset = range_start;
    while offset + 8 <= range_end {
        let mut header = [0u8; 16];
        file.seek(SeekFrom::Start(offset))?;
        file.read_exact(&mut header[..8])?;
        let mut atom_size = u64::from(read_u32_be(&header[0..4]));
        let atom_type = read_u32_be(&header[4..8]);
        let mut header_size = 8u64;
        if atom_size == 1 {
            file.read_exact(&mut header[8..16])?;
            atom_size = read_u64_be(&header[8..16]);
            header_size = 16;
        } else if atom_size == 0 {
            atom_size = range_end - offset;
        }
        if atom_size < header_size {
            return Ok(());
        }
        let atom_end = offset.saturating_add(atom_size);
        if atom_end < offset || atom_end > range_end {
            return Ok(());
        }
        let mut payload_start = offset + header_size;
        let payload_end = atom_end;
        const META: u32 = u32::from_be_bytes(*b"meta");
        const DATA: u32 = u32::from_be_bytes(*b"data");
        const ILST: u32 = u32::from_be_bytes(*b"ilst");
        if atom_type == META {
            if payload_start + 4 > payload_end {
                offset = atom_end;
                continue;
            }
            payload_start += 4;
        }
        if atom_type == DATA
            && let Some(tag) = current_tag
        {
            let payload_size = payload_end - payload_start;
            if (8..=MAX_MP4_TEXT_BYTES).contains(&payload_size) {
                let mut data = vec![0u8; payload_size as usize];
                file.seek(SeekFrom::Start(payload_start))?;
                file.read_exact(&mut data)?;
                if let Some(text) = parse_mp4_data_text(&data) {
                    assign_mp4_field(tag, text, fields);
                }
            }
        }
        let mut descend = false;
        let mut next_tag = current_tag;
        if is_mp4_metadata_key(atom_type) {
            descend = true;
            next_tag = Some(atom_type);
        } else if is_mp4_container(atom_type) {
            descend = true;
            if atom_type == ILST {
                next_tag = None;
            }
        } else if current_tag.is_some() && atom_type != DATA {
            descend = true;
        }
        if descend && payload_start < payload_end {
            parse_mp4_atoms(
                file,
                payload_start,
                payload_end,
                next_tag,
                depth + 1,
                fields,
            )?;
        }
        if atom_size == 0 {
            return Ok(());
        }
        offset = atom_end;
    }
    Ok(())
}

fn is_mp4_container(atom_type: u32) -> bool {
    atom_type == fourcc(b"moov")
        || atom_type == fourcc(b"udta")
        || atom_type == fourcc(b"meta")
        || atom_type == fourcc(b"ilst")
        || atom_type == fourcc(b"trak")
        || atom_type == fourcc(b"mdia")
        || atom_type == fourcc(b"minf")
        || atom_type == fourcc(b"stbl")
        || atom_type == fourcc(b"edts")
        || atom_type == fourcc(b"dinf")
        || atom_type == fourcc(b"mvex")
}

fn is_mp4_metadata_key(atom_type: u32) -> bool {
    atom_type == u32::from_be_bytes([0xa9, b'n', b'a', b'm'])
        || atom_type == u32::from_be_bytes([0xa9, b'A', b'R', b'T'])
        || atom_type == fourcc(b"aART")
        || atom_type == u32::from_be_bytes([0xa9, b'a', b'l', b'b'])
        || atom_type == u32::from_be_bytes([0xa9, b'd', b'a', b'y'])
        || atom_type == fourcc(b"titl")
        || atom_type == fourcc(b"auth")
        || atom_type == fourcc(b"albm")
        || atom_type == fourcc(b"yrrc")
}

fn parse_mp4_data_text(payload: &[u8]) -> Option<String> {
    if payload.len() < 8 {
        return None;
    }
    let marker_a = read_u32_be(&payload[0..4]);
    let marker_b = read_u32_be(&payload[4..8]);
    let mut data_type = marker_a;
    if (marker_a == 0 || marker_a > 32) && matches!(marker_b, 0 | 1 | 2 | 21) {
        data_type = marker_b;
    }
    let text = &payload[8..];
    if text.is_empty() {
        return None;
    }
    let decoded = if data_type == 2 {
        decode_utf16(text, false)
    } else {
        String::from_utf8_lossy(text)
            .split('\0')
            .next()
            .unwrap_or("")
            .to_owned()
    };
    sanitize(&decoded)
}

fn assign_mp4_field(atom_type: u32, value: String, fields: &mut MediaFields) {
    if atom_type == u32::from_be_bytes([0xa9, b'n', b'a', b'm']) || atom_type == fourcc(b"titl") {
        MediaFields::assign_if_missing(&mut fields.title, Some(value));
    } else if atom_type == u32::from_be_bytes([0xa9, b'A', b'R', b'T'])
        || atom_type == fourcc(b"aART")
        || atom_type == fourcc(b"auth")
    {
        MediaFields::assign_if_missing(&mut fields.artist, Some(value));
    } else if atom_type == u32::from_be_bytes([0xa9, b'a', b'l', b'b'])
        || atom_type == fourcc(b"albm")
    {
        MediaFields::assign_if_missing(&mut fields.album, Some(value));
    } else if atom_type == u32::from_be_bytes([0xa9, b'd', b'a', b'y'])
        || atom_type == fourcc(b"yrrc")
    {
        MediaFields::assign_if_missing(&mut fields.year, Some(value));
    }
}

/// Builds a tiny ID3v2.3 file for tests and fixtures.
pub fn write_id3v23_fixture(
    path: &Path,
    title: &str,
    artist: &str,
    album: &str,
    year: &str,
) -> io::Result<()> {
    fn text_frame(id: &[u8; 4], text: &str) -> Vec<u8> {
        let mut body = vec![3u8];
        body.extend(text.as_bytes());
        body.push(0);
        let mut frame = id.to_vec();
        frame.extend((body.len() as u32).to_be_bytes());
        frame.extend([0u8, 0]);
        frame.extend(body);
        frame
    }
    let mut frames = Vec::new();
    frames.extend(text_frame(b"TIT2", title));
    frames.extend(text_frame(b"TPE1", artist));
    frames.extend(text_frame(b"TALB", album));
    frames.extend(text_frame(b"TYER", year));
    let size = frames.len() as u32;
    let synchsafe = [
        ((size >> 21) & 0x7f) as u8,
        ((size >> 14) & 0x7f) as u8,
        ((size >> 7) & 0x7f) as u8,
        (size & 0x7f) as u8,
    ];
    let mut out = b"ID3".to_vec();
    out.extend([3u8, 0, 0]);
    out.extend(synchsafe);
    out.extend(frames);
    std::fs::write(path, out)
}

/// Builds a tiny FLAC with a Vorbis comment block for tests.
#[cfg(test)]
pub fn write_flac_fixture(path: &Path, title: &str, artist: &str) -> io::Result<()> {
    let mut comments = Vec::new();
    let vendor = b"aifs";
    comments.extend((vendor.len() as u32).to_le_bytes());
    comments.extend(vendor);
    let entries = [
        format!("TITLE={title}"),
        format!("ARTIST={artist}"),
        "DATE=2021".to_owned(),
    ];
    comments.extend((entries.len() as u32).to_le_bytes());
    for entry in entries {
        comments.extend((entry.len() as u32).to_le_bytes());
        comments.extend(entry.as_bytes());
    }
    let mut out = b"fLaC".to_vec();
    out.push(0x00);
    out.extend([0u8, 0, 34]);
    out.extend([0u8; 34]);
    out.push(0x84);
    let len = comments.len() as u32;
    out.push(((len >> 16) & 0xff) as u8);
    out.push(((len >> 8) & 0xff) as u8);
    out.push((len & 0xff) as u8);
    out.extend(comments);
    std::fs::write(path, out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{AssetId, FileIdentity, ObservedEntry, RelativePath};

    fn audio_entry(name: &str) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(name).unwrap_or_else(|e| panic!("{e}")),
            kind: EntryKind::File,
            family: FileFamily::Audio,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    #[test]
    fn id3v23_title_artist_album_year() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("show.mp3");
        write_id3v23_fixture(&path, "Night Drive", "Ada", "After Hours", "2019")
            .unwrap_or_else(|e| panic!("{e}"));
        let entry = audio_entry("show.mp3");
        let evidence = extract_entry(dir.path(), &entry).unwrap_or_else(|| panic!("tags"));
        assert_eq!(evidence.fact(keys::MEDIA_TITLE), Some("Night Drive"));
        assert_eq!(evidence.fact(keys::MEDIA_ARTIST), Some("Ada"));
        assert_eq!(evidence.fact(keys::MEDIA_ALBUM), Some("After Hours"));
        assert_eq!(evidence.fact(keys::MEDIA_YEAR), Some("2019"));
    }

    #[test]
    fn flac_vorbis_comments() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let path = dir.path().join("track.flac");
        write_flac_fixture(&path, "River", "Kim").unwrap_or_else(|e| panic!("{e}"));
        let entry = audio_entry("track.flac");
        let evidence = extract_entry(dir.path(), &entry).unwrap_or_else(|| panic!("tags"));
        assert_eq!(evidence.fact(keys::MEDIA_TITLE), Some("River"));
        assert_eq!(evidence.fact(keys::MEDIA_ARTIST), Some("Kim"));
        assert_eq!(evidence.fact(keys::MEDIA_YEAR), Some("2021"));
    }

    #[test]
    fn locked_files_are_skipped() {
        let dir = tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"));
        let mut entry = audio_entry("locked.mp3");
        entry.lock = LockState::Locked {
            reason: "busy".into(),
        };
        assert!(extract_entry(dir.path(), &entry).is_none());
    }
}
