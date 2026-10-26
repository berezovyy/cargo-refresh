//! Short, private socket namespaces, independent of the project and TMPDIR.
use anyhow::{Context, Result, ensure};
use std::{
    fs,
    os::unix::{
        ffi::OsStrExt,
        fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

// rustd binds workers/<32-byte hash>/sock.new/s before renaming to sock.
const LONGEST_SOCKET: &str = "p/d/workers/00000000000000000000000000000000/sock.new/s";
const SESSION_NAME_LEN: usize = 10 + 1 + 8; // u32 PID + '-' + random suffix

pub(crate) fn validate_socket(path: &Path) -> Result<()> {
    let bytes = path.as_os_str().as_bytes();
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    ensure!(
        !bytes.is_empty() && !bytes.contains(&0) && bytes.len() < address.sun_path.len(),
        "Unix socket path is invalid or too long ({} bytes; maximum {}): {}",
        bytes.len(),
        address.sun_path.len() - 1,
        path.display()
    );
    Ok(())
}

fn root_for(runtime: Option<&Path>, uid: u32) -> PathBuf {
    if let Some(runtime) = runtime.filter(|p| p.is_absolute()) {
        let root = runtime.join("cargo-refresh");
        let worst = root.join("x".repeat(SESSION_NAME_LEN)).join(LONGEST_SOCKET);
        if validate_socket(&worst).is_ok() {
            return root;
        }
    }
    PathBuf::from(format!("/tmp/cargo-refresh-{uid}"))
}

fn check_private(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)
        .with_context(|| format!("inspect private runtime directory {}", path.display()))?;
    ensure!(
        meta.is_dir()
            && meta.uid() == unsafe { libc::geteuid() }
            && meta.permissions().mode() & 0o7777 == 0o700,
        "refusing unsafe runtime directory {}: require a real directory owned by the current user with mode 0700",
        path.display()
    );
    Ok(())
}

fn private_dir(path: &Path) -> Result<()> {
    match fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(e) => {
            return Err(e).with_context(|| format!("create runtime directory {}", path.display()));
        }
    }
    check_private(path)
}

pub(crate) fn clean_stale(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some((pid, suffix)) = name.to_str().and_then(|n| n.split_once('-')) else {
            continue;
        };
        let Ok(pid) = pid.parse::<i32>() else {
            continue;
        };
        if pid <= 0 || suffix.len() != 8 || !suffix.bytes().all(|b| b.is_ascii_alphanumeric()) {
            continue;
        }
        // EPERM and reused/live PIDs are deliberately preserved. Never follow symlinks.
        if unsafe { libc::kill(pid, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
            && check_private(&entry.path()).is_ok()
        {
            match fs::remove_dir_all(entry.path()) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e).context("clean stale socket namespace"),
            }
        }
    }
    Ok(())
}

pub(crate) fn socket_dir() -> Result<tempfile::TempDir> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR");
    let root = root_for(runtime.as_deref().map(Path::new), unsafe {
        libc::geteuid()
    });
    if let Some(runtime) = runtime.as_deref().map(Path::new)
        && root == runtime.join("cargo-refresh")
    {
        check_private(runtime)?;
    }
    private_dir(&root)?;
    clean_stale(&root)?;
    let dir = tempfile::Builder::new()
        .prefix(&format!("{}-", std::process::id()))
        .permissions(fs::Permissions::from_mode(0o700))
        .rand_bytes(8)
        .tempdir_in(root)?;
    check_private(dir.path())?;
    validate_socket(&dir.path().join(LONGEST_SOCKET))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn path_selection_reserves_worker_bind_suffix_and_ignores_deep_runtime() {
        assert_eq!(
            root_for(Some(Path::new("/run/user/1000")), 1000),
            Path::new("/run/user/1000/cargo-refresh")
        );
        assert_eq!(
            root_for(Some(&PathBuf::from("/r").join("x".repeat(120))), 1000),
            Path::new("/tmp/cargo-refresh-1000")
        );
        assert_eq!(
            root_for(Some(Path::new("relative")), 1000),
            root_for(None, 1000)
        );
        assert!(validate_socket(Path::new(&"x".repeat(107))).is_ok());
        assert!(
            validate_socket(Path::new(&"x".repeat(108)))
                .unwrap_err()
                .to_string()
                .contains("maximum 107")
        );
        // Byte length, not character count.
        assert!(validate_socket(Path::new(&"é".repeat(54))).is_err());
    }
    #[test]
    fn refuses_unsafe_permissions_symlinks_and_foreign_ownership() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("runtime");
        private_dir(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_dir(&dir).is_err());
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
        let link = tmp.path().join("link");
        std::os::unix::fs::symlink(&dir, &link).unwrap();
        assert!(private_dir(&link).is_err());
        if unsafe { libc::geteuid() } != 0 {
            assert!(check_private(Path::new("/root")).is_err());
        }
    }
    #[test]
    fn cleans_dead_pid_only_and_preserves_live_and_unknown_entries() {
        let tmp = tempfile::tempdir().unwrap();
        let dead = tmp.path().join("2147483647-abcdefgh");
        private_dir(&dead).unwrap();
        fs::write(dead.join("socket-placeholder"), b"").unwrap();
        let live = tmp.path().join(format!("{}-abcdefgh", std::process::id()));
        private_dir(&live).unwrap();
        let unknown = tmp.path().join("unknown");
        private_dir(&unknown).unwrap();
        let target = tmp.path().join("target");
        private_dir(&target).unwrap();
        std::os::unix::fs::symlink(&target, tmp.path().join("2147483646-abcdefgh")).unwrap();
        clean_stale(tmp.path()).unwrap();
        assert!(!dead.exists());
        assert!(live.exists() && unknown.exists() && target.exists());
    }
}
