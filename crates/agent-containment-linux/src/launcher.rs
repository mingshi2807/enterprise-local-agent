use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::process::Stdio;
use std::time::Duration;

use rustix::process::{Pid, PidfdFlags, Signal, pidfd_open, pidfd_send_signal};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command};

use crate::fd_map::{isolate_child_fds, map_child_fd, select_child_fds};
use crate::protocol::{
    MAX_PROTOCOL_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES, WorkerRequest, WorkerResponse,
};

const MAX_STDERR_BYTES: usize = 4 * 1024;
const TERMINATION_TIMEOUT: Duration = Duration::from_secs(2);

pub(crate) struct SpawnedWorker {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    stdout: Option<ChildStdout>,
    stderr: Option<ChildStderr>,
    request: Vec<u8>,
}

impl SpawnedWorker {
    pub(crate) fn spawn(
        bwrap_path: &std::path::Path,
        workspace_parent: &OwnedFd,
        worker: &OwnedFd,
        request: &WorkerRequest,
    ) -> io::Result<Self> {
        let payload = serde_json::to_vec(request)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid worker request"))?;
        if payload.len() > MAX_PROTOCOL_FRAME_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "worker request too large",
            ));
        }
        let length = u32::try_from(payload.len())
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "worker request too large"))?;
        let mut framed = Vec::with_capacity(payload.len() + 4);
        framed.extend_from_slice(&length.to_be_bytes());
        framed.extend_from_slice(&payload);

        let workspace_source = workspace_parent.as_raw_fd();
        let worker_source = worker.as_raw_fd();
        let (workspace_child_fd, worker_child_fd) =
            select_child_fds(workspace_source, worker_source)?;
        let workspace_child_fd_arg = workspace_child_fd.to_string();
        let worker_child_fd_arg = worker_child_fd.to_string();
        let mut command = Command::new(bwrap_path);
        command
            .args([
                "--unshare-user",
                "--unshare-pid",
                "--unshare-net",
                "--unshare-ipc",
                "--unshare-uts",
                "--cap-drop",
                "ALL",
                "--new-session",
                "--die-with-parent",
                "--clearenv",
                "--bind-fd",
                &workspace_child_fd_arg,
                "/workspace",
                "--ro-bind-fd",
                &worker_child_fd_arg,
                "/worker",
                "--proc",
                "/proc",
                "--dir",
                "/dev",
                "--tmpfs",
                "/tmp",
                "--chdir",
                "/workspace",
                "--setenv",
                "LANG",
                "C",
                "--setenv",
                "TZ",
                "UTC",
                "--",
                "/worker",
            ])
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(false);

        // SAFETY: the closure is restricted to async-signal-safe dup2/fcntl
        // operations in the post-fork child. Parent descriptors remain CLOEXEC.
        unsafe {
            command.pre_exec(move || {
                isolate_child_fds()?;
                map_child_fd(workspace_source, workspace_child_fd)?;
                map_child_fd(worker_source, worker_child_fd)?;
                Ok(())
            });
        }

        let mut child = command.spawn()?;
        let stdin = child.stdin.take().ok_or_else(pipe_error)?;
        let stdout = child.stdout.take().ok_or_else(pipe_error)?;
        let stderr = child.stderr.take().ok_or_else(pipe_error)?;
        Ok(Self {
            child: Some(child),
            stdin: Some(stdin),
            stdout: Some(stdout),
            stderr: Some(stderr),
            request: framed,
        })
    }

    pub(crate) async fn exchange(&mut self) -> io::Result<WorkerResponse> {
        let mut stdin = self.stdin.take().ok_or_else(pipe_error)?;
        let mut stdout = self.stdout.take().ok_or_else(pipe_error)?;
        let mut stderr = self.stderr.take().ok_or_else(pipe_error)?;
        let child = self.child.as_mut().ok_or_else(pipe_error)?;
        let request = std::mem::take(&mut self.request);

        let write = async move {
            stdin.write_all(&request).await?;
            stdin.shutdown().await
        };
        let read_stdout = read_bounded(&mut stdout, MAX_RESPONSE_FRAME_BYTES + 4);
        let read_stderr = read_bounded(&mut stderr, MAX_STDERR_BYTES);
        let wait = child.wait();
        let joined = tokio::try_join!(write, read_stdout, read_stderr, wait);
        let ((), output, _stderr, status) = match joined {
            Ok(joined) => joined,
            Err(error) => {
                let _ = child.start_kill();
                let _ = child.wait().await;
                return Err(error);
            }
        };
        if !status.success() {
            return Err(io::Error::other("contained worker failed"));
        }
        decode_response(&output)
    }

    #[cfg(feature = "certification-hooks")]
    pub(crate) async fn send_request(&mut self) -> io::Result<()> {
        let mut stdin = self.stdin.take().ok_or_else(pipe_error)?;
        let request = std::mem::take(&mut self.request);
        stdin.write_all(&request).await?;
        stdin.shutdown().await
    }

    pub(crate) async fn terminate_and_reap(&mut self) -> io::Result<()> {
        self.stdin.take();
        self.stdout.take();
        self.stderr.take();
        if let Some(mut child) = self.child.take() {
            let signal_result = signal_sandbox_tree(&mut child);
            let wait_result = match tokio::time::timeout(TERMINATION_TIMEOUT, child.wait()).await {
                Ok(result) => result.map(|_| ()),
                Err(_) => {
                    let fallback = signal_child(&mut child);
                    let wait = tokio::time::timeout(TERMINATION_TIMEOUT, child.wait())
                        .await
                        .map_err(|_| io::Error::other("contained process did not terminate"))?
                        .map(|_| ());
                    fallback.and(wait)
                }
            };
            signal_result.and(wait_result)?;
        }
        Ok(())
    }

    #[cfg(feature = "certification-hooks")]
    pub(crate) fn pid(&self) -> Option<u32> {
        self.child.as_ref().and_then(Child::id)
    }
}

