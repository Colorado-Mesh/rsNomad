//! Opt-in sandboxed CGI for executable pages / `.allowed` scripts (Unix).

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use rns_identity::identity::Identity;

use crate::error::NomadError;

/// Wall-clock timeout for CGI / executable allowlist scripts.
pub const CGI_TIMEOUT: Duration = Duration::from_secs(10);

/// Minimal PATH for sandboxed scripts (never inherit the full parent env).
const SANITIZED_PATH: &str = "/usr/bin:/bin";

/// True when `path` exists and is executable by the current user (Unix `X_OK`).
#[cfg(unix)]
pub fn is_unix_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(meta) => meta.is_file() && (meta.permissions().mode() & 0o111) != 0,
        Err(_) => false,
    }
}

#[cfg(not(unix))]
pub fn is_unix_executable(_path: &Path) -> bool {
    false
}

/// Run an executable page under a cleared environment.
///
/// Sets only `PATH` (sanitized), optional `link_id` / `remote_identity` hex, and
/// `field_*` / `var_*` entries from `fields`. Captures stdout (capped), discards
/// stderr, uses no shell, and enforces [`CGI_TIMEOUT`].
///
/// The child is placed in its own process group so timeouts / completion can
/// signal the whole tree (descendants cannot hold the stdout pipe open).
#[cfg(unix)]
pub fn run_cgi(
    script: &Path,
    link_id: [u8; 16],
    remote_identity: Option<&Identity>,
    fields: &BTreeMap<String, String>,
    max_stdout: usize,
) -> Result<Vec<u8>, NomadError> {
    run_sandboxed(script, link_id, remote_identity, fields, max_stdout)
}

#[cfg(not(unix))]
pub fn run_cgi(
    _script: &Path,
    _link_id: [u8; 16],
    _remote_identity: Option<&Identity>,
    _fields: &BTreeMap<String, String>,
    _max_stdout: usize,
) -> Result<Vec<u8>, NomadError> {
    Err(NomadError::message("CGI is not supported on this platform"))
}

/// Run an executable `.allowed` script; stdout is the allowlist body.
#[cfg(unix)]
pub fn run_allowlist_script(script: &Path) -> Result<Vec<u8>, NomadError> {
    // Allowlists do not receive link/form context — empty fields, zero link id.
    run_sandboxed(script, [0u8; 16], None, &BTreeMap::new(), 64 * 1024)
}

#[cfg(not(unix))]
pub fn run_allowlist_script(_script: &Path) -> Result<Vec<u8>, NomadError> {
    Err(NomadError::message(
        "executable .allowed is not supported on this platform",
    ))
}

#[cfg(unix)]
fn run_sandboxed(
    script: &Path,
    link_id: [u8; 16],
    remote_identity: Option<&Identity>,
    fields: &BTreeMap<String, String>,
    max_stdout: usize,
) -> Result<Vec<u8>, NomadError> {
    use std::os::unix::process::CommandExt;

    if !script.is_file() {
        return Err(NomadError::NotFound(script.display().to_string()));
    }
    if !is_unix_executable(script) {
        return Err(NomadError::message("script is not executable"));
    }

    let mut command = Command::new(script);
    command.env_clear();
    command.env("PATH", SANITIZED_PATH);
    command.env("link_id", hex::encode(link_id));
    if let Some(id) = remote_identity {
        command.env("remote_identity", hex::encode(id.hash));
    }
    for (key, value) in fields {
        if key.starts_with("field_") || key.starts_with("var_") {
            command.env(key, value);
        }
    }
    command.stdin(Stdio::null());
    command.stdout(Stdio::piped());
    command.stderr(Stdio::null());
    // New process group (pgid == child pid) so we can kill descendants on exit/timeout.
    command.process_group(0);

    let mut child = command.spawn().map_err(NomadError::Io)?;
    let child_pid = child.id();
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| NomadError::message("CGI missing stdout pipe"))?;

    let reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let mut limited = (&mut stdout).take(max_stdout.saturating_add(1) as u64);
        limited.read_to_end(&mut buf).map(|_| buf)
    });

    let status = match wait_with_timeout(&mut child, CGI_TIMEOUT) {
        Ok(status) => {
            // Child already reaped by try_wait; kill any descendants still holding pipes.
            terminate_process_group(child_pid);
            status
        }
        Err(e) => {
            terminate_process_group(child_pid);
            let _ = child.kill();
            let _ = child.wait();
            let _ = reader.join();
            return Err(e);
        }
    };

    let buf = reader
        .join()
        .map_err(|_| NomadError::message("CGI stdout reader panicked"))?
        .map_err(NomadError::Io)?;

    if buf.len() > max_stdout {
        return Err(NomadError::TooLarge {
            size: buf.len(),
            max: max_stdout,
        });
    }

    // Exit code is ignored (NomadNet 1.4.1 parity) — still log unusual exits.
    if !status.success() {
        tracing::debug!(
            code = ?status.code(),
            script = %script.display(),
            "CGI exited non-zero; returning captured stdout"
        );
    }
    Ok(buf)
}

