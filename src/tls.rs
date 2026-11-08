//! TLS: the self-signed certificate, its fingerprint, and an acceptor that answers plain
//! HTTP on the HTTPS port with a redirect, so typing `flipper:8443` without https://
//! still lands on the page.

use std::io;
use std::path::Path;
use std::time::Duration;

use axum_server::accept::Accept;
use axum_server::tls_rustls::{RustlsAcceptor, RustlsConfig};
use futures_util::future::BoxFuture;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::CertificateDer;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// The certificate and key PEMs in `dir`, made on first use and kept, so a browser's
/// exception for it holds from one run to the next.
pub fn certificate(dir: &Path) -> io::Result<(Vec<u8>, Vec<u8>)> {
    let (cert, key) = (dir.join("cert.pem"), dir.join("key.pem"));
    if let (Ok(c), Ok(k)) = (std::fs::read(&cert), std::fs::read(&key)) {
        return Ok((c, k));
    }
    let names = vec!["flipper-one".to_string(), "flipper-one.local".to_string(), "localhost".to_string()];
    let made = rcgen::generate_simple_self_signed(names).map_err(io::Error::other)?;
    std::fs::create_dir_all(dir)?;
    std::fs::write(&cert, made.cert.pem())?;
    // The key is private from the first byte: created with mode 0600, not fixed after.
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&key)?;
    f.write_all(made.key_pair.serialize_pem().as_bytes())?;
    drop(f);
    Ok((std::fs::read(&cert)?, std::fs::read(&key)?))
}

/// SHA-256 of the certificate as browsers show it: "AB:CD:...".
pub fn fingerprint(cert_pem: &[u8]) -> String {
    let Ok(der) = CertificateDer::from_pem_slice(cert_pem) else { return String::new() };
    let digest = ring::digest::digest(&ring::digest::SHA256, der.as_ref());
    digest.as_ref().iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

/// TLS for connections that start with a TLS handshake; a redirect for the rest.
#[derive(Clone)]
pub struct SniffAcceptor {
    tls: RustlsAcceptor,
    port: u16,
}

impl SniffAcceptor {
    pub fn new(config: RustlsConfig, port: u16) -> Self {
        SniffAcceptor { tls: RustlsAcceptor::new(config).handshake_timeout(Duration::from_secs(10)), port }
    }
}

impl<S: Send + 'static> Accept<TcpStream, S> for SniffAcceptor {
    type Stream = <RustlsAcceptor as Accept<TcpStream, S>>::Stream;
    type Service = S;
    type Future = BoxFuture<'static, io::Result<(Self::Stream, S)>>;

    fn accept(&self, stream: TcpStream, service: S) -> Self::Future {
        let tls = self.tls.clone();
        let port = self.port;
        Box::pin(async move {
            let mut first = [0u8; 1];
            let n = tokio::time::timeout(Duration::from_secs(10), stream.peek(&mut first))
                .await
                .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "no request"))??;
            // 0x16: a TLS handshake record.
            if n == 1 && first[0] != 0x16 {
                redirect(stream, port).await;
                return Err(io::Error::other("plain HTTP, redirected to HTTPS"));
            }
            tls.accept(stream, service).await
        })
    }
}

/// Read a plain HTTP request head and answer it with a redirect to the same place over
/// HTTPS.
async fn redirect(mut stream: TcpStream, port: u16) {
    let mut buf = vec![0u8; 4096];
    let mut len = 0;
    let read = async {
        while len < buf.len() {
            let n = stream.read(&mut buf[len..]).await?;
            if n == 0 {
                break;
            }
            len += n;
            if buf[..len].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        io::Result::Ok(())
    };
    if tokio::time::timeout(Duration::from_secs(5), read).await.is_err() {
        return;
    }
    let local = stream.local_addr().map(|a| format!("{}:{port}", a.ip())).unwrap_or_default();
    let location = redirect_target(&String::from_utf8_lossy(&buf[..len]), &local);
    let res = format!("HTTP/1.1 301 Moved Permanently\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let _ = stream.write_all(res.as_bytes()).await;
    let _ = stream.shutdown().await;
}

/// Where a plain request should go: the same host and path, over HTTPS. Only a host made
/// of host characters is echoed, and only an absolute path.
fn redirect_target(head: &str, fallback_host: &str) -> String {
    let mut lines = head.lines();
    let path = lines
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .filter(|p| p.starts_with('/') && !p.starts_with("//") && p.chars().all(|c| c.is_ascii_graphic()))
        .unwrap_or("/");
    let host = lines
        .find_map(|l| l.strip_prefix("Host:").or_else(|| l.strip_prefix("host:")))
        .map(str::trim)
        .filter(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_alphanumeric() || ".-:[]".contains(c)))
        .unwrap_or(fallback_host);
    format!("https://{host}{path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redirects_keep_host_and_path() {
        let head = "GET /stream?x=1 HTTP/1.1\r\nHost: 10.0.0.5:8443\r\nUser-Agent: t\r\n\r\n";
        assert_eq!(redirect_target(head, "1.2.3.4:8443"), "https://10.0.0.5:8443/stream?x=1");
    }

    #[test]
    fn redirects_do_not_echo_junk() {
        let head = "GET //evil.example/ HTTP/1.1\r\nHost: a\"b<c>\r\n\r\n";
        assert_eq!(redirect_target(head, "1.2.3.4:8443"), "https://1.2.3.4:8443/");
        assert_eq!(redirect_target("", "1.2.3.4:8443"), "https://1.2.3.4:8443/");
    }

    #[test]
    fn fingerprint_of_a_made_certificate() {
        let dir = std::env::temp_dir().join(format!("kvm-tls-test-{}", std::process::id()));
        let (cert, _) = certificate(&dir).unwrap();
        let fp = fingerprint(&cert);
        assert_eq!(fp.len(), 32 * 3 - 1);
        assert_eq!(certificate(&dir).unwrap().0, cert, "kept, not remade");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
