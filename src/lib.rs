//! # tvpn
//!
//! An on-demand WireGuard VPN for the terminal, written in Rust. A single
//! binary acts as a client (`tvpn up`) and as a self-hosted server
//! (`tvpn server up`) on Linux, macOS and Windows.
//!
//! The library exposes the building blocks used by the CLI: the WireGuard
//! config format ([`config`]), keys ([`keys`]), the data-plane [`engine`]
//! (built on Cloudflare's `boringtun`), and system network configuration
//! ([`netcfg`]).

pub mod cli;
pub mod cloud;
pub mod commands;
pub mod config;
pub mod control;
pub mod device;
pub mod engine;
pub mod keys;
pub mod netcfg;
pub mod paths;
pub mod privilege;
pub mod profiles;
pub mod server;
pub mod tun;
pub mod ui;
#[cfg(windows)]
pub mod wintun;
