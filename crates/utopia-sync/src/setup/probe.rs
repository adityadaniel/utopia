//! A read-only, single manifest request (§12.6). Caller runs it off the UI thread.
use crate::{SyncError, Transport, PROTOCOL_VERSION};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Connection {
    Reachable,
    Unauthorized,
    VersionMismatch,
    Unreachable,
}
impl Connection {
    pub fn message(self) -> &'static str {
        match self {
            Self::Reachable => "Reachable — protocol supported.",
            Self::Unauthorized => "Unauthorized — check the relay token (401/403).",
            Self::VersionMismatch => {
                "Version mismatch — update Utopia or the relay before syncing."
            }
            Self::Unreachable => {
                "Could not reach a compatible sync target. Check its address and try again."
            }
        }
    }
}
pub fn test_connection(t: &mut dyn Transport) -> Connection {
    match t.manifest(None) {
        Ok(Some(m)) if m.protocol_version == PROTOCOL_VERSION => Connection::Reachable,
        Ok(Some(_)) | Err(SyncError::ProtocolVersion(_)) => Connection::VersionMismatch,
        Err(SyncError::Unauthorized) => Connection::Unauthorized,
        _ => Connection::Unreachable,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    #[test]
    fn ac_12_6_test_connection_maps_http_results_without_echoing_secrets() {
        for (status, version, expected) in [
            (200, 1, Connection::Reachable),
            (401, 1, Connection::Unauthorized),
            (403, 1, Connection::Unauthorized),
            (200, 999, Connection::VersionMismatch),
            (500, 1, Connection::Unreachable),
        ] {
            let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let url = format!("http://{}", server.local_addr().unwrap());
            let host = std::thread::spawn(move || {
                let (mut socket, _) = server.accept().unwrap();
                let mut request = [0; 4096];
                let n = socket.read(&mut request).unwrap();
                assert!(String::from_utf8_lossy(&request[..n])
                    .starts_with("GET /v1/graphs/g/manifest "));
                let body = if status == 200 {
                    format!(r#"{{"protocol_version":{version},"rev":0,"pages":[]}}"#)
                } else {
                    "secret-server-body".into()
                };
                write!(socket, "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            });
            let mut t =
                crate::http::HttpTransport::new(&url, "g", Some("token-secret".into())).unwrap();
            let result = test_connection(&mut t);
            assert_eq!(result, expected);
            assert!(!result.message().contains("secret"));
            host.join().unwrap();
        }
    }
}