/// Signal the child's process group (negative pid), then best-effort reap.
#[cfg(unix)]
fn terminate_process_group(child_pid: u32) {
    let pid = child_pid as i32;
    if pid > 0 {
        // SAFETY: killpg with the child's pgid (set via process_group(0)).
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
}

#[cfg(unix)]
fn wait_with_timeout(
    child: &mut std::process::Child,
    timeout: Duration,
) -> Result<std::process::ExitStatus, NomadError> {
    let start = std::time::Instant::now();
    loop {
        match child.try_wait().map_err(NomadError::Io)? {
            Some(status) => return Ok(status),
            None if start.elapsed() >= timeout => {
                return Err(NomadError::message("CGI timed out"));
            }
            None => std::thread::sleep(Duration::from_millis(20)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    #[cfg(unix)]
    fn cgi_runs_script_with_env_and_caps_stdout() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let script = dir.path().join("page.mu");
        std::fs::write(
            &script,
            b"#!/bin/sh\nprintf '%s' \"$field_q-$remote_identity-$link_id\"\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();

        let mut fields = BTreeMap::new();
        fields.insert("field_q".into(), "hi".into());
        fields.insert("other".into(), "nope".into());
        let id = Identity::new();
        let link = [0x11u8; 16];
        let out = run_cgi(&script, link, Some(&id), &fields, 1024).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert!(text.starts_with("hi-"));
        assert!(text.contains(&hex::encode(id.hash)));
        assert!(text.contains(&hex::encode(link)));
        assert!(!text.contains("nope"));
    }

    #[test]
    #[cfg(unix)]
    fn cgi_rejects_oversized_stdout() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let script = dir.path().join("big.mu");
        std::fs::write(
            &script,
            b"#!/bin/sh\ndd if=/dev/zero bs=1 count=64 2>/dev/null\n",
        )
        .unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        let err = run_cgi(&script, [0u8; 16], None, &BTreeMap::new(), 8).unwrap_err();
        assert!(matches!(err, NomadError::TooLarge { .. }));
    }

    #[test]
    #[cfg(unix)]
    fn cgi_kills_process_group_on_timeout() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempdir().unwrap();
        let script = dir.path().join("hang.mu");
        // Child sleeps past CGI_TIMEOUT; process group kill must reclaim it.
        std::fs::write(&script, b"#!/bin/sh\nsleep 60\n").unwrap();
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
        let err = run_cgi(&script, [0u8; 16], None, &BTreeMap::new(), 1024).unwrap_err();
        assert!(
            err.to_string().contains("timed out"),
            "expected timeout, got {err}"
        );
    }

    #[test]
    fn non_executable_is_false_for_missing() {
        assert!(!is_unix_executable(Path::new("/no/such/script")));
    }
}
