//! Pid liveness (spec section 4): same-machine assumption makes
//! kill(pid, 0) an honest check. EPERM means "exists, not ours" -> alive.

#[allow(dead_code)] // used from Task 2 onward
pub fn pid_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    let rc = unsafe { libc::kill(pid, 0) };
    if rc == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn own_pid_is_alive() {
        assert!(pid_alive(std::process::id() as i32));
    }

    #[test]
    fn reaped_child_is_dead() {
        let mut child = std::process::Command::new("true").spawn().unwrap();
        let pid = child.id() as i32;
        child.wait().unwrap();
        assert!(!pid_alive(pid));
    }

    #[test]
    fn nonpositive_pids_are_dead() {
        assert!(!pid_alive(0));
        assert!(!pid_alive(-1));
    }
}
