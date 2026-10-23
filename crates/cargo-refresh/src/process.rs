//! Spawn retries for Linux's transient inherited-writer ETXTBSY window.
use std::{
    io,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

pub fn spawn(command: &mut Command) -> io::Result<Child> {
    spawn_for(command, Duration::from_secs(1))
}

fn spawn_for(command: &mut Command, timeout: Duration) -> io::Result<Child> {
    let deadline = Instant::now() + timeout;
    let mut delay = Duration::from_millis(1);
    loop {
        match command.spawn() {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) => {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return Err(error);
                }
                thread::sleep(delay.min(remaining));
                delay = (delay * 2).min(Duration::from_millis(50));
            }
            result => return result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        os::{fd::AsRawFd, unix::fs::PermissionsExt},
    };

    #[test]
    fn inherited_writable_copy_exec_stress_500() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("app");
        let running = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let active = running.clone();
        let forks = thread::spawn(move || {
            while active.load(std::sync::atomic::Ordering::Relaxed) {
                assert!(Command::new("/bin/true").status().unwrap().success());
            }
        });
        for iteration in 0..500 {
            if iteration > 0 {
                fs::remove_file(&executable).unwrap();
            }
            let mut writer = fs::File::create(&executable).unwrap();
            std::io::copy(&mut fs::File::open("/bin/true").unwrap(), &mut writer).unwrap();
            fs::set_permissions(&executable, fs::Permissions::from_mode(0o500)).unwrap();
            assert_ne!(
                unsafe { libc::fcntl(writer.as_raw_fd(), libc::F_GETFD) } & libc::FD_CLOEXEC,
                0
            );
            let mut pipe = [0; 2];
            assert_eq!(
                unsafe { libc::pipe2(pipe.as_mut_ptr(), libc::O_CLOEXEC) },
                0
            );
            let pid = unsafe { libc::fork() };
            assert!(pid >= 0);
            if pid == 0 {
                // Only async-signal-safe libc calls after fork in this multithreaded test.
                unsafe {
                    libc::close(pipe[1]);
                    let mut byte = 0_u8;
                    libc::read(pipe[0], (&mut byte as *mut u8).cast(), 1);
                    libc::close(pipe[0]);
                    let pause = libc::timespec {
                        tv_sec: 0,
                        tv_nsec: 2_000_000,
                    };
                    libc::nanosleep(&pause, std::ptr::null_mut());
                    libc::_exit(0);
                }
            }
            unsafe {
                libc::close(pipe[0]);
            }
            drop(writer);
            let error = Command::new(&executable).spawn().unwrap_err();
            assert_eq!(
                error.raw_os_error(),
                Some(libc::ETXTBSY),
                "iteration {iteration}"
            );
            // Release the forked child (and its inherited CLOEXEC writer) while retrying exec.
            unsafe {
                libc::close(pipe[1]);
            }
            assert!(
                spawn(&mut Command::new(&executable))
                    .unwrap()
                    .wait()
                    .unwrap()
                    .success()
            );
            assert_eq!(unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) }, pid);
        }
        running.store(false, std::sync::atomic::Ordering::Relaxed);
        forks.join().unwrap();
    }

    #[test]
    fn persistent_busy_is_bounded_and_other_errors_are_preserved() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("app");
        fs::copy("/bin/true", &executable).unwrap();
        let _writer = fs::OpenOptions::new()
            .write(true)
            .open(&executable)
            .unwrap();
        assert_eq!(
            spawn_for(&mut Command::new(&executable), Duration::from_millis(20))
                .unwrap_err()
                .raw_os_error(),
            Some(libc::ETXTBSY)
        );
        assert_eq!(
            spawn(&mut Command::new(directory.path().join("absent")))
                .unwrap_err()
                .kind(),
            io::ErrorKind::NotFound
        );
    }
}
