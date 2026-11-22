//! Profile store: WireGuard configs saved under `<config>/profiles/<name>.conf`.

use crate::config::{Config, Parsed};
use crate::paths;
use anyhow::{Context, Result, anyhow, bail};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ProfileRef {
    pub name: String,
    pub path: PathBuf,
}

pub fn validate_name(name: &str) -> Result<()> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && !name.starts_with('.');
    if !ok {
        bail!("invalid profile name `{name}`: use letters, digits, '-', '_' or '.' (max 64 chars)");
    }
    Ok(())
}

/// Derive a profile name from a file name (`Mullvad US #3.conf` -> `Mullvad-US-3`).
pub fn name_from_path(path: &Path) -> String {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut out = String::new();
    for c in stem.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches(|c| c == '-' || c == '.').to_string();
    let mut out = if out.is_empty() { "profile".to_string() } else { out };
    out.truncate(64);
    out
}

pub fn list_in(dir: &Path) -> Result<Vec<ProfileRef>> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e.eq_ignore_ascii_case("conf"))
            && let Some(stem) = path.file_stem()
        {
            out.push(ProfileRef { name: stem.to_string_lossy().into_owned(), path });
        }
    }
    out.sort_by_key(|p| p.name.to_lowercase());
    Ok(out)
}

pub fn list() -> Result<Vec<ProfileRef>> {
    list_in(&paths::profiles_dir())
}

/// Explains where WireGuard configs come from, for users who have none yet.
pub fn getting_started(name: &str) -> String {
    format!(
        "tvpn connects using a WireGuard config file (.conf). Get one from:\n  \
         - a VPN provider that offers WireGuard config downloads, or\n  \
         - a tvpn server you run: `tvpn server add-peer <name>` on that server\n\
         Then import it and connect:\n  \
         tvpn import <file.conf> --name {name}\n  \
         tvpn up {name}\n\
         Or let tvpn start a server in the region you want (uses your AWS account):\n  \
         tvpn cloud setup\n  \
         tvpn cloud up <region>      # e.g. mumbai, tokyo, de; see `tvpn cloud regions`"
    )
}

/// Resolve a profile argument: a saved profile name or a path to a `.conf` file.
/// With no argument, the only saved profile is used.
pub fn resolve(arg: Option<&str>) -> Result<ProfileRef> {
    let Some(arg) = arg else {
        let all = list()?;
        return match all.len() {
            0 => Err(anyhow!("no profiles yet.\n\n{}", getting_started("myvpn"))),
            1 => Ok(all.into_iter().next().expect("one profile")),
            _ => Err(anyhow!(
                "several profiles exist; choose one: {}",
                all.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
            )),
        };
    };

    let as_path = Path::new(arg);
    let looks_like_path =
        arg.contains('/') || arg.contains('\\') || arg.to_ascii_lowercase().ends_with(".conf");
    if looks_like_path || (as_path.is_file() && validate_name(arg).is_err()) {
        if !as_path.is_file() {
            bail!("config file not found: {arg}");
        }
        return Ok(ProfileRef { name: name_from_path(as_path), path: as_path.to_path_buf() });
    }

    let path = paths::profiles_dir().join(format!("{arg}.conf"));
    if path.is_file() {
        return Ok(ProfileRef { name: arg.to_string(), path });
    }
    if as_path.is_file() {
        return Ok(ProfileRef { name: name_from_path(as_path), path: as_path.to_path_buf() });
    }
    let available = list().unwrap_or_default();
    if available.is_empty() {
        bail!("profile `{arg}` not found: no profiles imported yet.\n\n{}", getting_started(arg));
    }
    bail!(
        "profile `{arg}` not found. Available: {}",
        available.iter().map(|p| p.name.as_str()).collect::<Vec<_>>().join(", ")
    )
}

pub fn load(path: &Path) -> Result<Parsed> {
    let text = fs::read_to_string(path).with_context(|| format!("cannot read {}", path.display()))?;
    Config::parse(&text).with_context(|| format!("invalid config {}", path.display()))
}

/// Import a config file into the profile store. Returns (name, destination, warnings).
pub fn import(src: &Path, name: Option<&str>, force: bool) -> Result<(String, PathBuf, Vec<String>)> {
    let parsed = load(src)?;
    parsed.config.validate_client().map_err(|e| anyhow!("{}: {e}", src.display()))?;
    let name = match name {
        Some(n) => n.to_string(),
        None => name_from_path(src),
    };
    validate_name(&name)?;
    let dest = paths::profiles_dir().join(format!("{name}.conf"));
    if dest.exists() && !force {
        bail!("profile `{name}` already exists (use --force to overwrite, or --name to pick another name)");
    }
    // Store the normalized form so the file is always valid for tvpn and wg-quick.
    paths::write_private(&dest, parsed.config.to_wg_string().as_bytes())?;
    Ok((name, dest, parsed.warnings))
}

pub fn save(name: &str, config: &Config, force: bool) -> Result<PathBuf> {
    validate_name(name)?;
    let dest = paths::profiles_dir().join(format!("{name}.conf"));
    if dest.exists() && !force {
        bail!("profile `{name}` already exists (use --force to overwrite)");
    }
    paths::write_private(&dest, config.to_wg_string().as_bytes())?;
    Ok(dest)
}

pub fn remove(name: &str) -> Result<PathBuf> {
    validate_name(name)?;
    let path = paths::profiles_dir().join(format!("{name}.conf"));
    if !path.exists() {
        bail!("profile `{name}` not found");
    }
    fs::remove_file(&path).with_context(|| format!("cannot remove {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_from_paths() {
        assert_eq!(name_from_path(Path::new("/x/Mullvad US #3.conf")), "Mullvad-US-3");
        assert_eq!(name_from_path(Path::new("wg0.conf")), "wg0");
        assert_eq!(name_from_path(Path::new("###.conf")), "profile");
    }

    #[test]
    fn name_validation() {
        assert!(validate_name("home").is_ok());
        assert!(validate_name("us-east_1.b").is_ok());
        assert!(validate_name("../etc").is_err());
        assert!(validate_name(".hidden").is_err());
        assert!(validate_name("").is_err());
        assert!(validate_name("a b").is_err());
    }
}
