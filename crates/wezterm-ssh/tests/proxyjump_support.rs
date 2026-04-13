//! Regression test: ProxyJump should be supported when building wezterm-ssh
//! with only the `libssh-rs` feature (no `ssh2` feature).

#[cfg(all(feature = "libssh-rs", not(feature = "ssh2")))]
use wezterm_ssh::{Config, Session, SessionEvent};

// This test is only meaningful when `ssh2` is not compiled in.
#[cfg(all(feature = "libssh-rs", not(feature = "ssh2")))]
#[test]
fn proxyjump_does_not_require_ssh2_feature() {
    smol::block_on(async {
        // Configure a host that will fail to connect quickly (port 1).
        // The important part is that it fails due to connection/handshake,
        // not because ProxyJump is gated behind the `ssh2` feature.
        let mut cfg = Config::new();
        cfg.add_config_string(
            r#"
Host target
  HostName 127.0.0.1
  Port 2222
  User test
  ProxyJump 127.0.0.1:1
"#,
        );

        let mut config = cfg.for_host("target");
        config.insert("wezterm_ssh_backend".to_string(), "libssh".to_string());
        config.insert("wezterm_ssh_verbose".to_string(), "false".to_string());

        let (_session, events) = Session::connect(config).expect("Session::connect failed");

        let mut last_error: Option<String> = None;
        while let Ok(event) = events.recv().await {
            match event {
                SessionEvent::Error(err) => {
                    last_error = Some(err);
                    break;
                }
                SessionEvent::Authenticated => {
                    // Unexpected, but if it did authenticate then ProxyJump clearly worked.
                    break;
                }
                _ => {}
            }
        }

        let err = last_error.expect("expected an error event");
        assert!(
            !err.contains("not compiled with the `ssh2` feature"),
            "ProxyJump incorrectly requires ssh2: {}",
            err
        );
    })
}

// If compiled with ssh2, skip this test; this file exists primarily
// to cover the libssh-only build.
#[cfg(any(not(feature = "libssh-rs"), feature = "ssh2"))]
#[test]
fn proxyjump_support_test_is_skipped_in_this_feature_set() {}
