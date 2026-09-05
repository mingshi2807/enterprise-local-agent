#[allow(dead_code)]
#[path = "../path.rs"]
mod path;
#[allow(dead_code)]
#[path = "../protocol.rs"]
mod protocol;

use std::fs::File;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::os::fd::OwnedFd;
use std::os::unix::fs::MetadataExt;
use std::time::Duration;

use landlock::{
    ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetStatus,
};
use rustix::fd::AsFd;
use rustix::fs::{AtFlags, Mode, OFlags, open, openat2, renameat, unlinkat};

use path::resolve_flags;
use protocol::{
    LandlockState, MAX_CONTENT_BYTES, MAX_PROTOCOL_FRAME_BYTES, MAX_RESPONSE_FRAME_BYTES,
    NamespaceIds, PROTOCOL_VERSION, WorkerErrorCode, WorkerRequest, WorkerResponse,
    WorkerWriteResult,
};

fn main() {
    let response = match initialize_process().and_then(|()| read_request()) {
        Ok(request) => handle_request(request),
        Err(code) => WorkerResponse::ProtocolError {
            version: PROTOCOL_VERSION,
            code,
        },
    };
    let _ = write_response(&response);
}

fn initialize_process() -> Result<(), WorkerErrorCode> {
    // SAFETY: prctl and umask have no pointer arguments here and affect only
    // this single-threaded worker process before any untrusted data is read.
    if unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) } != 0 {
        return Err(WorkerErrorCode::Unsupported);
    }
    // SAFETY: setting a restrictive process umask is local and infallible.
    unsafe { libc::umask(0o077) };
    Ok(())
}

fn read_request() -> Result<WorkerRequest, WorkerErrorCode> {
    let mut input = std::io::stdin().lock();
    let mut length = [0_u8; 4];
    input
        .read_exact(&mut length)
        .map_err(|_| WorkerErrorCode::InvalidInput)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_PROTOCOL_FRAME_BYTES {
        return Err(WorkerErrorCode::InvalidInput);
    }
    let mut payload = vec![0_u8; length];
    input
        .read_exact(&mut payload)
        .map_err(|_| WorkerErrorCode::InvalidInput)?;
    let mut trailing = [0_u8; 1];
    if input
        .read(&mut trailing)
        .map_err(|_| WorkerErrorCode::InvalidInput)?
        != 0
    {
        return Err(WorkerErrorCode::InvalidInput);
    }
    serde_json::from_slice(&payload).map_err(|_| WorkerErrorCode::InvalidInput)
}

fn write_response(response: &WorkerResponse) -> Result<(), WorkerErrorCode> {
    let payload = serde_json::to_vec(response).map_err(|_| WorkerErrorCode::IoFailure)?;
    if payload.len() > MAX_RESPONSE_FRAME_BYTES {
        return Err(WorkerErrorCode::IoFailure);
    }
    let length = u32::try_from(payload.len()).map_err(|_| WorkerErrorCode::IoFailure)?;
    let mut output = std::io::stdout().lock();
    output
        .write_all(&length.to_be_bytes())
        .and_then(|()| output.write_all(&payload))
        .and_then(|()| output.flush())
        .map_err(|_| WorkerErrorCode::IoFailure)
}

fn handle_request(request: WorkerRequest) -> WorkerResponse {
    match request {
        WorkerRequest::Probe {
            version,
            host_namespaces,
            host_probe_port,
        } if version == PROTOCOL_VERSION => probe(host_namespaces, host_probe_port),
        WorkerRequest::WorkspaceWriteFile {
            version,
            tool_call_id,
            basename,
            content,
        } if version == PROTOCOL_VERSION => {
            let result = write_file(tool_call_id, &basename, &content);
            WorkerResponse::WorkspaceWriteFile {
                version: PROTOCOL_VERSION,
                tool_call_id,
                result,
            }
        }
        #[cfg(feature = "certification-hooks")]
        WorkerRequest::CertificationHold { version } if version == PROTOCOL_VERSION => {
            certification_hold()
        }
        WorkerRequest::Probe { .. } | WorkerRequest::WorkspaceWriteFile { .. } => {
            WorkerResponse::ProtocolError {
                version: PROTOCOL_VERSION,
                code: WorkerErrorCode::Unsupported,
            }
        }
        #[cfg(feature = "certification-hooks")]
        WorkerRequest::CertificationHold { .. } => WorkerResponse::ProtocolError {
            version: PROTOCOL_VERSION,
            code: WorkerErrorCode::Unsupported,
        },
    }
}

