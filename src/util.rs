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

/// True if `host` is a loopback address or local hostname, where plaintext
/// (no-TLS) credentials never leave the machine. Used to allow plaintext against
/// local test servers (e.g. GreenMail on 127.0.0.1) while refusing to send
/// credentials in cleartext to a real remote host.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    let host = host.trim().trim_matches(['[', ']']);
    if host.eq_ignore_ascii_case("localhost") || host.eq_ignore_ascii_case("localhost.localdomain")
    {
        return true;
    }
    match host.parse::<std::net::IpAddr>() {
        Ok(addr) => addr.is_loopback(),
        Err(_) => false,
    }
}

/// Refuse to transmit credentials over an unencrypted connection to a
/// non-loopback host. `protocol` is "IMAP"/"SMTP" for the error message.
pub(crate) fn ensure_plaintext_credentials_allowed(
    protocol: &str,
    host: &str,
    has_credentials: bool,
) -> Result<()> {
    if has_credentials && !is_loopback_host(host) {
        return Err(AppError::new(
            "insecure_transport",
            format!(
                "refusing to send {protocol} credentials in cleartext to non-loopback host {host:?}; \
                 enable TLS (imap.tls / smtp.starttls or smtp.tls_wrapper) or use a loopback host for local testing"
            ),
        ));
    }
    Ok(())
}

pub(crate) fn blake3_fingerprint(bytes: &[u8]) -> String {
    format!("blake3:{}", blake3::hash(bytes).to_hex())
}

pub(crate) fn blake3_fingerprint_str(value: &str) -> String {
    blake3_fingerprint(value.as_bytes())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_hosts_are_recognized() {
        for host in [
            "127.0.0.1",
            "::1",
            "[::1]",
            "localhost",
            "LocalHost",
            " 127.0.0.1 ",
        ] {
            assert!(is_loopback_host(host), "{host} should be loopback");
        }
        for host in ["imap.example.com", "10.0.0.5", "192.168.1.2", "", "example"] {
            assert!(!is_loopback_host(host), "{host} should not be loopback");
        }
    }

    #[test]
    fn plaintext_credentials_blocked_only_for_remote_hosts() {
        // Loopback: allowed even with credentials (local test servers).
        assert!(ensure_plaintext_credentials_allowed("IMAP", "127.0.0.1", true).is_ok());
        assert!(ensure_plaintext_credentials_allowed("SMTP", "localhost", true).is_ok());
        // Remote host with no credentials: nothing to leak, allowed.
        assert!(ensure_plaintext_credentials_allowed("IMAP", "imap.example.com", false).is_ok());
        // Remote host with credentials over plaintext: refused.
        let refused = ensure_plaintext_credentials_allowed("IMAP", "imap.example.com", true);
        assert!(
            refused.is_err(),
            "plaintext credentials to a remote host must be refused"
        );
        assert_eq!(
            refused.err().map(|err| err.error_code),
            Some("insecure_transport")
        );
    }
}
