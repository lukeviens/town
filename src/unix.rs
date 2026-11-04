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
