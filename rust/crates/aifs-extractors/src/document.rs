//! Bounded readers for PDF, Office, and plain-text documents.

use aifs_domain::{
    Confidence, EntryKind, Evidence, EvidenceSource, FileFamily, LockState, ObservedEntry,
    evidence::keys,
};
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;

const MAX_DOCUMENT_BYTES: u64 = 8 * 1024 * 1024;
const MAX_ZIP_ENTRY_BYTES: u64 = 2 * 1024 * 1024;
const MAX_TEXT_CHARS: usize = 16_384;
const MAX_TITLE_CHARS: usize = 200;
const MAX_DESCRIPTION_CHARS: usize = 280;

/// Reads document text/properties for one observed file.
pub fn extract_document_entry(root: &Path, entry: &ObservedEntry) -> Option<Evidence> {
    if entry.kind != EntryKind::File {
        return None;
    }
    if !matches!(
        entry.family,
        FileFamily::Document
            | FileFamily::Spreadsheet
            | FileFamily::Presentation
            | FileFamily::Ebook
    ) {
        return None;
    }
    if matches!(entry.lock, LockState::Locked { .. }) {
        return None;
    }
    let extension = entry.extension()?;
    let path = entry.path.resolve(root);
    let fields = read_document_fields(&path, &extension)?;
    let mut evidence = Evidence::new(
        entry.id,
        EvidenceSource::DocumentMetadata,
        Confidence::CERTAIN,
    );
    if let Some(title) = &fields.title {
        evidence = evidence.with_fact(keys::DOCUMENT_TITLE, title.clone());
        evidence = evidence.with_fact(keys::DESCRIPTION, truncate(title, MAX_DESCRIPTION_CHARS));
    }
    if let Some(text) = &fields.text {
        evidence = evidence.with_fact(keys::DOCUMENT_TEXT, text.clone());
        if evidence.fact(keys::DESCRIPTION).is_none() {
            evidence = evidence.with_fact(keys::DESCRIPTION, truncate(text, MAX_DESCRIPTION_CHARS));
        }
    }
    if evidence.is_empty() {
        None
    } else {
        Some(evidence)
    }
}

#[derive(Default)]
struct DocumentFields {
    title: Option<String>,
    text: Option<String>,
}

fn read_document_fields(path: &Path, extension: &str) -> Option<DocumentFields> {
    match extension {
        "txt" | "md" | "rst" | "tex" | "csv" | "tsv" => read_plain_text(path),
        "html" | "htm" => read_html(path),
        "pdf" => read_pdf(path),
        "docx" => read_office_zip(path, &["word/document.xml"], Some("docProps/core.xml"), "t"),
        "odt" | "ods" | "odp" => read_office_zip(path, &["content.xml"], Some("meta.xml"), "p"),
        "xlsx" => read_office_zip(
            path,
            &["xl/sharedStrings.xml"],
            Some("docProps/core.xml"),
            "t",
        ),
        "pptx" => read_pptx(path),
        "epub" => read_epub(path),
        _ => None,
    }
}

fn read_capped_file(path: &Path) -> io::Result<Vec<u8>> {
    let file = File::open(path)?;
    let mut buf = Vec::new();
    file.take(MAX_DOCUMENT_BYTES).read_to_end(&mut buf)?;
    Ok(buf)
}

fn read_plain_text(path: &Path) -> Option<DocumentFields> {
    let bytes = read_capped_file(path).ok()?;
    if looks_binary(&bytes) {
        return None;
    }
    let text = String::from_utf8_lossy(&bytes);
    let text = normalize_text(&text);
    if text.is_empty() {
        None
    } else {
        Some(DocumentFields {
            title: first_line(&text),
            text: Some(truncate(&text, MAX_TEXT_CHARS)),
        })
    }
}

fn read_html(path: &Path) -> Option<DocumentFields> {
    let bytes = read_capped_file(path).ok()?;
    if looks_binary(&bytes) {
        return None;
    }
    let raw = String::from_utf8_lossy(&bytes);
    let title = html_title(&raw);
    let text = normalize_text(&strip_markup(&raw));
    if title.is_none() && text.is_empty() {
        return None;
    }
    Some(DocumentFields {
        title,
        text: if text.is_empty() {
            None
        } else {
            Some(truncate(&text, MAX_TEXT_CHARS))
        },
    })
}

