//! Windows: locate or fetch `wintun.dll` (the WireGuard project's TUN driver).
//!
//! Search order:
//! 1. `TVPN_WINTUN_DLL` environment variable
//! 2. next to `tvpn.exe` (release zips and winget ship it there)
//! 3. `%ProgramData%\tvpn\wintun\<arch>\wintun.dll` (cache)
//! 4. download the official release from wintun.net, verify its SHA-256
//!    against a pinned hash, and extract it into the cache.

use crate::paths;
use anyhow::{Context, Result, anyhow, bail};
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::PathBuf;

pub const WINTUN_VERSION: &str = "0.14.1";
const WINTUN_URL: &str = "https://www.wintun.net/builds/wintun-0.14.1.zip";
const WINTUN_SHA256: &str = "07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51";

fn arch_dir() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        "arm"
    }
}

fn cache_path() -> PathBuf {
    paths::machine_data_dir().join("wintun").join(arch_dir()).join("wintun.dll")
}

/// Where wintun.dll would be loaded from without downloading, if anywhere.
pub fn find_local() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("TVPN_WINTUN_DLL").map(PathBuf::from)
        && p.is_file()
    {
        return Some(p);
    }
    // Check next to the executable, also resolving symlinks (winget's portable
    // installs and npm shims launch tvpn through a link in another directory).
    if let Ok(exe) = std::env::current_exe() {
        let resolved = std::fs::canonicalize(&exe).ok().map(|p| {
            // Drop the verbatim prefix (\\?\) that canonicalize adds.
            let s = p.to_string_lossy().into_owned();
            PathBuf::from(s.strip_prefix(r"\\?\").map(str::to_string).unwrap_or(s))
        });
        for e in [Some(exe), resolved].into_iter().flatten() {
            if let Some(p) = e.parent().map(|d| d.join("wintun.dll"))
                && p.is_file()
            {
                return Some(p);
            }
        }
    }
    let cached = cache_path();
    cached.is_file().then_some(cached)
}

pub fn ensure() -> Result<PathBuf> {
    if let Some(p) = find_local() {
        return Ok(p);
    }
    eprintln!("Downloading the Wintun driver {WINTUN_VERSION} (one-time setup)…");
    let zip = download()
        .context("could not download wintun.dll; place it next to tvpn.exe or set TVPN_WINTUN_DLL")?;
    let digest = Sha256::digest(&zip);
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    if hex != WINTUN_SHA256 {
        bail!("wintun download failed verification (sha256 {hex}); refusing to use it");
    }
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip))?;
    let inner = format!("wintun/bin/{}/wintun.dll", arch_dir());
    let mut file = archive.by_name(&inner).with_context(|| format!("{inner} missing from archive"))?;
    let mut dll = Vec::new();
    file.read_to_end(&mut dll)?;

    let dest = cache_path();
    std::fs::create_dir_all(dest.parent().expect("has parent"))
        .with_context(|| format!("cannot create {}", dest.display()))?;
    let tmp = dest.with_extension("dll.tmp");
    std::fs::write(&tmp, &dll)?;
    std::fs::rename(&tmp, &dest)?;
    Ok(dest)
}

fn download() -> Result<Vec<u8>> {
    let mut resp = ureq::get(WINTUN_URL)
        .header("User-Agent", concat!("tvpn/", env!("CARGO_PKG_VERSION")))
        .call()
        .map_err(|e| anyhow!("{e}"))?;
    let bytes =
        resp.body_mut().with_config().limit(16 * 1024 * 1024).read_to_vec().map_err(|e| anyhow!("{e}"))?;
    Ok(bytes)
}