pub(crate) fn probe_lifecycle_support() -> io::Result<()> {
    let raw = i32::try_from(std::process::id())
        .map_err(|_| io::Error::other("process identifier is unsupported"))?;
    let pid =
        Pid::from_raw(raw).ok_or_else(|| io::Error::other("process identifier is unsupported"))?;
    let _pidfd = pidfd_open(pid, PidfdFlags::empty()).map_err(io::Error::from)?;
    std::fs::read_to_string(format!("/proc/self/task/{raw}/children"))?;
    Ok(())
}

impl Drop for SpawnedWorker {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

async fn read_bounded(reader: &mut (impl AsyncRead + Unpin), limit: usize) -> io::Result<Vec<u8>> {
    let mut output = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            return Ok(output);
        }
        if output.len().saturating_add(read) > limit {
            return Err(io::Error::other("contained worker output exceeded limit"));
        }
        output.extend_from_slice(&chunk[..read]);
    }
}

fn decode_response(frame: &[u8]) -> io::Result<WorkerResponse> {
    if frame.len() < 4 {
        return Err(io::Error::other("contained worker response was incomplete"));
    }
    let length = u32::from_be_bytes([frame[0], frame[1], frame[2], frame[3]]) as usize;
    if length > MAX_RESPONSE_FRAME_BYTES || frame.len() != length + 4 {
        return Err(io::Error::other(
            "contained worker response framing was invalid",
        ));
    }
    serde_json::from_slice(&frame[4..])
        .map_err(|_| io::Error::other("contained worker response was invalid"))
}

fn pipe_error() -> io::Error {
    io::Error::other("contained worker pipe was unavailable")
}

fn direct_child_pidfds(parent: u32) -> io::Result<Vec<OwnedFd>> {
    let children = match std::fs::read_to_string(format!("/proc/{parent}/task/{parent}/children")) {
        Ok(children) => children,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut pidfds = Vec::new();
    for value in children.split_whitespace() {
        let raw = value
            .parse::<i32>()
            .map_err(|_| io::Error::other("contained process tree was invalid"))?;
        let pid = Pid::from_raw(raw)
            .ok_or_else(|| io::Error::other("contained process tree was invalid"))?;
        match pidfd_open(pid, PidfdFlags::empty()) {
            Ok(pidfd) => pidfds.push(pidfd),
            Err(rustix::io::Errno::SRCH) => {}
            Err(error) => return Err(io::Error::from(error)),
        }
    }
    Ok(pidfds)
}

fn signal_sandbox_tree(child: &mut Child) -> io::Result<()> {
    let Some(raw) = child.id().and_then(|pid| i32::try_from(pid).ok()) else {
        return Ok(());
    };
    let pid = Pid::from_raw(raw)
        .ok_or_else(|| io::Error::other("contained process identifier was invalid"))?;
    let outer = match pidfd_open(pid, PidfdFlags::empty()) {
        Ok(outer) => outer,
        Err(rustix::io::Errno::SRCH) => return Ok(()),
        Err(error) => return Err(io::Error::from(error)),
    };
    if let Err(error) = pidfd_send_signal(&outer, Signal::STOP)
        && error != rustix::io::Errno::SRCH
    {
        return Err(io::Error::from(error));
    }
    let supervisors = match direct_child_pidfds(raw as u32) {
        Ok(supervisors) => supervisors,
        Err(error) => {
            let _ = pidfd_send_signal(&outer, Signal::KILL);
            return Err(error);
        }
    };
    if supervisors.is_empty() {
        signal_pidfd(&outer, Signal::KILL)
    } else {
        let mut result = Ok(());
        for supervisor in &supervisors {
            if let Err(error) = signal_pidfd(supervisor, Signal::KILL)
                && result.is_ok()
            {
                result = Err(error);
            }
        }
        let resume = signal_pidfd(&outer, Signal::CONT);
        result.and(resume)
    }
}

fn signal_pidfd(pidfd: &OwnedFd, signal: Signal) -> io::Result<()> {
    match pidfd_send_signal(pidfd, signal) {
        Ok(()) | Err(rustix::io::Errno::SRCH) => Ok(()),
        Err(error) => Err(io::Error::from(error)),
    }
}

fn signal_child(child: &mut Child) -> io::Result<()> {
    match child.start_kill() {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::InvalidInput => Ok(()),
        Err(error) => Err(error),
    }
}
