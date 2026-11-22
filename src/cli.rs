//! Command-line interface definition.

use clap::{Args, Parser, Subcommand};
use ipnet::{IpNet, Ipv4Net};
use std::net::IpAddr;
use std::path::PathBuf;

const ABOUT: &str =
    "On-demand WireGuard VPN for your terminal: connect when you need it, gone when you don't.";

const AFTER_HELP: &str = "\
Quick start (client):
  tvpn import ~/Downloads/provider.conf --name work
  tvpn up work            # foreground; press Ctrl+C to disconnect
  tvpn status             # from another terminal
  tvpn down

Quick start (self-hosted server on a Linux VPS):
  tvpn server init --endpoint vpn.example.com
  tvpn server add-peer laptop --qr
  tvpn server up

Profiles are standard WireGuard .conf files, so configs from any WireGuard
provider work, and configs made by `tvpn server add-peer` work in the official
WireGuard apps.";

const IMPORT_HELP: &str = "\
Where do I get a .conf file?
  - From a VPN provider that offers WireGuard config downloads
    (look for \"WireGuard configuration\" in your account's download page).
  - From a tvpn server you run: `tvpn server add-peer <name> -o <name>.conf`.

Examples:
  tvpn import ~/Downloads/us-east.conf --name work
  tvpn import laptop.conf
  tvpn up work";

#[derive(Debug, Parser)]
#[command(
    name = "tvpn",
    version,
    about = ABOUT,
    after_help = AFTER_HELP,
    propagate_version = true,
    arg_required_else_help = true
)]
pub struct Cli {
    /// More log output (-v, -vv, -vvv). TVPN_LOG / RUST_LOG override this.
    #[arg(short, long, action = clap::ArgAction::Count, global = true)]
    pub verbose: u8,

    /// Disable colored output (also honored: NO_COLOR environment variable)
    #[arg(long, global = true)]
    pub no_color: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Connect using a profile (runs in the foreground; Ctrl+C disconnects)
    #[command(visible_alias = "connect")]
    Up(UpArgs),

    /// Disconnect the running VPN session
    #[command(visible_alias = "disconnect")]
    Down(DownArgs),

    /// Show connection status
    Status(StatusArgs),

    /// Import a WireGuard .conf file as a profile
    #[command(after_help = IMPORT_HELP)]
    Import(ImportArgs),

    /// List saved profiles
    #[command(visible_alias = "ls")]
    List,

    /// Show a saved profile (private key hidden unless --reveal)
    Show(ShowArgs),

    /// Delete a saved profile
    #[command(visible_alias = "rm")]
    Remove(RemoveArgs),

    /// Generate a private key (like `wg genkey`)
    Genkey,

    /// Read a private key on stdin and print its public key (like `wg pubkey`)
    Pubkey,

    /// Generate a pre-shared key (like `wg genpsk`)
    Genpsk,

    /// Generate and print a private/public key pair
    Keypair,

    /// Change your region: a VPN server on demand in the country you pick (your AWS account)
    #[command(subcommand)]
    Cloud(CloudCommand),

    /// Run and manage your own VPN server
    #[command(subcommand)]
    Server(ServerCommand),

    /// Check this system for everything tvpn needs
    Doctor,

    /// Print a shell completion script (bash, zsh, fish, powershell, elvish)
    Completions(CompletionsArgs),
}

#[derive(Debug, Args)]
pub struct UpArgs {
    /// Profile name, or a path to a WireGuard .conf file. Optional if only one profile exists.
    pub profile: Option<String>,

    /// Do not change the system DNS settings
    #[arg(long)]
    pub no_dns: bool,

    /// Name for the tunnel interface (default: tvpn0; "tvpn" on Windows)
    #[arg(long, value_name = "NAME")]
    pub interface: Option<String>,

    /// No banner or live status line; only warnings and errors
    #[arg(short, long)]
    pub quiet: bool,

    /// Never re-run automatically through sudo (Linux/macOS)
    #[arg(long)]
    pub no_sudo: bool,
}

#[derive(Debug, Args)]
pub struct DownArgs {
    /// Never re-run automatically through sudo (Linux/macOS)
    #[arg(long)]
    pub no_sudo: bool,
}

#[derive(Debug, Args)]
pub struct StatusArgs {
    /// Machine-readable JSON output
    #[arg(long)]
    pub json: bool,
}

#[derive(Debug, Args)]
pub struct ImportArgs {
    /// WireGuard config file to import
    pub file: PathBuf,

    /// Profile name (default: derived from the file name)
    #[arg(short, long)]
    pub name: Option<String>,

    /// Overwrite an existing profile with the same name
    #[arg(short, long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ShowArgs {
    /// Profile name or path
    pub profile: String,

    /// Include the private key
    #[arg(long)]
    pub reveal: bool,

    /// Also render the full config as a QR code (for the WireGuard mobile apps)
    #[arg(long)]
    pub qr: bool,
}

#[derive(Debug, Args)]
pub struct RemoveArgs {
    /// Profile name
    pub profile: String,
}

#[derive(Debug, Args)]
pub struct CompletionsArgs {
    /// Target shell
    pub shell: clap_complete::Shell,
}

#[derive(Debug, Subcommand)]
pub enum ServerCommand {
    /// Create the server configuration (keys, subnet, port)
    Init(ServerInitArgs),

    /// Run the server in the foreground (Ctrl+C stops it)
    Up(ServerUpArgs),

    /// Stop the running server
    Down(DownArgs),

    /// Add a client and print its config (and QR code with --qr)
    #[command(visible_alias = "add")]
    AddPeer(AddPeerArgs),