fn read_pdf(path: &Path) -> Option<DocumentFields> {
    let bytes = read_capped_file(path).ok()?;
    if !bytes.starts_with(b"%PDF") {
        return None;
    }
    let title = pdf_info_title(&bytes).and_then(|value| sanitize(&value));
    let mut text = String::new();
    for stream in pdf_streams(&bytes) {
        let decoded = decode_pdf_stream(stream);
        collect_pdf_strings(&decoded, &mut text);
        if text.len() >= MAX_TEXT_CHARS {
            break;
        }
    }
    let text = normalize_text(&text);
    if title.is_none() && text.is_empty() {
        return None;
    }
    Some(DocumentFields {
        title,
        text: if text.is_empty() {
            None
        } else {
            Some(truncate(&text, MAX_TEXT_CHARS))
        },
    })
}

fn read_office_zip(
    path: &Path,
    text_members: &[&str],
    meta_member: Option<&str>,
    text_tag: &str,
) -> Option<DocumentFields> {
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let title = meta_member.and_then(|name| zip_xml_text(&mut archive, name, "title"));
    let mut text = String::new();
    for name in text_members {
        if let Some(chunk) = zip_xml_text(&mut archive, name, text_tag) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&chunk);
        }
        if text.len() >= MAX_TEXT_CHARS {
            break;
        }
    }
    let text = normalize_text(&text);
    if title.is_none() && text.is_empty() {
        return None;
    }
    Some(DocumentFields {
        title: title.map(|value| truncate(&value, MAX_TITLE_CHARS)),
        text: if text.is_empty() {
            None
        } else {
            Some(truncate(&text, MAX_TEXT_CHARS))
        },
    })
}

fn read_pptx(path: &Path) -> Option<DocumentFields> {
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let title = zip_xml_text(&mut archive, "docProps/core.xml", "title");
    let mut names: Vec<String> = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_owned())
        })
        .filter(|name| name.starts_with("ppt/slides/slide") && name.ends_with(".xml"))
        .collect();
    names.sort();
    let mut text = String::new();
    for name in names {
        if let Some(chunk) = zip_xml_text(&mut archive, &name, "t") {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&chunk);
        }
        if text.len() >= MAX_TEXT_CHARS {
            break;
        }
    }
    let text = normalize_text(&text);
    if title.is_none() && text.is_empty() {
        return None;
    }
    Some(DocumentFields {
        title: title.map(|value| truncate(&value, MAX_TITLE_CHARS)),
        text: if text.is_empty() {
            None
        } else {
            Some(truncate(&text, MAX_TEXT_CHARS))
        },
    })
}

fn read_epub(path: &Path) -> Option<DocumentFields> {
    let file = File::open(path).ok()?;
    let mut archive = zip::ZipArchive::new(file).ok()?;
    let title = zip_xml_text(&mut archive, "OEBPS/content.opf", "title")
        .or_else(|| zip_xml_text(&mut archive, "EPUB/content.opf", "title"));
    let mut names: Vec<String> = (0..archive.len())
        .filter_map(|index| {
            archive
                .by_index(index)
                .ok()
                .map(|entry| entry.name().to_owned())
        })
        .filter(|name| {
            let lower = name.to_ascii_lowercase();
            lower.ends_with(".xhtml") || lower.ends_with(".html") || lower.ends_with(".htm")
        })
        .collect();
    names.sort();
    let mut text = String::new();
    for name in names {
        if let Some(raw) = zip_member_string(&mut archive, &name) {
            let chunk = normalize_text(&strip_markup(&raw));
            if chunk.is_empty() {
                continue;
            }
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&chunk);
        }
        if text.len() >= MAX_TEXT_CHARS {
            break;
        }
    }
    let text = normalize_text(&text);
    if title.is_none() && text.is_empty() {
        return None;
    }
    Some(DocumentFields {
        title: title.map(|value| truncate(&value, MAX_TITLE_CHARS)),
        text: if text.is_empty() {
            None
        } else {
            Some(truncate(&text, MAX_TEXT_CHARS))
        },
    })
}

fn zip_xml_text(
    archive: &mut zip::ZipArchive<File>,
    name: &str,
    local_tag: &str,
) -> Option<String> {
    let xml = zip_member_string(archive, name)?;
    let text = xml_local_text(&xml, local_tag);
    if text.is_empty() { None } else { Some(text) }
}

