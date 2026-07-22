//! IPC 传输层：Unix socket + JSON 消息帧。

use std::io::{self, BufReader, BufWriter, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::Serialize;

// ── Frame layer ────────────────────────────────────────────

pub fn read_frame(reader: &mut impl Read) -> io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 256 * 1024 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame too large: {len} bytes"),
        ));
    }
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(body)
}

pub fn write_frame(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let len = body.len() as u32;
    writer.write_all(&len.to_be_bytes())?;
    writer.write_all(body)?;
    writer.flush()?;
    Ok(())
}

// ── Client ─────────────────────────────────────────────────

pub struct IpcClient {
    reader: BufReader<UnixStream>,
    writer: BufWriter<UnixStream>,
}

impl IpcClient {
    pub fn connect(path: &Path) -> io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        let reader = BufReader::new(
            stream
                .try_clone()
                .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?,
        );
        let writer = BufWriter::new(stream);
        Ok(Self { reader, writer })
    }

    pub fn request<R: DeserializeOwned>(&mut self, req: &impl Serialize) -> io::Result<R> {
        let body = serde_json::to_vec(req).map_err(json_err)?;
        write_frame(&mut self.writer, &body)?;
        let resp_bytes = read_frame(&mut self.reader)?;
        serde_json::from_slice(&resp_bytes).map_err(json_err)
    }

    pub fn send(&mut self, req: &impl Serialize) -> io::Result<()> {
        let body = serde_json::to_vec(req).map_err(json_err)?;
        write_frame(&mut self.writer, &body)
    }

    pub fn try_read<R: DeserializeOwned>(&mut self) -> io::Result<Option<R>> {
        self.reader.get_mut().set_nonblocking(true)?;
        let result = read_frame(&mut self.reader);
        self.reader.get_mut().set_nonblocking(false)?;
        match result {
            Ok(body) => Ok(Some(serde_json::from_slice(&body).map_err(json_err)?)),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) => Err(e),
        }
    }
}

// ── Server ─────────────────────────────────────────────────

#[derive(Debug)]
pub struct IpcServer {
    listener: UnixListener,
    path: PathBuf,
}

impl IpcServer {
    pub fn bind(path: &Path) -> io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if path.exists() {
            if UnixStream::connect(path).is_ok() {
                return Err(io::Error::new(
                    io::ErrorKind::AddrInUse,
                    format!("daemon socket already in use: {}", path.display()),
                ));
            }
            std::fs::remove_file(path)?;
        }
        let listener = UnixListener::bind(path)?;
        Ok(Self {
            listener,
            path: path.to_path_buf(),
        })
    }

    pub fn accept(&self) -> io::Result<(UnixStream, std::os::unix::net::SocketAddr)> {
        self.listener.accept()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

// ── helpers ────────────────────────────────────────────────

pub fn json_err(e: serde_json::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, e)
}

// ── tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn frame_roundtrip() {
        let body = br#"{"type":"hello"}"#;
        let mut buf = Vec::new();
        write_frame(&mut buf, body).unwrap();
        assert_eq!(&buf[..4], &16u32.to_be_bytes());
        assert_eq!(&buf[4..], body);
        let back = read_frame(&mut Cursor::new(buf)).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn frame_fragmented_read() {
        let body = b"x".repeat(100);
        let mut buf = Vec::new();
        write_frame(&mut buf, &body).unwrap();
        let back = read_frame(&mut Cursor::new(buf)).unwrap();
        assert_eq!(back, body);
    }

    #[test]
    fn frame_too_large_rejected() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&(300_000u32).to_be_bytes());
        buf.resize(4 + 300_000, 0);
        let err = read_frame(&mut Cursor::new(buf)).unwrap_err();
        assert!(err.to_string().contains("too large"));
    }

    #[test]
    fn client_request_response() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("test.sock");
        let server = IpcServer::bind(&path).unwrap();
        let server_path = server.path().to_path_buf();
        let _jh = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            loop {
                match read_frame(&mut stream) {
                    Ok(b) => {
                        write_frame(&mut stream, &b).unwrap();
                    }
                    Err(_) => break,
                }
            }
        });
        let mut client = IpcClient::connect(&server_path).unwrap();
        #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
        struct Msg {
            text: String,
        }
        let resp: Msg = client
            .request(&Msg {
                text: "ping".into(),
            })
            .unwrap();
        assert_eq!(resp.text, "ping");
    }

    #[test]
    fn server_rejects_duplicate_bind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("dup.sock");
        let _s1 = IpcServer::bind(&path).unwrap();
        let err = IpcServer::bind(&path).unwrap_err();
        assert!(err.kind() == io::ErrorKind::AddrInUse);
    }

    #[test]
    fn server_cleans_stale_socket() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("stale.sock");
        std::fs::write(&path, b"").unwrap();
        let server = IpcServer::bind(&path).unwrap();
        assert!(server.path().exists());
        drop(server);
        assert!(!path.exists());
    }
}