#[cfg(feature = "certification-hooks")]
fn certification_hold() -> WorkerResponse {
    // SAFETY: this certification-only fork occurs in the single-threaded
    // worker. The child calls only async-signal-safe pause until killed.
    let child = unsafe { libc::fork() };
    if child == 0 {
        loop {
            // SAFETY: pause has no arguments and the child performs no Rust
            // allocation or library work after fork.
            unsafe { libc::pause() };
        }
    }
    if child < 0 {
        return WorkerResponse::ProtocolError {
            version: PROTOCOL_VERSION,
            code: WorkerErrorCode::IoFailure,
        };
    }
    if write_file_inner("certification", ".ela-m6-1-certification-ready", b"ready").is_err() {
        return WorkerResponse::ProtocolError {
            version: PROTOCOL_VERSION,
            code: WorkerErrorCode::IoFailure,
        };
    }
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn probe(host: NamespaceIds, port: u16) -> WorkerResponse {
    let own = namespace_ids();
    let namespaces_isolated = own.is_ok_and(|own| {
        own.user != host.user
            && own.mount != host.mount
            && own.pid != host.pid
            && own.network != host.network
    });
    let network_isolated = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into(),
        Duration::from_millis(100),
    )
    .is_err();
    let environment: Vec<_> = std::env::vars().collect();
    let environment_isolated = environment.len() == 3
        && environment.iter().all(|(name, value)| {
            matches!(
                (name.as_str(), value.as_str()),
                ("LANG", "C") | ("TZ", "UTC") | ("PWD", "/workspace")
            )
        });
    let descriptors_isolated = (3..4096).all(|fd| {
        // SAFETY: F_GETFD only queries whether this integer names an open FD.
        (unsafe { libc::fcntl(fd, libc::F_GETFD) }) == -1
    });
    let home_isolated = !std::path::Path::new("/home").exists()
        && !std::path::Path::new("/root").exists()
        && std::env::var_os("HOME").is_none()
        && std::env::var_os("SSH_AUTH_SOCK").is_none();
    // SAFETY: PR_GET_NO_NEW_PRIVS has no pointer arguments.
    let no_new_privileges = unsafe { libc::prctl(libc::PR_GET_NO_NEW_PRIVS, 0, 0, 0, 0) } == 1;
    let capabilities_dropped = capabilities_are_zero();
    let openat2_enforced = probe_openat2();
    let landlock = apply_landlock();
    WorkerResponse::Probe {
        version: PROTOCOL_VERSION,
        namespaces_isolated,
        network_isolated,
        environment_isolated,
        descriptors_isolated,
        home_isolated,
        no_new_privileges,
        capabilities_dropped,
        openat2_enforced,
        landlock,
    }
}

fn write_file(
    tool_call_id: agent_core::ToolCallId,
    basename: &str,
    content: &str,
) -> WorkerWriteResult {
    if basename.is_empty()
        || basename == "."
        || basename == ".."
        || basename.contains('/')
        || basename.chars().any(char::is_control)
        || content.len() > MAX_CONTENT_BYTES
    {
        return failed(WorkerErrorCode::InvalidInput);
    }
    let _landlock = apply_landlock();
    match write_file_inner(&tool_call_id.to_string(), basename, content.as_bytes()) {
        Ok(bytes_written) => WorkerWriteResult::Written { bytes_written },
        Err(code) => failed(code),
    }
}

