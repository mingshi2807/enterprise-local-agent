use std::io;
use std::os::fd::RawFd;

const FIRST_CHILD_FD: RawFd = 198;

pub(crate) fn select_child_fds(
    workspace_source: RawFd,
    worker_source: RawFd,
) -> io::Result<(RawFd, RawFd)> {
    let mut candidates = (FIRST_CHILD_FD..FIRST_CHILD_FD + 4)
        .filter(|candidate| *candidate != workspace_source && *candidate != worker_source);
    let workspace = candidates
        .next()
        .ok_or_else(|| io::Error::other("child descriptor mapping is unavailable"))?;
    let worker = candidates
        .next()
        .ok_or_else(|| io::Error::other("child descriptor mapping is unavailable"))?;
    Ok((workspace, worker))
}

/// Marks every non-stdio descriptor CLOEXEC in the post-fork child.
///
/// The two descriptors needed by bubblewrap are subsequently duplicated by
/// `map_child_fd`, which clears CLOEXEC only on those child-local duplicates.
///
/// # Safety
///
/// This must only be called from `CommandExt::pre_exec`.
pub(crate) unsafe fn isolate_child_fds() -> io::Result<()> {
    // SAFETY: close_range with CLOSE_RANGE_CLOEXEC changes only descriptor
    // flags in the post-fork child and does not allocate or dereference data.
    let result = unsafe {
        libc::syscall(
            libc::SYS_close_range,
            3_u32,
            u32::MAX,
            libc::CLOSE_RANGE_CLOEXEC,
        )
    };
    if result == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Maps one inherited CLOEXEC descriptor in the post-fork child only.
///
/// # Safety
///
/// This must only be called from `CommandExt::pre_exec`. It uses only
/// async-signal-safe libc operations, performs no allocation, and never
/// changes descriptor flags in the multithreaded parent.
pub(crate) unsafe fn map_child_fd(source: RawFd, destination: RawFd) -> io::Result<()> {
    if source != destination {
        // SAFETY: both values are process-local file descriptor numbers and
        // this function is restricted to the post-fork child.
        if unsafe { libc::dup2(source, destination) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    // dup2 clears CLOEXEC except when source == destination.
    // SAFETY: destination is open after the branch above.
    let flags = unsafe { libc::fcntl(destination, libc::F_GETFD) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: F_SETFD only changes the child descriptor's close-on-exec bit.
    if unsafe { libc::fcntl(destination, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn child_destinations_never_alias_either_source() {
        for sources in [(3, 4), (198, 4), (3, 199), (198, 199), (200, 201)] {
            let (workspace, worker) =
                select_child_fds(sources.0, sources.1).expect("mapping must be available");
            assert_ne!(workspace, worker);
            assert!(![sources.0, sources.1].contains(&workspace));
            assert!(![sources.0, sources.1].contains(&worker));
        }
    }
}