fn zip_member_string(archive: &mut zip::ZipArchive<File>, name: &str) -> Option<String> {
    let entry = archive.by_name(name).ok()?;
    if entry.size() > MAX_ZIP_ENTRY_BYTES {
        return None;
    }
    let mut buf = Vec::new();
    entry.take(MAX_ZIP_ENTRY_BYTES).read_to_end(&mut buf).ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

fn xml_local_text(xml: &str, local_tag: &str) -> String {
    let mut out = String::new();
    let mut rest = xml;
    let mut depth = 0usize;
    while let Some(open) = rest.find('<') {
        if depth > 0 {
            let text = &rest[..open];
            push_xml_text(&mut out, text);
        }
        rest = &rest[open + 1..];
        if rest.starts_with('?') || rest.starts_with('!') {
            rest = rest.split_once('>').map(|(_, tail)| tail).unwrap_or("");
            continue;
        }
        let closing = rest.starts_with('/');
        if closing {
            rest = &rest[1..];
        }
        let name_end = rest
            .find(|ch: char| ch.is_whitespace() || ch == '>' || ch == '/')
            .unwrap_or(rest.len());
        let qname = &rest[..name_end];
        let local = qname.rsplit(':').next().unwrap_or(qname);
        let after_name = &rest[name_end..];
        let self_closing = after_name.trim_start().starts_with("/>")
            || after_name
                .split_once('>')
                .is_some_and(|(attrs, _)| attrs.trim_end().ends_with('/'));
        rest = after_name
            .split_once('>')
            .map(|(_, tail)| tail)
            .unwrap_or("");
        if local != local_tag {
            continue;
        }
        if closing {
            depth = depth.saturating_sub(1);
            if !out.is_empty() && !out.ends_with('\n') {
                out.push('\n');
            }
        } else if !self_closing {
            depth += 1;
        }
        if out.len() >= MAX_TEXT_CHARS {
            break;
        }
    }
    normalize_text(&out)
}

fn push_xml_text(out: &mut String, raw: &str) {
    let decoded = decode_basic_entities(raw);
    let trimmed = decoded.trim();
    if trimmed.is_empty() {
        return;
    }
    if !out.is_empty() && !out.ends_with(['\n', ' ']) {
        out.push(' ');
    }
    out.push_str(trimmed);
}

fn pdf_streams(bytes: &[u8]) -> Vec<&[u8]> {
    let mut bodies = Vec::new();
    let mut offset = 0usize;
    while let Some(rel) = find_bytes(&bytes[offset..], b"stream") {
        let mut start = offset + rel + b"stream".len();
        if bytes.get(start) == Some(&b'\r') {
            start += 1;
        }
        if bytes.get(start) == Some(&b'\n') {
            start += 1;
        }
        let Some(end_rel) = find_bytes(&bytes[start..], b"endstream") else {
            break;
        };
        bodies.push(&bytes[start..start + end_rel]);
        offset = start + end_rel + b"endstream".len();
    }
    bodies
}

fn decode_pdf_stream(body: &[u8]) -> Vec<u8> {
    let trimmed = trim_pdf_stream(body);
    if let Ok(decoded) = inflate(trimmed)
        && !decoded.is_empty()
    {
        return decoded;
    }
    trimmed.to_vec()
}

fn trim_pdf_stream(body: &[u8]) -> &[u8] {
    let mut start = 0usize;
    let mut end = body.len();
    while start < end && matches!(body[start], b'\r' | b'\n' | b' ') {
        start += 1;
    }
    while end > start && matches!(body[end - 1], b'\r' | b'\n' | b' ') {
        end -= 1;
    }
    &body[start..end]
}

fn inflate(bytes: &[u8]) -> Result<Vec<u8>, io::Error> {
    let decoder = flate2::read::ZlibDecoder::new(bytes);
    let mut out = Vec::new();
    decoder.take(MAX_ZIP_ENTRY_BYTES).read_to_end(&mut out)?;
    Ok(out)
}

fn collect_pdf_strings(bytes: &[u8], out: &mut String) {
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'(' {
            let (text, next) = decode_pdf_literal(bytes, index);
            if let Some(text) = sanitize(&text) {
                if !out.is_empty() && !out.ends_with(['\n', ' ']) {
                    out.push(' ');
                }
                out.push_str(&text);
            }
            index = next;
            continue;
        }
        index += 1;
        if out.len() >= MAX_TEXT_CHARS {
            break;
        }
    }
}

