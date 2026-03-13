use crate::ErrorPayload;
use crate::Request;
use crate::Response;
use crate::Shared;
use std::fs;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering;
use std::thread;
use tokio::runtime::Handle;
use tracing::warn;

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixListener;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(windows)]
use uds_windows::UnixListener;
#[cfg(windows)]
use uds_windows::UnixStream;

pub struct LocalIpcServer {
    stop_requested: Arc<AtomicBool>,
    join_handle: Option<thread::JoinHandle<()>>,
    socket_path: PathBuf,
}

impl LocalIpcServer {
    pub fn start(socket_path: PathBuf, shared: Arc<Shared>) -> io::Result<Self> {
        let parent = socket_path.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "socket path `{}` has no parent directory",
                    socket_path.display()
                ),
            )
        })?;
        fs::create_dir_all(parent)?;
        set_dir_permissions(parent)?;
        if socket_path.exists() {
            fs::remove_file(&socket_path)?;
        }

        let listener = UnixListener::bind(&socket_path)?;
        set_socket_permissions(&socket_path)?;
        let stop_requested = Arc::new(AtomicBool::new(false));
        let stop_requested_for_thread = Arc::clone(&stop_requested);
        let socket_path_for_thread = socket_path.clone();
        let runtime_handle = Handle::current();

        let join_handle = thread::spawn(move || {
            loop {
                let accepted = listener.accept();
                let (mut stream, _) = match accepted {
                    Ok(accepted) => accepted,
                    Err(err) => {
                        if stop_requested_for_thread.load(Ordering::Relaxed) {
                            break;
                        }
                        warn!(
                            socket_path = %socket_path_for_thread.display(),
                            "control-plane IPC accept failed: {err}"
                        );
                        continue;
                    }
                };
                if stop_requested_for_thread.load(Ordering::Relaxed) {
                    break;
                }
                if let Err(err) = handle_stream(&mut stream, &shared, &runtime_handle) {
                    warn!(
                        socket_path = %socket_path_for_thread.display(),
                        "control-plane IPC request failed: {err}"
                    );
                }
            }
        });

        Ok(Self {
            stop_requested,
            join_handle: Some(join_handle),
            socket_path,
        })
    }
}

impl Drop for LocalIpcServer {
    fn drop(&mut self) {
        self.stop_requested.store(true, Ordering::Relaxed);
        let _ = UnixStream::connect(&self.socket_path);
        if let Some(join_handle) = self.join_handle.take()
            && let Err(err) = join_handle.join()
        {
            warn!("control-plane IPC thread join failed: {err:?}");
        }
        if let Err(err) = fs::remove_file(&self.socket_path)
            && err.kind() != io::ErrorKind::NotFound
        {
            warn!(
                socket_path = %self.socket_path.display(),
                "failed to remove control-plane socket: {err}"
            );
        }
    }
}

fn handle_stream(
    stream: &mut UnixStream,
    shared: &Arc<Shared>,
    runtime_handle: &Handle,
) -> io::Result<()> {
    let mut request_bytes = Vec::new();
    stream.read_to_end(&mut request_bytes)?;
    let request_text = String::from_utf8(request_bytes).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid UTF-8 request: {err}"),
        )
    })?;
    let trimmed = request_text.trim();

    let response = if trimmed.is_empty() {
        Response::Error {
            payload: ErrorPayload {
                message: "request must not be empty".to_string(),
            },
        }
    } else {
        match serde_json::from_str::<Request>(trimmed) {
            Ok(request) => runtime_handle.block_on(shared.handle_request(request)),
            Err(err) => Response::Error {
                payload: ErrorPayload {
                    message: format!("invalid control-plane request: {err}"),
                },
            },
        }
    };
    let json = serde_json::to_vec(&response).map_err(|err| {
        io::Error::other(format!("failed to serialize control-plane response: {err}"))
    })?;
    stream.write_all(&json)?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    Ok(())
}

#[cfg(unix)]
fn set_dir_permissions(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(unix)]
fn set_socket_permissions(path: &Path) -> io::Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(windows)]
fn set_dir_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(windows)]
fn set_socket_permissions(_path: &Path) -> io::Result<()> {
    Ok(())
}
