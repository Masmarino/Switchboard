//! Socket Unix (droits du systeme de fichiers) sous macOS/Linux ; TCP sur la boucle locale
//! sous Windows, ou la std n'expose pas AF_UNIX. Le jeton de `control.json` protege les deux.

use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub(crate) trait ControlStream: Read + Write + Send + Sized + 'static {
    fn try_clone_stream(&self) -> std::io::Result<Self>;
    fn set_timeouts(&self, timeout: Duration) -> std::io::Result<()>;
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    pub(crate) type Listener = UnixListener;
    pub(crate) type Stream = UnixStream;

    impl ControlStream for UnixStream {
        fn try_clone_stream(&self) -> std::io::Result<Self> {
            self.try_clone()
        }

        fn set_timeouts(&self, timeout: Duration) -> std::io::Result<()> {
            self.set_read_timeout(Some(timeout))?;
            self.set_write_timeout(Some(timeout))
        }
    }

    /// `sun_path` est limite a 104 octets sous macOS (108 sous Linux).
    const MAX_SOCKET_PATH: usize = 100;

    /// Appelant garant qu'aucun superviseur vivant n'utilise deja la socket.
    pub(crate) fn bind(dir: &Path) -> std::io::Result<(Listener, String)> {
        use std::os::unix::fs::PermissionsExt;
        std::fs::create_dir_all(dir)?;
        let path = socket_path(dir);
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        Ok((listener, path.to_string_lossy().into_owned()))
    }

    /// A cote de `apps.json`, sauf si ce chemin est trop long : dossier runtime de
    /// l'utilisateur (ou temporaire). L'adresse reelle est publiee dans `control.json`.
    fn socket_path(dir: &Path) -> std::path::PathBuf {
        let preferred = dir.join(crate::control::endpoint::SOCKET_FILE_NAME);
        if preferred.as_os_str().len() <= MAX_SOCKET_PATH {
            return preferred;
        }
        let short_dir = std::env::var_os("XDG_RUNTIME_DIR").map(std::path::PathBuf::from).unwrap_or_else(std::env::temp_dir);
        short_dir.join(format!("switchboard-{}.sock", std::process::id()))
    }

    pub(crate) fn accept(listener: &Listener) -> std::io::Result<Stream> {
        listener.accept().map(|(stream, _)| stream)
    }

    pub(crate) fn connect(address: &str, _timeout: Duration) -> std::io::Result<Stream> {
        UnixStream::connect(address)
    }

    pub(crate) fn cleanup(address: &str) {
        let _ = std::fs::remove_file(address);
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::net::{SocketAddr, TcpListener, TcpStream};

    pub(crate) type Listener = TcpListener;
    pub(crate) type Stream = TcpStream;

    impl ControlStream for TcpStream {
        fn try_clone_stream(&self) -> std::io::Result<Self> {
            self.try_clone()
        }

        fn set_timeouts(&self, timeout: Duration) -> std::io::Result<()> {
            self.set_read_timeout(Some(timeout))?;
            self.set_write_timeout(Some(timeout))
        }
    }

    pub(crate) fn bind(_dir: &Path) -> std::io::Result<(Listener, String)> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let address = listener.local_addr()?.to_string();
        Ok((listener, address))
    }

    pub(crate) fn accept(listener: &Listener) -> std::io::Result<Stream> {
        listener.accept().map(|(stream, _)| stream)
    }

    pub(crate) fn connect(address: &str, timeout: Duration) -> std::io::Result<Stream> {
        let addr: SocketAddr = address
            .parse()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, format!("{address}: {e}")))?;
        if !addr.ip().is_loopback() {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidInput, "control endpoint must be loopback"));
        }
        TcpStream::connect_timeout(&addr, timeout)
    }

    pub(crate) fn cleanup(_address: &str) {}
}

pub(crate) use imp::{accept, bind, cleanup, connect, Listener};
