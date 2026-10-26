//! Windows/Linux peers must run this placed helper, not merely share a uid
//! or know the socket token. macOS keeps its existing code-signature check.

#[cfg(not(target_os = "macos"))]
pub fn validate(pid: Option<u32>) -> std::io::Result<()> {
    let own = crate::process::executable(std::process::id());
    let peer = pid.and_then(crate::process::executable);
    if matches!((own, peer), (Some(own), Some(peer)) if same_path(&own, &peer)) {
        Ok(())
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "computer-use peer does not run this helper executable",
        ))
    }
}

#[cfg(any(not(target_os = "macos"), test))]
pub(crate) fn same_path(own: &std::path::Path, peer: &std::path::Path) -> bool {
    // Linux retains this suffix on a running binary replaced during update.
    let own = own.to_string_lossy();
    let peer = peer.to_string_lossy();
    let own = own.strip_suffix(" (deleted)").unwrap_or(&own);
    let peer = peer.strip_suffix(" (deleted)").unwrap_or(&peer);
    if cfg!(windows) {
        own.eq_ignore_ascii_case(peer)
    } else {
        own == peer
    }
}

#[cfg(test)]
mod tests {
    use super::same_path;
    use std::path::Path;

    #[test]
    fn another_program_or_another_copy_is_not_the_placed_helper() {
        let own = Path::new("/user/computer/bin/brigadier-cu");
        assert!(same_path(own, own));
        assert!(same_path(
            own,
            Path::new("/user/computer/bin/brigadier-cu (deleted)")
        ));
        assert!(!same_path(own, Path::new("/tmp/brigadier-cu")));
        assert!(!same_path(own, Path::new("/usr/bin/python3")));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn kernel_process_identity_is_required() {
        assert!(super::validate(Some(std::process::id())).is_ok());
        assert!(super::validate(None).is_err());
        assert!(super::validate(Some(u32::MAX)).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn an_unrelated_same_user_process_is_refused() {
        let mut child = std::process::Command::new("sleep")
            .arg("5")
            .spawn()
            .unwrap();
        let verdict = super::validate(Some(child.id()));
        let _ = child.kill();
        let _ = child.wait();
        assert_eq!(
            verdict.unwrap_err().kind(),
            std::io::ErrorKind::PermissionDenied
        );
    }
}
