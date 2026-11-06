//! What town asks of the system that Rust has no word for.
//!
//! - **`unsafe`**: a call into C is, because the compiler cannot check what C promises.
//! - **once**: each is made here, its promise kept, so the rest of town calls it as any function.

/// A process, or a process group, by its number.
pub type Pid = libc::pid_t;

/// End a process group, and all in it; one already empty is no error.
pub fn end_group(group: Pid) {
    // SAFETY: a signal to a group; it touches no memory of ours
    unsafe { libc::killpg(group, libc::SIGKILL) };
}

/// Whether a process is there: signal 0 asks only that.
#[cfg(test)]
pub fn alive(pid: Pid) -> bool {
    // SAFETY: as in `end_group`; signal 0 is not even sent
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Wait until a process has ended, as the kernel says it: not by asking again and again. One
/// already gone has ended. It waits as a thread does, so a test waits for it aside.
#[cfg(all(test, target_os = "macos"))]
pub fn exited(pid: Pid) {
    // SAFETY: a kqueue of our own, one event of ours for C to fill, and the queue closed after
    unsafe {
        let queue = libc::kqueue();
        assert!(queue >= 0, "a kqueue");
        let mut ask: libc::kevent = std::mem::zeroed();
        ask.ident = pid as usize;
        ask.filter = libc::EVFILT_PROC;
        ask.flags = libc::EV_ADD | libc::EV_ONESHOT;
        ask.fflags = libc::NOTE_EXIT;
        let mut told: libc::kevent = std::mem::zeroed();
        // none to watch is gone already; else it waits for the one word
        let null = std::ptr::null();
        let n = libc::kevent(queue, &ask, 1, &mut told, 1, null);
        let gone = std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        libc::close(queue);
        assert!(n == 1 || gone, "the kernel would not say when {pid} ends");
    }
}

/// As on macOS, by a descriptor for the process, readable once it has ended.
#[cfg(all(test, target_os = "linux"))]
pub fn exited(pid: Pid) {
    // SAFETY: a descriptor of our own, polled, then closed
    unsafe {
        let fd = libc::syscall(libc::SYS_pidfd_open, pid, 0) as libc::c_int;
        if fd < 0 {
            return; // gone already
        }
        let mut poll = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
        libc::poll(&mut poll, 1, -1);
        libc::close(fd);
    }
}

/// Whether this process was started to ignore a signal: under `nohup`, or as a job in the
/// background.
pub fn ignored(signal: i32) -> bool {
    // SAFETY: a null new action only reads the old one, into memory of ours that C fills
    unsafe {
        let mut was: libc::sigaction = std::mem::zeroed();
        libc::sigaction(signal, std::ptr::null(), &mut was);
        was.sa_sigaction == libc::SIG_IGN
    }
}

/// Hold a file's lock, alone, while the file is open; whether it could be had now. It lets go as
/// the process ends, however it ends.
pub fn lock(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    // SAFETY: a lock on a file we keep open; it touches no memory of ours
    unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
}
