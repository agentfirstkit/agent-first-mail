use crate::error::{AppError, Result};
use atomic_write_file::AtomicWriteFile;
use std::io::Write as _;
use std::path::Path;

/// Serialize `value` as pretty JSON with a trailing newline and write it to
/// `path`, creating parent directories as needed.
pub(crate) fn write_json_pretty<T: serde::Serialize>(path: &Path, value: &T) -> Result<()> {
    let data =
        serde_json::to_string_pretty(value).map_err(|e| AppError::json("serialize json", &e))?;
    write_bytes_atomic(path, (data + "\n").as_bytes(), "write json")
}

pub(crate) fn write_string_atomic(path: &Path, data: &str) -> Result<()> {
    write_bytes_atomic(path, data.as_bytes(), "write file")
}

pub(crate) fn write_bytes_atomic(path: &Path, data: &[u8], context: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| AppError::io("create directory", &e))?;
    }
    let mut file = AtomicWriteFile::open(path).map_err(|e| AppError::io(context, &e))?;
    file.write_all(data)
        .map_err(|e| AppError::io(context, &e))?;
    file.commit().map_err(|e| AppError::io(context, &e))
}

pub(crate) fn sha256_fingerprint(bytes: &[u8]) -> String {
    let digest = ring::digest::digest(&ring::digest::SHA256, bytes);
    let mut out = String::with_capacity("sha256:".len() + digest.as_ref().len() * 2);
    out.push_str("sha256:");
    for byte in digest.as_ref() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

pub(crate) fn file_sha256_fingerprint(path: &Path, context: &str) -> Result<String> {
    let bytes = std::fs::read(path).map_err(|e| AppError::io(context, &e))?;
    Ok(sha256_fingerprint(&bytes))
}

/// Normalize an IMAP flag set: drop the volatile `\Recent` flag, then sort and
/// de-duplicate so stored flags compare equal regardless of server order.
pub(crate) fn canonical_flags(flags: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut flags = flags
        .into_iter()
        .filter(|flag| !flag.eq_ignore_ascii_case("\\Recent"))
        .collect::<Vec<_>>();
    flags.sort();
    flags.dedup();
    flags
}
