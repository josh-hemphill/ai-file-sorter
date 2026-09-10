//! Lightweight GGUF header/metadata reads. Does not load weights.

use std::fs::File;
use std::io::{Read, Seek};
use std::path::Path;

const MAX_KV: u64 = 4096;
const MAX_STRING: u64 = 4096;
const MAX_SCAN: u64 = 8 * 1024 * 1024;
const MAX_ARRAY: u64 = 1_000_000;

const BLOCK_COUNT_KEYS: &[&str] = &[
    "llama.block_count",
    "gemma3.block_count",
    "gemma2.block_count",
    "llama.layer_count",
    "llama.n_layer",
    "qwen.block_count",
    "qwen2.block_count",
    "general.block_count",
    "block_count",
];

/// Reads transformer block/layer count from GGUF KV metadata when present.
pub fn read_block_count(path: &Path) -> Option<u32> {
    let mut file = File::open(path).ok()?;
    let mut magic = [0_u8; 4];
    file.read_exact(&mut magic).ok()?;
    if &magic != b"GGUF" {
        return None;
    }
    let version = read_u32_le(&mut file)?;
    if !(2..=3).contains(&version) {
        return None;
    }
    let _n_tensors = read_u64_le(&mut file)?;
    let n_kv = read_u64_le(&mut file)?;
    if n_kv > MAX_KV {
        return None;
    }
    for _ in 0..n_kv {
        if file.stream_position().ok()? > MAX_SCAN {
            return None;
        }
        let key = read_string(&mut file)?;
        let ty = read_u32_le(&mut file)?;
        if BLOCK_COUNT_KEYS.iter().any(|candidate| *candidate == key) {
            if let Some(value) = read_positive_count(ty, &mut file) {
                return Some(value);
            }
        } else {
            skip_value(ty, &mut file, 0)?;
        }
    }
    None
}

fn read_positive_count(ty: u32, file: &mut File) -> Option<u32> {
    let value = match ty {
        2 => u32::from(read_u16_le(file)?),
        3 => i16::from_le_bytes(read_bytes(file)?)
            .try_into()
            .ok()
            .filter(|value: &u32| *value > 0)?,
        4 => {
            let value = read_u32_le(file)?;
            (value > 0).then_some(value)?
        }
        5 => read_i32_le(file)?
            .try_into()
            .ok()
            .filter(|value: &u32| *value > 0)?,
        10 => u32::try_from(read_u64_le(file)?)
            .ok()
            .filter(|value| *value > 0)?,
        11 => {
            let value = read_i64_le(file)?;
            u32::try_from(value).ok().filter(|value| *value > 0)?
        }
        _ => {
            skip_value(ty, file, 0)?;
            return None;
        }
    };
    (value < 10_000).then_some(value)
}

fn skip_value(ty: u32, file: &mut File, depth: u8) -> Option<()> {
    if depth > 4 {
        return None;
    }
    match ty {
        0 | 1 | 7 => skip(file, 1),
        2..=3 => skip(file, 2),
        4..=6 => skip(file, 4),
        8 => {
            let _ = read_string(file)?;
            Some(())
        }
        9 => {
            let inner = read_u32_le(file)?;
            let count = read_u64_le(file)?;
            if count > MAX_ARRAY {
                return None;
            }
            for _ in 0..count {
                skip_value(inner, file, depth.saturating_add(1))?;
            }
            Some(())
        }
        10..=12 => skip(file, 8),
        _ => None,
    }
}

fn read_string(file: &mut File) -> Option<String> {
    let len = read_u64_le(file)?;
    if len > MAX_STRING {
        return None;
    }
    let mut buf = vec![0_u8; usize::try_from(len).ok()?];
    file.read_exact(&mut buf).ok()?;
    String::from_utf8(buf).ok()
}

fn skip(file: &mut File, n: u64) -> Option<()> {
    let mut buf = vec![0_u8; usize::try_from(n).ok()?];
    file.read_exact(&mut buf).ok()
}

fn read_bytes<const N: usize>(file: &mut File) -> Option<[u8; N]> {
    let mut buf = [0_u8; N];
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

fn read_u16_le(file: &mut File) -> Option<u16> {
    Some(u16::from_le_bytes(read_bytes(file)?))
}

fn read_u32_le(file: &mut File) -> Option<u32> {
    Some(u32::from_le_bytes(read_bytes(file)?))
}

fn read_i32_le(file: &mut File) -> Option<i32> {
    Some(i32::from_le_bytes(read_bytes(file)?))
}

fn read_u64_le(file: &mut File) -> Option<u64> {
    Some(u64::from_le_bytes(read_bytes(file)?))
}

fn read_i64_le(file: &mut File) -> Option<i64> {
    Some(i64::from_le_bytes(read_bytes(file)?))
}

#[cfg(test)]
pub(crate) fn write_gguf_with_block_count(
    path: &Path,
    key: &str,
    block_count: u32,
) -> std::io::Result<()> {
    use std::io::Write;
    let mut out = Vec::new();
    out.extend_from_slice(b"GGUF");
    out.extend_from_slice(&3_u32.to_le_bytes());
    out.extend_from_slice(&0_u64.to_le_bytes());
    out.extend_from_slice(&1_u64.to_le_bytes());
    let key_bytes = key.as_bytes();
    out.extend_from_slice(&(key_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(key_bytes);
    out.extend_from_slice(&5_u32.to_le_bytes());
    out.extend_from_slice(&(block_count as i32).to_le_bytes());
    let mut file = File::create(path)?;
    file.write_all(&out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_gemma_and_llama_block_count_keys() {
        let dir = tempfile::tempdir().unwrap_or_else(|error| panic!("{error}"));
        let path = dir.path().join("model.gguf");
        write_gguf_with_block_count(&path, "gemma3.block_count", 34)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(read_block_count(&path), Some(34));
        write_gguf_with_block_count(&path, "llama.block_count", 32)
            .unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(read_block_count(&path), Some(32));
        std::fs::write(&path, b"GGUF\x03\x00\x00\x00").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(read_block_count(&path), None);
        std::fs::write(&path, b"<!DOCTYPE html>").unwrap_or_else(|error| panic!("{error}"));
        assert_eq!(read_block_count(&path), None);
    }
}
