//! Privilege detection and (on Unix) transparent re-execution through sudo.

use anyhow::{Result, bail};

/// True when running as root (Unix) or from an elevated process (Windows).
pub fn is_elevated() -> bool {
    imp::is_elevated()
}

/// Make sure we can configure network interfaces.
///
/// On Linux/macOS, when not root and running interactively, this re-executes
/// the same command through `sudo` (and does not return on success).
pub fn ensure_elevated(allow_sudo: bool) -> Result<()> {
    if is_elevated() {
        return Ok(());
    }
    #[cfg(unix)]
    if allow_sudo {
        imp::reexec_with_sudo()?;
    }
    let _ = allow_sudo;
    bail!("{}", elevation_hint())
}

pub fn elevation_hint() -> &'static str {
    #[cfg(windows)]
    {
        "administrator rights are required to create the VPN adapter.\n  \
         Open your terminal (cmd, PowerShell or Windows Terminal) with \"Run as administrator\",\n  \
         or prefix the command with `sudo` if Sudo for Windows is enabled."
    }
    #[cfg(unix)]
    {
        "root privileges are required to create the tunnel interface.\n  \
         Re-run the command with sudo, e.g. `sudo tvpn up <profile>`."
    }
}

#[cfg(unix)]
mod imp {
    use anyhow::{Result, bail};
    use std::io::IsTerminal;
    use std::os::unix::process::CommandExt;
    use std::process::Command;

    pub fn is_elevated() -> bool {
        unsafe { libc::geteuid() == 0 }
    }

    pub fn reexec_with_sudo() -> Result<()> {
        if std::env::var_os("TVPN_NO_SUDO").is_some() || !std::io::stdin().is_terminal() {
            return Ok(());
        }
        let Some(sudo) = super::which("sudo") else { return Ok(()) };
        let exe = std::env::current_exe()?;
        eprintln!("tvpn needs root to manage network interfaces; re-running with sudo…");
        let mut cmd = Command::new(sudo);
        // Carry over tvpn-specific settings, which sudo would otherwise drop.
        let mut envs: Vec<String> = Vec::new();
        for var in ["TVPN_CONFIG_DIR", "TVPN_RUNTIME_DIR", "TVPN_LOG", "RUST_LOG", "NO_COLOR"] {
            if let Ok(val) = std::env::var(var) {
                envs.push(format!("{var}={val}"));
            }
        }
        if !envs.is_empty() {
            cmd.arg("env").args(envs);
        }
        cmd.arg(exe).args(std::env::args_os().skip(1));
        cmd.env("TVPN_NO_SUDO", "1");
        let err = cmd.exec();
        bail!("failed to run sudo: {err}")
    }
}

#[cfg(windows)]
mod imp {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    pub fn is_elevated() -> bool {
        unsafe {
            let mut token: HANDLE = std::ptr::null_mut();
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            let mut elevation = TOKEN_ELEVATION { TokenIsElevated: 0 };
            let mut len = 0u32;
            let ok = GetTokenInformation(
                token,
                TokenElevation,
                (&mut elevation as *mut TOKEN_ELEVATION).cast(),
                std::mem::size_of::<TOKEN_ELEVATION>() as u32,
                &mut len,
            );
            CloseHandle(token);
            ok != 0 && elevation.TokenIsElevated != 0
        }
    }
}

/// Locate an executable on PATH.
pub fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    // Common sbin locations are often missing from a non-root PATH.
    #[cfg(unix)]
    for dir in ["/usr/sbin", "/sbin", "/usr/local/sbin"] {
        let candidate = std::path::Path::new(dir).join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}
