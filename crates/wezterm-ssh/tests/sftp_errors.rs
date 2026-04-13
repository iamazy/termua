use wezterm_ssh::{SftpChannelError, SftpError};

#[test]
fn sftp_not_found_errors_are_reported_as_not_found() {
    assert!(SftpChannelError::Sftp(SftpError::NoSuchFile).is_not_found());
    assert!(SftpChannelError::Sftp(SftpError::NoSuchPath).is_not_found());
    assert!(!SftpChannelError::Sftp(SftpError::PermissionDenied).is_not_found());
}

#[cfg(feature = "libssh-rs")]
#[test]
fn libssh_missing_path_codes_are_reported_as_not_found() {
    let missing_file =
        SftpChannelError::LibSsh(libssh_rs::Error::fatal("SftpError: Sftp error code 2"));
    let missing_path =
        SftpChannelError::LibSsh(libssh_rs::Error::fatal("SftpError: Sftp error code 10"));
    let denied = SftpChannelError::LibSsh(libssh_rs::Error::fatal("SftpError: Sftp error code 3"));

    assert!(missing_file.is_not_found());
    assert!(missing_path.is_not_found());
    assert!(!denied.is_not_found());
}
