//! `.allowed` companion-file ACL (NomadNet 1.4.1 `request_allowed` parity).

use std::path::Path;

use rns_identity::identity::Identity;

use crate::cgi::{is_unix_executable, run_allowlist_script};
use crate::paths::reject_if_symlink;

/// Outcome of an identity allowlist check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AclDecision {
    /// No companion, or remote identity hash is listed.
    Allow,
    /// Companion exists and remote identity is missing/anonymous/not listed,
    /// or the resource path itself ends with `.allowed`.
    Deny,
}

/// Enforce NomadNet `{resource}.allowed` rules for `resource_path`.
///
/// - Paths ending in `.allowed` (case-insensitive) are always denied.
/// - Missing companion → allow.
/// - Static companion: lines of exactly 32 hex chars (whitespace trimmed) are
///   16-byte identity hashes; allow iff `remote_identity.hash` is listed.
/// - Executable companion: when `allow_executable` is true (Unix), run under
///   the CGI sandbox and parse stdout as the list; otherwise read as a static
///   file even if the execute bit is set.
pub fn request_allowed(
    resource_path: &Path,
    remote_identity: Option<&Identity>,
    allow_executable: bool,
) -> AclDecision {
    let path_str = resource_path.to_string_lossy();
    if path_str.to_ascii_lowercase().ends_with(".allowed") {
        return AclDecision::Deny;
    }

    let allowed_path = {
        let mut p = resource_path.as_os_str().to_owned();
        p.push(".allowed");
        std::path::PathBuf::from(p)
    };

    if !allowed_path.is_file() {
        return AclDecision::Allow;
    }

    let allowed_bytes = match read_allowed_bytes(&allowed_path, allow_executable) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %allowed_path.display(),
                "failed to read .allowed companion; denying"
            );
            return AclDecision::Deny;
        }
    };

    let allowed_list = parse_allowed_hashes(&allowed_bytes);
    match remote_identity {
        Some(id) if allowed_list.iter().any(|h| h == &id.hash) => AclDecision::Allow,
        _ => AclDecision::Deny,
    }
}

fn read_allowed_bytes(allowed_path: &Path, allow_executable: bool) -> std::io::Result<Vec<u8>> {
    if let Err(e) = reject_if_symlink(allowed_path) {
        return Err(std::io::Error::other(e.to_string()));
    }

    #[cfg(unix)]
    {
        if allow_executable && is_unix_executable(allowed_path) {
            return run_allowlist_script(allowed_path)
                .map_err(|e| std::io::Error::other(format!("executable .allowed failed: {e}")));
        }
    }
    #[cfg(not(unix))]
    {
        let _ = allow_executable;
    }

    std::fs::read(allowed_path)
}

/// Parse allowlist lines: trim whitespace; keep lines that are exactly 32 hex chars.
pub fn parse_allowed_hashes(input: &[u8]) -> Vec<[u8; 16]> {
    let mut out = Vec::new();
    for line in input.split(|b| *b == b'\n' || *b == b'\r') {
        let trimmed = trim_ascii_whitespace(line);
        if trimmed.len() != 32 {
            continue;
        }
        if !trimmed.iter().all(u8::is_ascii_hexdigit) {
            continue;
        }
        let Ok(s) = std::str::from_utf8(trimmed) else {
            continue;
        };
        let mut hash = [0u8; 16];
        if hex::decode_to_slice(s, &mut hash).is_ok() {
            out.push(hash);
        }
    }
    out
}

fn trim_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let start = bytes
        .iter()
        .position(|b| !b.is_ascii_whitespace())
        .unwrap_or(bytes.len());
    let end = bytes
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map(|i| i + 1)
        .unwrap_or(start);
    &bytes[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::tempdir;

    fn identity_with_hash(hash: [u8; 16]) -> Identity {
        let mut id = Identity::new();
        // Tests only: override truncated hash used by ACL matching.
        id.hash = hash;
        id
    }

    #[test]
    fn missing_companion_allows() {
        let dir = tempdir().unwrap();
        let resource = dir.path().join("page.mu");
        std::fs::write(&resource, b"> hi\n").unwrap();
        assert_eq!(request_allowed(&resource, None, false), AclDecision::Allow);
    }

    #[test]
    fn path_ending_allowed_denies() {
        let dir = tempdir().unwrap();
        let resource = dir.path().join("page.mu.allowed");
        std::fs::write(&resource, b"deadbeef").unwrap();
        assert_eq!(request_allowed(&resource, None, false), AclDecision::Deny);
    }

    #[test]
    fn static_list_requires_listed_identity() {
        let dir = tempdir().unwrap();
        let resource = dir.path().join("secret.mu");
        std::fs::write(&resource, b"> secret\n").unwrap();
        let hash = [0xab; 16];
        let allowed = dir.path().join("secret.mu.allowed");
        {
            let mut f = std::fs::File::create(&allowed).unwrap();
            writeln!(f, "  {}  ", hex::encode(hash)).unwrap();
            writeln!(f, "not-a-hash").unwrap();
            writeln!(f, "zzzz").unwrap();
        }
        assert_eq!(request_allowed(&resource, None, false), AclDecision::Deny);
        assert_eq!(
            request_allowed(&resource, Some(&identity_with_hash([0x11; 16])), false),
            AclDecision::Deny
        );
        assert_eq!(
            request_allowed(&resource, Some(&identity_with_hash(hash)), false),
            AclDecision::Allow
        );
    }

    #[test]
    fn parse_allowed_hashes_trims_and_filters() {
        let input = b"  aabbccddeeff00112233445566778899  \n\
                      short\n\
                      AABBCCDDEEFF00112233445566778899\n\
                      not-hex-!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!!\n";
        let hashes = parse_allowed_hashes(input);
        assert_eq!(hashes.len(), 2);
        let expected: [u8; 16] = hex::decode("aabbccddeeff00112233445566778899")
            .unwrap()
            .try_into()
            .unwrap();
        assert_eq!(hashes[0], expected);
        assert_eq!(hashes[1], expected);
    }

    #[test]
    #[cfg(unix)]
    fn executable_allowed_reads_as_static_when_cgi_off() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let resource = dir.path().join("x.mu");
        std::fs::write(&resource, b"> x\n").unwrap();
        let hash = [0xcd; 16];
        let allowed = dir.path().join("x.mu.allowed");
        std::fs::write(&allowed, format!("{}\n", hex::encode(hash))).unwrap();
        let mut perms = std::fs::metadata(&allowed).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&allowed, perms).unwrap();
        assert_eq!(
            request_allowed(&resource, Some(&identity_with_hash(hash)), false),
            AclDecision::Allow
        );
    }
}
