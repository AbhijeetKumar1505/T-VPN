//! Where tvpn keeps its files.
//!
//! * config (profiles, server):   `~/.config/tvpn` (Unix) or `%APPDATA%\tvpn` (Windows)
//! * runtime (live session state): `/run/tvpn` (Linux), `/var/run/tvpn` (macOS),
//!   `%ProgramData%\tvpn\run` (Windows)
//!
//! When run through `sudo`, the *invoking* user's config directory is used so
//! that `sudo tvpn up work` finds the profiles you imported without sudo.
//! Override with `TVPN_CONFIG_DIR` / `TVPN_RUNTIME_DIR`.

use anyhow::{Context, Result};
use std::fs;
use std::path::{Path, PathBuf};

pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TVPN_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    platform_config_dir()
}

#[cfg(unix)]
fn platform_config_dir() -> PathBuf {
    if let Some(home) = sudo_user_home() {
        return home.join(".config").join("tvpn");
    }
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("tvpn");
    }
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/root"));
    home.join(".config").join("tvpn")
}

#[cfg(windows)]
fn platform_config_dir() -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("USERPROFILE").map(|p| PathBuf::from(p).join("AppData").join("Roaming")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("tvpn")
}

pub fn profiles_dir() -> PathBuf {
    config_dir().join("profiles")
}

pub fn server_dir() -> PathBuf {
    config_dir().join("server")
}

pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("TVPN_RUNTIME_DIR") {
        return PathBuf::from(dir);
    }
    #[cfg(target_os = "linux")]
    {
        PathBuf::from("/run/tvpn")
    }
    #[cfg(all(unix, not(target_os = "linux")))]
    {
        PathBuf::from("/var/run/tvpn")
    }
    #[cfg(windows)]
    {
        program_data().join("tvpn").join("run")
    }
}

/// Machine-wide data directory (Windows: wintun driver cache).
#[cfg(windows)]
pub fn machine_data_dir() -> PathBuf {
    program_data().join("tvpn")
}

#[cfg(windows)]
fn program_data() -> PathBuf {
    std::env::var_os("ProgramData").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\ProgramData"))
}

/// Home directory of the user who invoked `sudo`, when running as root via sudo.
#[cfg(unix)]
pub fn sudo_user_home() -> Option<PathBuf> {
    if unsafe { libc::geteuid() } != 0 {
        return None;
    }
    let user = std::env::var("SUDO_USER").ok().filter(|u| !u.is_empty() && u != "root")?;
    let c_user = std::ffi::CString::new(user).ok()?;
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut buf = vec![0u8; 16 * 1024];
    let mut result: *mut libc::passwd = std::ptr::null_mut();
    let rc = unsafe {
        libc::getpwnam_r(c_user.as_ptr(), &mut pwd, buf.as_mut_ptr().cast(), buf.len(), &mut result)
    };
    if rc != 0 || result.is_null() || pwd.pw_dir.is_null() {
        return None;
    }
    let dir = unsafe { std::ffi::CStr::from_ptr(pwd.pw_dir) };
    Some(PathBuf::from(dir.to_string_lossy().into_owned()))
}

/// When running as root through sudo, hand files we create in the user's
/// config directory back to that user.
pub fn fix_owner(path: &Path) {
    #[cfg(unix)]
    {
        if unsafe { libc::geteuid() } != 0 || std::env::var_os("TVPN_CONFIG_DIR").is_some() {
            return;
        }
        let (Some(uid), Some(gid)) = (
            std::env::var("SUDO_UID").ok().and_then(|v| v.parse::<u32>().ok()),
            std::env::var("SUDO_GID").ok().and_then(|v| v.parse::<u32>().ok()),
        ) else {
            return;
        };
        if uid == 0 {
            return;
        }
        let config = config_dir();
        // Walk up from the path to the config dir, chowning each component we own.
        let mut cur = Some(path);
        while let Some(p) = cur {
            if !p.starts_with(&config) {
                break;
            }
            let _ = std::os::unix::fs::chown(p, Some(uid), Some(gid));
            if p == config {
                break;
            }
            cur = p.parent();
        }
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Create a directory (and parents) readable only by the owner on Unix.
pub fn create_private_dir(dir: &Path) -> Result<()> {
    fs::create_dir_all(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    fix_owner(dir);
    Ok(())
}

/// Write a file containing secrets (mode 0600 on Unix), atomically where possible.
pub fn write_private(path: &Path, contents: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_private_dir(parent)?;
    }
    let tmp = path.with_extension("tmp");
    {
        let mut opts = fs::OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut f = opts.open(&tmp).with_context(|| format!("cannot write {}", tmp.display()))?;
        std::io::Write::write_all(&mut f, contents)?;
        f.sync_all().ok();
    }
    fs::rename(&tmp, path).with_context(|| format!("cannot write {}", path.display()))?;
    fix_owner(path);
    Ok(())
}