fn decode_pdf_literal(bytes: &[u8], start: usize) -> (String, usize) {
    let mut index = start + 1;
    let mut depth = 1i32;
    let mut raw = Vec::new();
    while index < bytes.len() && depth > 0 {
        let byte = bytes[index];
        if byte == b'\\' && index + 1 < bytes.len() {
            index += 1;
            match bytes[index] {
                b'n' => raw.push(b'\n'),
                b'r' => raw.push(b'\r'),
                b't' => raw.push(b'\t'),
                b'(' | b')' | b'\\' => raw.push(bytes[index]),
                other if other.is_ascii_digit() => {
                    let mut value = u32::from(other - b'0');
                    let mut consumed = 1;
                    while consumed < 3
                        && index + consumed < bytes.len()
                        && bytes[index + consumed].is_ascii_digit()
                    {
                        value = value * 8 + u32::from(bytes[index + consumed] - b'0');
                        consumed += 1;
                    }
                    raw.push(value as u8);
                    index += consumed - 1;
                }
                other => raw.push(other),
            }
        } else if byte == b'(' {
            depth += 1;
            raw.push(byte);
        } else if byte == b')' {
            depth -= 1;
            if depth > 0 {
                raw.push(byte);
            }
        } else {
            raw.push(byte);
        }
        index += 1;
    }
    (String::from_utf8_lossy(&raw).into_owned(), index)
}

fn pdf_info_title(bytes: &[u8]) -> Option<String> {
    let marker = b"/Title";
    let rel = find_bytes(bytes, marker)?;
    let mut index = rel + marker.len();
    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
        index += 1;
    }
    if bytes.get(index) == Some(&b'(') {
        return Some(decode_pdf_literal(bytes, index).0);
    }
    None
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn looks_binary(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(512)];
    if sample.contains(&0) {
        return true;
    }
    let controls = sample
        .iter()
        .filter(|byte| byte.is_ascii_control() && !matches!(**byte, b'\n' | b'\r' | b'\t'))
        .count();
    controls > sample.len() / 8
}

fn html_title(raw: &str) -> Option<String> {
    let lower = raw.to_ascii_lowercase();
    let start = lower.find("<title>")? + "<title>".len();
    let end = lower[start..].find("</title>")? + start;
    sanitize(&strip_markup(&raw[start..end])).map(|value| truncate(&value, MAX_TITLE_CHARS))
}

fn strip_markup(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        match ch {
            '<' => in_tag = true,
            '>' => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(ch),
            _ => {}
        }
    }
    decode_basic_entities(&out)
}

fn decode_basic_entities(input: &str) -> String {
    input
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
}

fn first_line(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| truncate(line, MAX_TITLE_CHARS))
}

fn normalize_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut last_space = true;
    for ch in value.chars() {
        if ch == '\0' {
            continue;
        }
        if ch == '\n' || ch == '\r' {
            if !out.ends_with('\n') && !out.is_empty() {
                out.push('\n');
            }
            last_space = true;
            continue;
        }
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
                last_space = true;
            }
            continue;
        }
        out.push(ch);
        last_space = false;
    }
    out.trim().to_owned()
}

fn sanitize(value: &str) -> Option<String> {
    let normalized = normalize_text(value);
    if normalized.is_empty() {
        None
    } else {
        Some(normalized)
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_owned();
    }
    value.chars().take(max_chars).collect()
}

/// Writes a tiny uncompressed PDF with a title and a visible string.
pub fn write_pdf_fixture(path: &Path, title: &str, body: &str) -> io::Result<()> {
    let stream = format!("BT /F1 12 Tf 72 720 Td ({body}) Tj ET\n");
    let contents = format!(
        "<< /Length {} >>\nstream\n{stream}endstream\n",
        stream.len()
    );
    let pdf = format!(
        "%PDF-1.4\n\
1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n\
2 0 obj\n<< /Type /Pages /Count 1 /Kids [3 0 R] >>\nendobj\n\
3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
/Resources << /Font << /F1 5 0 R >> >> >>\nendobj\n\
4 0 obj\n{contents}endobj\n\
5 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n\
6 0 obj\n<< /Title ({title}) >>\nendobj\n\
trailer\n<< /Root 1 0 R /Info 6 0 R >>\n%%EOF\n"
    );
    std::fs::write(path, pdf)
}

