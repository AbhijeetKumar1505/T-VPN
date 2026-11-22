//! Profile management and key utilities.

use crate::cli::{ImportArgs, RemoveArgs, ShowArgs};
use crate::control::{Mode, SessionState};
use crate::keys::Key;
use crate::{paths, profiles, ui};
use anyhow::{Context, Result, bail};
use std::io::Read;

pub fn import(args: ImportArgs) -> Result<i32> {
    let (name, dest, warnings) = profiles::import(&args.file, args.name.as_deref(), args.force)?;
    for w in warnings {
        ui::warn(w);
    }
    ui::success(format!("Imported profile {}", ui::cyan(&name)));
    ui::kv("saved to", dest.display());
    ui::hint(format!("Connect with: tvpn up {name}"));
    Ok(0)
}

pub fn list() -> Result<i32> {
    let all = profiles::list()?;
    if all.is_empty() {
        println!("No profiles yet.");
        ui::hint("Import a WireGuard config:   tvpn import <file.conf> [--name NAME]");
        ui::hint(format!("Profiles live in {}", paths::profiles_dir().display()));
        return Ok(0);
    }
    let active = SessionState::load(Mode::Client).map(|s| s.profile);
    let rows: Vec<(String, Vec<String>)> = all
        .iter()
        .map(|p| {
            let marker =
                if active.as_deref() == Some(p.name.as_str()) { ui::green(ui::ON) } else { String::new() };
            let cells = match profiles::load(&p.path) {
                Ok(parsed) => {
                    let c = parsed.config;
                    let ep = c
                        .peers
                        .iter()
                        .find_map(|p| p.endpoint.as_ref())
                        .map(ToString::to_string)
                        .unwrap_or_else(|| "-".into());
                    let addr =
                        c.interface.addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
                    vec![p.name.clone(), ep, addr]
                }
                Err(e) => vec![p.name.clone(), ui::yellow(format!("invalid: {e:#}")), String::new()],
            };
            (marker, cells)
        })
        .collect();
    ui::table(&["PROFILE", "ENDPOINT", "ADDRESS"], &rows);
    Ok(0)
}

pub fn show(args: ShowArgs) -> Result<i32> {
    let p = profiles::resolve(Some(&args.profile))?;
    let parsed = profiles::load(&p.path)?;
    let text = parsed.config.to_wg_string();
    let shown = if args.reveal {
        text.clone()
    } else {
        text.lines()
            .map(|l| {
                if l.to_ascii_lowercase().starts_with("privatekey") {
                    "PrivateKey = (hidden; use --reveal)".to_string()
                } else if l.to_ascii_lowercase().starts_with("presharedkey") {
                    "PresharedKey = (hidden; use --reveal)".to_string()
                } else {
                    l.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    println!("{}", ui::dim(format!("# {}", p.path.display())));
    println!("{shown}");
    println!();
    ui::kv("public key", parsed.config.interface.private_key.public_key());
    if args.qr {
        println!();
        println!("{}", ui::qr(&text)?);
        ui::warn("this QR code contains the profile's private key; don't share it");
    }
    Ok(0)
}

pub fn remove(args: RemoveArgs) -> Result<i32> {
    if SessionState::load(Mode::Client).is_some_and(|s| s.profile == args.profile) {
        bail!("profile `{}` is currently connected; run `tvpn down` first", args.profile);
    }
    let path = profiles::remove(&args.profile)?;
    ui::success(format!("Removed profile {} ({})", args.profile, path.display()));
    Ok(0)
}

pub fn genkey() -> Result<i32> {
    println!("{}", Key::generate_private());
    Ok(0)
}

pub fn genpsk() -> Result<i32> {
    println!("{}", Key::generate_preshared());
    Ok(0)
}

pub fn pubkey() -> Result<i32> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input).context("cannot read stdin")?;
    let key: Key = input.trim().parse()?;
    println!("{}", key.public_key());
    Ok(0)
}

pub fn keypair() -> Result<i32> {
    let private = Key::generate_private();
    println!("PrivateKey = {private}");
    println!("PublicKey  = {}", private.public_key());
    Ok(0)
}