fn write_file_inner(
    temporary_id: &str,
    basename: &str,
    content: &[u8],
) -> Result<u32, WorkerErrorCode> {
    let root = open(
        "/workspace",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map_err(map_errno)?;
    let temp_name = format!(".ela-write-{temporary_id}-tmp");
    let temp = openat2(
        root.as_fd(),
        temp_name.as_str(),
        OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::from_raw_mode(0o600),
        resolve_flags(),
    )
    .map_err(map_errno)?;
    let result = commit_file(root.as_fd(), temp, &temp_name, basename, content);
    if result.is_err() {
        let _ = unlinkat(root.as_fd(), temp_name.as_str(), AtFlags::empty());
    }
    result
}

fn commit_file(
    root: impl AsFd,
    temp: OwnedFd,
    temp_name: &str,
    basename: &str,
    content: &[u8],
) -> Result<u32, WorkerErrorCode> {
    let mut file = File::from(temp);
    file.write_all(content)
        .map_err(|error| map_io_error(&error))?;
    file.sync_all().map_err(|error| map_io_error(&error))?;
    renameat(root.as_fd(), temp_name, root.as_fd(), basename).map_err(map_errno)?;
    let directory = openat2(
        root.as_fd(),
        ".",
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        resolve_flags(),
    )
    .map_err(map_errno)?;
    if let Err(error) = File::from(directory).sync_all()
        && error.raw_os_error() != Some(libc::EINVAL)
    {
        return Err(map_io_error(&error));
    }
    u32::try_from(content.len()).map_err(|_| WorkerErrorCode::IoFailure)
}

fn probe_openat2() -> bool {
    let Ok(root) = open(
        "/workspace",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    ) else {
        return false;
    };
    openat2(
        root.as_fd(),
        ".",
        OFlags::PATH | OFlags::DIRECTORY | OFlags::CLOEXEC,
        Mode::empty(),
        resolve_flags(),
    )
    .is_ok()
}

fn apply_landlock() -> LandlockState {
    let abi = ABI::V9;
    let Ok(workspace) = PathFd::new("/workspace") else {
        return LandlockState::Unavailable;
    };
    let result = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))
        .and_then(|ruleset| ruleset.create())
        .and_then(|ruleset| ruleset.add_rule(PathBeneath::new(workspace, AccessFs::from_all(abi))))
        .and_then(|ruleset| ruleset.restrict_self());
    match result {
        Ok(status) => match status.ruleset {
            RulesetStatus::FullyEnforced => LandlockState::FullyEnforced,
            RulesetStatus::PartiallyEnforced => LandlockState::PartiallyEnforced,
            RulesetStatus::NotEnforced => LandlockState::Unavailable,
        },
        Err(_) => LandlockState::Unavailable,
    }
}

fn namespace_ids() -> std::io::Result<NamespaceIds> {
    Ok(NamespaceIds {
        user: std::fs::metadata("/proc/self/ns/user")?.ino(),
        mount: std::fs::metadata("/proc/self/ns/mnt")?.ino(),
        pid: std::fs::metadata("/proc/self/ns/pid")?.ino(),
        network: std::fs::metadata("/proc/self/ns/net")?.ino(),
    })
}

fn capabilities_are_zero() -> bool {
    let Ok(status) = std::fs::read_to_string("/proc/self/status") else {
        return false;
    };
    status
        .lines()
        .find_map(|line| line.strip_prefix("CapEff:\t"))
        .is_some_and(|value| value.bytes().all(|byte| byte == b'0'))
}

fn map_errno(error: rustix::io::Errno) -> WorkerErrorCode {
    match error {
        rustix::io::Errno::NOENT => WorkerErrorCode::NotFound,
        rustix::io::Errno::EXIST | rustix::io::Errno::ISDIR | rustix::io::Errno::NOTDIR => {
            WorkerErrorCode::Conflict
        }
        rustix::io::Errno::XDEV | rustix::io::Errno::LOOP | rustix::io::Errno::ACCESS => {
            WorkerErrorCode::Rejected
        }
        _ => WorkerErrorCode::IoFailure,
    }
}

fn map_io_error(error: &std::io::Error) -> WorkerErrorCode {
    error
        .raw_os_error()
        .map_or(WorkerErrorCode::IoFailure, |code| {
            map_errno(rustix::io::Errno::from_raw_os_error(code))
        })
}

const fn failed(code: WorkerErrorCode) -> WorkerWriteResult {
    WorkerWriteResult::Failed { code }
}