/// Writes a tiny DOCX with core title and document body text.
pub fn write_docx_fixture(path: &Path, title: &str, body: &str) -> io::Result<()> {
    let file = File::create(path)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("[Content_Types].xml", options)?;
    zip.write_all(
        br#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://purl.oclc.org/ooxml/officeDocument/relationships"></Types>"#,
    )?;
    zip.start_file("docProps/core.xml", options)?;
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>{title}</dc:title></cp:coreProperties>"#
        )
        .as_bytes(),
    )?;
    zip.start_file("word/document.xml", options)?;
    zip.write_all(
        format!(
            r#"<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>{body}</w:t></w:r></w:p></w:body></w:document>"#
        )
        .as_bytes(),
    )?;
    zip.finish()?;
    Ok(())
}

/// Writes a UTF-8 text file for tests.
pub fn write_plain_text_fixture(path: &Path, text: &str) -> io::Result<()> {
    std::fs::write(path, text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use aifs_domain::{AssetId, FileIdentity, ObservedEntry, RelativePath};

    fn doc_entry(name: &str, family: FileFamily) -> ObservedEntry {
        ObservedEntry {
            id: AssetId::new(),
            path: RelativePath::parse(name).unwrap_or_else(|error| panic!("{error}")),
            kind: EntryKind::File,
            family,
            identity: FileIdentity::default(),
            is_hidden: false,
            lock: LockState::Readable,
        }
    }

    #[test]
    fn plain_text_uses_first_line_as_title() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        write_plain_text_fixture(
            dir.path().join("notes.txt").as_path(),
            "Quarterly\nBody line",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let entry = doc_entry("notes.txt", FileFamily::Document);
        let evidence =
            extract_document_entry(dir.path(), &entry).unwrap_or_else(|| panic!("text evidence"));
        assert_eq!(evidence.fact(keys::DOCUMENT_TITLE), Some("Quarterly"));
        assert_eq!(
            evidence.fact(keys::DOCUMENT_TEXT),
            Some("Quarterly\nBody line")
        );
    }

    #[test]
    fn pdf_extracts_title_and_visible_string() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        write_pdf_fixture(
            dir.path().join("brief.pdf").as_path(),
            "Q2 Brief",
            "Hello PDF",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let entry = doc_entry("brief.pdf", FileFamily::Document);
        let evidence =
            extract_document_entry(dir.path(), &entry).unwrap_or_else(|| panic!("pdf evidence"));
        assert_eq!(evidence.fact(keys::DOCUMENT_TITLE), Some("Q2 Brief"));
        assert!(
            evidence
                .fact(keys::DOCUMENT_TEXT)
                .is_some_and(|text| text.contains("Hello PDF")),
            "{evidence:?}"
        );
    }

    #[test]
    fn docx_extracts_core_title_and_body() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        write_docx_fixture(
            dir.path().join("memo.docx").as_path(),
            "Staff Memo",
            "Please file this.",
        )
        .unwrap_or_else(|error| panic!("{error}"));
        let entry = doc_entry("memo.docx", FileFamily::Document);
        let evidence =
            extract_document_entry(dir.path(), &entry).unwrap_or_else(|| panic!("docx evidence"));
        assert_eq!(evidence.fact(keys::DOCUMENT_TITLE), Some("Staff Memo"));
        assert!(
            evidence
                .fact(keys::DOCUMENT_TEXT)
                .is_some_and(|text| text.contains("Please file this.")),
            "{evidence:?}"
        );
        assert!(
            evidence
                .fact(keys::DESCRIPTION)
                .is_some_and(|text| text.contains("Staff Memo"))
        );
    }

    #[test]
    fn locked_documents_are_skipped() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let mut entry = doc_entry("locked.txt", FileFamily::Document);
        entry.lock = LockState::Locked {
            reason: "busy".into(),
        };
        assert!(extract_document_entry(dir.path(), &entry).is_none());
    }
}