    /// Remove a client
    #[command(visible_alias = "remove")]
    RemovePeer(PeerNameArgs),

    /// List clients (with live status when the server is running)
    #[command(visible_alias = "list")]
    Peers,

    /// Print a client's config again
    ShowPeer(ShowPeerArgs),

    /// Apply peer changes to the running server without restarting it
    Reload(DownArgs),
}

#[derive(Debug, Args)]
pub struct ServerInitArgs {
    /// Public hostname or IP clients connect to (default: this machine's outbound IP)
    #[arg(long, value_name = "HOST")]
    pub endpoint: Option<String>,

    /// UDP port to listen on
    #[arg(long, default_value_t = 51820)]
    pub port: u16,

    /// IPv4 subnet for the VPN
    #[arg(long, default_value = "10.8.0.0/24")]
    pub subnet: Ipv4Net,

    /// Also give clients IPv6 addresses from a private (ULA) /64
    #[arg(long)]
    pub ipv6: bool,

    /// DNS servers pushed to clients (comma separated)
    #[arg(long, value_delimiter = ',', default_value = "1.1.1.1,1.0.0.1")]
    pub dns: Vec<IpAddr>,

    /// Replace an existing server configuration (removes all peers)
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ServerUpArgs {
    /// Do not enable IP forwarding / NAT (peers can only reach this machine)
    #[arg(long)]
    pub no_nat: bool,

    /// Do not add firewall rules (INPUT/FORWARD accepts)
    #[arg(long)]
    pub no_firewall: bool,

    /// Internet-facing interface used for NAT (default: auto-detect)
    #[arg(long, value_name = "IFACE")]
    pub wan: Option<String>,

    /// No banner or live status line
    #[arg(short, long)]
    pub quiet: bool,

    /// Never re-run automatically through sudo
    #[arg(long)]
    pub no_sudo: bool,
}

#[derive(Debug, Args)]
pub struct AddPeerArgs {
    /// Name for the client, e.g. laptop or phone
    pub name: String,

    /// Traffic the client sends through the VPN (default: everything)
    #[arg(long, value_delimiter = ',', value_name = "CIDRS")]
    pub allowed_ips: Option<Vec<IpNet>>,

    /// Render the config as a QR code for the WireGuard mobile apps
    #[arg(long)]
    pub qr: bool,

    /// Also write the client config to this file
    #[arg(short, long, value_name = "FILE")]
    pub output: Option<PathBuf>,

    /// Also save it as a local tvpn profile with the same name
    #[arg(long)]
    pub import: bool,
}

#[derive(Debug, Args)]
pub struct PeerNameArgs {
    pub name: String,
}

#[derive(Debug, Args)]
pub struct ShowPeerArgs {
    pub name: String,

    /// Render as a QR code
    #[arg(long)]
    pub qr: bool,
}

const CLOUD_HELP: &str = "\
Example:
  tvpn cloud setup            # once: add your AWS access keys
  tvpn cloud up mumbai        # server in Mumbai; Ctrl+C disconnects and deletes it
  tvpn cloud up de            # country codes work too; see `tvpn cloud regions`

Each session gets a brand-new server with fresh keys. It is deleted when you
disconnect, deletes itself if no client is connected for a while, and has a
maximum lifetime, so a forgotten server doesn't keep costing money.";

#[derive(Debug, Subcommand)]
#[command(after_help = CLOUD_HELP)]
pub enum CloudCommand {
    /// Save your AWS access keys for `tvpn cloud` (one time; shows the IAM policy to use)
    Setup,

    /// List the regions you can connect through
    Regions,

    /// Start a server in REGION, connect through it, and delete it when you disconnect
    Up(CloudUpArgs),

    /// List tvpn cloud servers that are still running (all regions)
    #[command(visible_alias = "ls")]
    List,

    /// Delete tvpn cloud servers that are still running
    Cleanup(CloudCleanupArgs),

    /// Write a server's first-boot script and configs to a folder (for debugging)
    #[command(hide = true)]
    Render(CloudRenderArgs),
}

#[derive(Debug, Args)]
pub struct CloudRenderArgs {
    /// Public IP the client config should point at
    #[arg(long)]
    pub server_ip: std::net::IpAddr,

    /// Output folder (gets user-data.sh, server/server.conf, server/server.json, client.conf)
    #[arg(long)]
    pub dir: PathBuf,
}

#[derive(Debug, Args)]
pub struct CloudUpArgs {
    /// City, country code or AWS region, e.g. mumbai, tokyo, de, us-east-1 (see `tvpn cloud regions`)
    pub region: String,

    /// EC2 instance type for the server
    #[arg(long, default_value = "t3.micro", value_name = "TYPE")]
    pub instance_type: String,

    /// Safety net: delete the server after this many hours even if you stay connected
    #[arg(long, default_value_t = 8, value_parser = clap::value_parser!(u32).range(1..=168))]
    pub max_hours: u32,

    /// Safety net: the server deletes itself after this many minutes without a client
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u32).range(5..=1440))]
    pub idle_minutes: u32,

    /// Do not change the system DNS settings
    #[arg(long)]
    pub no_dns: bool,

    /// No banner or live status line
    #[arg(short, long)]
    pub quiet: bool,

    /// Never re-run automatically through sudo (Linux/macOS)
    #[arg(long)]
    pub no_sudo: bool,

    /// tvpn version to install on the server (default: this version)
    #[arg(long, hide = true)]
    pub server_version: Option<String>,
}

#[derive(Debug, Args)]
pub struct CloudCleanupArgs {
    /// Delete without asking for confirmation
    #[arg(short, long)]
    pub yes: bool,
}
