//! A device whose SSH server offers `hmac-sha1-96` and no other MAC.
//!
//! Older switches are often locked to it. The client must agree on it and
//! get as far as authentication; before it knew the algorithm, the
//! handshake failed with "No common Mac algorithm".
//!
//! The stand-in device is OpenSSH's own `sshd`, run as this user on a
//! loopback port with a throwaway host key and a configuration that allows
//! that one MAC — and only CTR ciphers, as such a device has, since an
//! AEAD cipher carries its own integrity check and no MAC is agreed at
//! all — so the algorithm is checked against an implementation
//! other than the one under test. Nothing on this machine can log in to it
//! (password login needs root for an sshd), so success is reaching the
//! login and being refused there. Skipped, saying so, where no `sshd` is
//! installed.

use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use corelens_discover::hostkeys::HostKeyStore;
use corelens_discover::ssh::{Credentials, Device, Secret, SshError, SshOptions};

struct Sshd(Child);
impl Drop for Sshd {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn sshd_binary() -> Option<&'static str> {
    ["/usr/sbin/sshd", "/usr/local/sbin/sshd", "/opt/homebrew/sbin/sshd"].into_iter().find(|p| Path::new(p).exists())
}

fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn start(sshd: &str, dir: &Path, port: u16, macs: &str) -> Sshd {
    let key = dir.join("host_ed25519");
    let made = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-f"])
        .arg(&key)
        .status()
        .expect("ssh-keygen runs");
    assert!(made.success());
    let config = dir.join("sshd_config");
    std::fs::write(
        &config,
        format!(
            "Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nPidFile {}\nCiphers aes128-ctr,aes256-ctr\nMACs {macs}\n\
             UsePAM no\nPasswordAuthentication yes\nKbdInteractiveAuthentication no\n\
             PubkeyAuthentication no\nAuthorizedKeysFile none\nStrictModes no\nLogLevel ERROR\n",
            key.display(),
            dir.join("sshd.pid").display(),
        ),
    )
    .unwrap();
    let child = Command::new(sshd)
        .arg("-D")
        .arg("-e")
        .arg("-f")
        .arg(&config)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("sshd starts");
    let deadline = Instant::now() + Duration::from_secs(10);
    while std::net::TcpStream::connect(("127.0.0.1", port)).is_err() {
        assert!(Instant::now() < deadline, "sshd did not listen on {port}");
        std::thread::sleep(Duration::from_millis(50));
    }
    Sshd(child)
}

#[tokio::test]
async fn a_server_offering_only_hmac_sha1_96_is_reached_up_to_the_login() {
    let Some(sshd) = sshd_binary() else {
        eprintln!("skipped: no sshd on this machine to stand in for the device");
        return;
    };
    let dir = std::env::temp_dir().join(format!("cv-sshd-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let port = free_port();
    let _server = start(sshd, &dir, port, "hmac-sha1-96");

    let credentials = Credentials {
        username: "nobody-here".into(),
        password: Secret::new("not-a-real-password"),
        enable_password: None,
    };
    let options = SshOptions {
        port,
        connect_timeout: Duration::from_secs(10),
        auth_timeout: Duration::from_secs(10),
        command_timeout: Duration::from_secs(5),
        login_transcript: None,
        support_capture: None,
        max_output_bytes: corelens_discover::ssh::DEFAULT_MAX_OUTPUT_BYTES,
    };
    let store = Arc::new(std::sync::Mutex::new(HostKeyStore::new()));
    let outcome = Device::connect("127.0.0.1", &credentials, options, store, None).await;
    let _ = std::fs::remove_dir_all(&dir);
    match outcome {
        Err(SshError::AuthFailed { .. }) => {}
        Err(other) => panic!("the handshake should agree on hmac-sha1-96 and fail only at the login: {other}"),
        Ok(_) => panic!("nothing should be able to log in to this sshd"),
    }
}
