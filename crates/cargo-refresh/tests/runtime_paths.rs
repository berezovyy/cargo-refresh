//! Subprocess isolation lets this exercise TMPDIR/XDG without mutating test-global env.
use cargo_refresh::{agent, supervisor::Supervisor};
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::Path,
    process::Command,
    time::{Duration, Instant},
};

#[test]
fn deep_project_and_tmpdir_use_short_sockets_and_report_startup_failure() {
    const CHILD: &str = "REFRESH_RUNTIME_PATH_TEST_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let root = tempfile::tempdir().unwrap();
        let deep = root.path().join("deep-project-".repeat(12));
        fs::create_dir(&deep).unwrap();
        assert!(deep.as_os_str().len() > 150);
        let status = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "deep_project_and_tmpdir_use_short_sockets_and_report_startup_failure",
                "--nocapture",
            ])
            .current_dir(&deep)
            .env(CHILD, "1")
            .env("TMPDIR", &deep)
            // Long XDG must fall back, independently of TMPDIR or project length.
            .env("XDG_RUNTIME_DIR", &deep)
            .status()
            .unwrap();
        assert!(status.success());
        return;
    }
    let project = std::env::current_dir().unwrap();
    let storage = project.join("storage");
    let mut host = Supervisor::new(
        &project,
        &storage,
        vec!["-c".into(), "sleep 30; :".into()],
        false,
        "bash".into(),
    )
    .unwrap();
    host.start(Path::new("/bin/bash")).unwrap();
    assert!(host.executable.starts_with(&project));
    assert!(
        host.socket
            .starts_with(format!("/tmp/cargo-refresh-{}", unsafe { libc::geteuid() }))
    );
    assert!(host.socket.as_os_str().len() < 108);
    assert!(
        agent::Client::new(&host.socket)
            .ping_compatible()
            .unwrap()
            .ok
    );
    let socket_dir = host.socket.parent().unwrap().to_path_buf();
    assert_eq!(
        fs::metadata(&socket_dir).unwrap().permissions().mode() & 0o777,
        0o700
    );
    host.stop();
    assert!(!socket_dir.exists());

    // Simulate a library reporting a bind/startup failure using the real startup channel.
    let source = project.join("failure.c");
    let library = project.join("failure.so");
    fs::write(
        &source,
        r#"#include <stdio.h>
#include <stdlib.h>
__attribute__((constructor)) static void init(void) {
    FILE *f = fopen(getenv("REFRESH_AGENT_ERROR"), "w");
    if (f) { fputs("cannot bind/listen on agent socket: injected failure\n", f); fclose(f); }
}
"#,
    )
    .unwrap();
    assert!(
        Command::new("cc")
            .args(["-shared", "-fPIC"])
            .arg(&source)
            .arg("-o")
            .arg(&library)
            .status()
            .unwrap()
            .success()
    );
    host.configure_agent(Some(&library));
    let started = Instant::now();
    let error = host.start(Path::new("/bin/bash")).unwrap_err().to_string();
    assert!(
        error.contains("cannot bind/listen on agent socket: injected failure"),
        "{error}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "startup error waited for timeout"
    );
    host.stop();
}
