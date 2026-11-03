//! What town asks of the system that Rust has no word for: to end a process group, and whether a
//! signal is ignored. A call into C is `unsafe` only because the compiler cannot check what C
//! promises; each is made here once, its promise kept, so the rest of town calls it as it would any
//! function.

/// A process, or a process group, by its number.
pub type Pid = libc::pid_t;

/// End a process group, and all in it. One already empty is no error to us.
pub fn end_group(group: Pid) {
    // SAFETY: a signal to a group; it touches no memory of ours
    unsafe { libc::killpg(group, libc::SIGKILL) };
}

/// Whether a process is there: a signal of none asks only that.
#[cfg(test)]
pub fn alive(pid: Pid) -> bool {
    // SAFETY: as in `end_group`, and a signal of none is not even sent
    unsafe { libc::kill(pid, 0) == 0 }
}

/// Whether this process was started to ignore a signal: under `nohup`, or as a job in the
/// background.
pub fn ignored(signal: i32) -> bool {
    // SAFETY: a null new action only reads the old one, into memory we own and C fills
    unsafe {
        let mut was: libc::sigaction = std::mem::zeroed();
        libc::sigaction(signal, std::ptr::null(), &mut was);
        was.sa_sigaction == libc::SIG_IGN
    }
}
