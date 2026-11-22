//! `tvpn cloud ...`: change your region with an on-demand server.

use super::session::Shutdown;
use super::up::{self, Connect};
use crate::cli::{CloudCleanupArgs, CloudCommand, CloudRenderArgs, CloudUpArgs};
use crate::cloud::aws::{self, Aws, Credentials, Instance};
use crate::cloud::regions::{self, Region};
use crate::cloud::{self, CloudServer, bootstrap};
use crate::control::{self, Mode, SessionState};
use crate::{privilege, ui};
use anyhow::{Context, Result, anyhow, bail};
use std::net::IpAddr;
use std::time::{Duration, Instant};

pub async fn run(cmd: CloudCommand) -> Result<i32> {
    match cmd {
        CloudCommand::Setup => setup().await,
        CloudCommand::Regions => list_regions(),
        CloudCommand::Up(a) => up(a).await,
        CloudCommand::List => list().await,
        CloudCommand::Cleanup(a) => cleanup(a).await,
        CloudCommand::Render(a) => render(a),
    }
}

/// Write what `tvpn cloud up` would send to a new server, plus the matching
/// client config, without touching any cloud account.
fn render(args: CloudRenderArgs) -> Result<i32> {
    let plan = bootstrap::Plan::generate();
    let user_data = plan.user_data(&bootstrap::Options {
        version: env!("CARGO_PKG_VERSION"),
        repo: cloud::release_repo(),
        max_minutes: 8 * 60,
        idle_minutes: 15,
    });
    let dir = &args.dir;
    let client = plan.client_config(args.server_ip, "tvpn cloud (render)");
    let files = [
        (dir.join("user-data.sh"), user_data),
        (dir.join("server").join("server.conf"), plan.server.to_wg_string()),
        (dir.join("server").join("server.json"), serde_json::to_string_pretty(&plan.meta)?),
        (dir.join("client.conf"), client.to_wg_string()),
    ];
    for (path, contents) in &files {
        crate::paths::write_private(path, contents.as_bytes())?;
        println!("{}", path.display());
    }
    Ok(0)
}

/// Run blocking AWS calls off the async runtime.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| anyhow!("background task failed: {e}"))?
}

fn region_label(r: &Region) -> String {
    if r.country.is_empty() { r.code.clone() } else { format!("{}, {}", r.city, r.country) }
}

async fn setup() -> Result<i32> {
    let term = console::Term::stderr();
    if !term.is_term() {
        bail!(
            "run `tvpn cloud setup` in an interactive terminal, or set AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY"
        );
    }
    println!("{}", ui::bold("tvpn cloud setup"));
    println!();
    println!("tvpn cloud starts a short-lived VPN server in the region you choose, on your own AWS");
    println!("account, and deletes it when you disconnect. New AWS accounts get Free Tier credits");
    println!("(https://aws.amazon.com/free); after that, a server costs roughly 1-2 cents per hour,");
    println!("and only while you are connected.");
    println!();
    println!("1. Create an AWS account at https://aws.amazon.com if you don't have one.");
    println!("2. In the AWS console open IAM > Users > Create user (for example \"tvpn\").");
    println!("3. Open the user > Permissions > Add permissions > Create inline policy > JSON, and paste:");
    println!();
    for line in aws::IAM_POLICY.lines() {
        println!("     {}", ui::cyan(line));
    }
    println!();
    println!("4. Open Security credentials > Create access key > \"Command Line Interface (CLI)\".");
    println!();

    term.write_str("Access key ID: ")?;
    let id = term.read_line()?.trim().to_string();
    term.write_str("Secret access key (hidden): ")?;
    let secret = term.read_secure_line()?.trim().to_string();
    if id.is_empty() || secret.is_empty() {
        bail!("both the access key ID and the secret access key are needed");
    }
    let creds = Credentials {
        access_key_id: id,
        secret_access_key: secret,
        session_token: None,
        source: "setup".into(),
    };

    println!("Checking the keys with AWS…");
    let account = blocking({
        let creds = creds.clone();
        move || Aws::new(creds, "us-east-1").account_id()
    })
    .await
    .context("AWS rejected these keys")?;
    let path = creds.save()?;
    ui::success(format!("Keys work (AWS account {account}) and were saved to {}", path.display()));
    println!();
    ui::hint("Next: `tvpn cloud regions` to pick a place, then `tvpn cloud up <region>`.");
    Ok(0)
}

fn list_regions() -> Result<i32> {
    let rows: Vec<(String, Vec<String>)> = regions::all()
        .into_iter()
        .map(|r| {
            let note =
                if r.opt_in { ui::dim("opt-in: enable it in your AWS account first") } else { String::new() };
            (String::new(), vec![regions::short_name(&r.code), r.city, r.country, r.code, note])
        })
        .collect();
    ui::table(&["NAME", "CITY", "COUNTRY", "AWS REGION", ""], &rows);
    println!();
    ui::hint(
        "Connect with `tvpn cloud up <name>` (city names, country codes and AWS region codes all work).",
    );
    Ok(0)
}

async fn up(args: CloudUpArgs) -> Result<i32> {
    let region = regions::resolve(&args.region)?;
    // Fail fast (before any sudo prompt) when there are no credentials.
    Credentials::load()?;
    privilege::ensure_elevated(!args.no_sudo)?;
    let creds = Credentials::load()?;
    let mut shutdown = Shutdown::install();
    up::ensure_no_client_session().await?;

    let label = region_label(&region);
    let version = args.server_version.clone().unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string());
    let plan = bootstrap::Plan::generate();
    let user_data = plan.user_data(&bootstrap::Options {
        version: &version,
        repo: cloud::release_repo(),
        max_minutes: args.max_hours * 60,
        idle_minutes: args.idle_minutes,
    });
    let aws = Aws::new(creds, &region.code);
    let quiet = args.quiet;
    let step = |msg: String| {
        if !quiet {
            println!("  {} {msg}", ui::dim("›"));
        }
    };

    if !quiet {
        println!(
            "{} {}{}server in {}",
            ui::bold("tvpn cloud"),
            env!("CARGO_PKG_VERSION"),
            ui::DOT,
            ui::cyan(&label)
        );
    }
    if region.opt_in {
        ui::hint(format!(
            "{} is an opt-in region: it must be enabled in your AWS account first",
            region.code
        ));
    }

    step(format!("preparing {} (security group, Ubuntu image)…", region.code));
    let (group, image) = blocking({
        let aws = aws.clone();
        move || Ok((aws.ensure_security_group(bootstrap::LISTEN_PORT)?, aws.ubuntu_image()?))
    })
    .await?;
    if shutdown.triggered() {
        return Ok(up::CANCELLED);
    }

    step(format!("starting a {} server…", args.instance_type));
    let name = format!("tvpn-{}-{}", regions::short_name(&region.code), control::now_unix());
    let instance_id = blocking({
        let aws = aws.clone();
        let instance_type = args.instance_type.clone();
        move || aws.run_instance(&image, &instance_type, &group, &user_data, &name)
    })
    .await?;
    let launched = Instant::now();

    // From here on the server must be deleted, whatever happens.
    let outcome = connect_through(&aws, &instance_id, &plan, &region, &args, &mut shutdown).await;

    step(format!("deleting the server {instance_id}…"));
    let deleted = blocking({
        let aws = aws.clone();
        let id = instance_id.clone();
        move || aws.terminate(&id)
    })
    .await;
    match deleted {
        Ok(()) => ui::success(format!(
            "Cloud server deleted {}",
            ui::dim(format!("(it ran for {})", ui::duration(launched.elapsed().as_secs())))
        )),
        Err(e) => ui::warn(format!(
            "could not delete the server {instance_id} in {}: {e:#}\n  \
             it deletes itself after {} minutes without a client; or run `tvpn cloud cleanup`",
            region.code, args.idle_minutes
        )),
    }
    outcome
}

async fn connect_through(
    aws: &Aws,
    instance_id: &str,
    plan: &bootstrap::Plan,
    region: &Region,
    args: &CloudUpArgs,
    shutdown: &mut Shutdown,
) -> Result<i32> {
    let Some(ip) = wait_for_public_ip(aws, instance_id, shutdown).await? else {
        return Ok(up::CANCELLED);
    };
    if !args.quiet {
        println!("  {} server {instance_id} is running at {ip}", ui::dim("›"));
    }
    let label = region_label(region);
    let server = CloudServer {
        provider: "aws".into(),
        region: region.code.clone(),
        instance_id: instance_id.to_string(),
        public_ip: Some(ip),
        launched_at: control::now_unix(),
    };
    up::connect(
        Connect {
            name: format!("cloud-{}", regions::short_name(&region.code)),
            config: plan.client_config(ip, &format!("tvpn cloud ({label})")),
            no_dns: args.no_dns,
            interface: None,
            quiet: args.quiet,
            wait_for_server: Some(Duration::from_secs(300)),
            cloud: Some(server),
        },
        shutdown,
    )
    .await
}

/// Poll until the instance runs and has a public IP. `None` = cancelled by the user.
async fn wait_for_public_ip(aws: &Aws, id: &str, shutdown: &mut Shutdown) -> Result<Option<IpAddr>> {
    let start = Instant::now();
    loop {
        if shutdown.triggered() {
            return Ok(None);
        }
        let result = blocking({
            let aws = aws.clone();
            let id = id.to_string();
            move || aws.instance(&id)
        })
        .await;
        match result {
            Ok(i) if i.state == "running" && i.public_ip.is_some() => return Ok(i.public_ip),
            Ok(i) if matches!(i.state.as_str(), "shutting-down" | "terminated" | "stopping" | "stopped") => {
                bail!("the server stopped unexpectedly (state: {})", i.state)
            }
            Ok(_) => {}
            // New instances can take a moment to become visible to DescribeInstances.
            Err(e)
                if start.elapsed() < Duration::from_secs(30)
                    && (format!("{e:#}").contains("InvalidInstanceID")
                        || format!("{e:#}").contains("not found")) => {}
            Err(e) => return Err(e),
        }
        if start.elapsed() > Duration::from_secs(180) {
            bail!("the server did not start within 3 minutes");
        }
        tokio::select! {
            _ = tokio::time::sleep(Duration::from_secs(2)) => {}
            _ = shutdown.wait() => {}
        }
    }
}

/// Find tvpn servers in every region (regions not enabled for the account are skipped).
async fn scan(creds: Credentials) -> (Vec<Instance>, Vec<String>) {
    let mut tasks = Vec::new();
    for region in regions::all() {
        let creds = creds.clone();
        tasks.push((
            region.clone(),
            tokio::task::spawn_blocking(move || Aws::new(creds, &region.code).tvpn_instances()),
        ));
    }
    let mut found = Vec::new();
    let mut errors = Vec::new();
    for (region, task) in tasks {
        match task.await {
            Ok(Ok(list)) => found.extend(list),
            Ok(Err(e)) => {
                let msg = format!("{e:#}");
                let not_enabled = msg.contains("OptInRequired") || msg.contains("AuthFailure");
                if !(region.opt_in && not_enabled) {
                    errors.push(format!("{}: {}", region.code, msg.lines().next().unwrap_or_default()));
                }
            }
            Err(e) => errors.push(format!("{}: {e}", region.code)),
        }
    }
    found.sort_by(|a, b| (&a.region, &a.id).cmp(&(&b.region, &b.id)));
    (found, errors)
}

fn print_servers(servers: &[Instance], in_use: Option<&str>) {
    let rows: Vec<(String, Vec<String>)> = servers
        .iter()
        .map(|i| {
            let marker = if Some(i.id.as_str()) == in_use { ui::green(ui::ON) } else { String::new() };
            (
                marker,
                vec![
                    i.id.clone(),
                    format!("{} ({})", regions::short_name(&i.region), i.region),
                    i.state.clone(),
                    i.public_ip.map(|ip| ip.to_string()).unwrap_or_else(|| "-".into()),
                    i.launch_time.clone().unwrap_or_default().replace(".000Z", "Z"),
                ],
            )
        })
        .collect();
    ui::table(&["SERVER", "REGION", "STATE", "IP", "STARTED (UTC)"], &rows);
}

fn active_cloud_instance() -> Option<String> {
    SessionState::load(Mode::Client).and_then(|s| s.cloud).map(|c| c.instance_id)
}

async fn list() -> Result<i32> {
    let creds = Credentials::load()?;
    println!("Checking all regions…");
    let (servers, errors) = scan(creds).await;
    for e in &errors {
        ui::warn(e);
    }
    if servers.is_empty() {
        println!("{}No tvpn cloud servers are running.", ui::OFF);
        return Ok(0);
    }
    print_servers(&servers, active_cloud_instance().as_deref());
    println!();
    ui::hint("Delete leftovers with `tvpn cloud cleanup`.");
    Ok(0)
}

async fn cleanup(args: CloudCleanupArgs) -> Result<i32> {
    let creds = Credentials::load()?;
    println!("Checking all regions…");
    let (servers, errors) = scan(creds.clone()).await;
    for e in &errors {
        ui::warn(e);
    }
    let in_use = active_cloud_instance();
    let (busy, targets): (Vec<Instance>, Vec<Instance>) =
        servers.into_iter().partition(|i| Some(&i.id) == in_use.as_ref());
    for i in &busy {
        ui::hint(format!(
            "keeping {} ({}): your current session uses it; `tvpn down` deletes it",
            i.id, i.region
        ));
    }
    if targets.is_empty() {
        println!("{}Nothing to clean up.", ui::OFF);
        return Ok(0);
    }
    print_servers(&targets, None);
    if !args.yes {
        let term = console::Term::stderr();
        if !term.is_term() {
            bail!("pass --yes to delete without confirmation");
        }
        term.write_str(&format!("Delete these {} server(s)? [y/N] ", targets.len()))?;
        if !matches!(term.read_line()?.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Nothing deleted.");
            return Ok(1);
        }
    }
    let mut failed = 0;
    for i in targets {
        let id = i.id.clone();
        let region = i.region.clone();
        let creds = creds.clone();
        match blocking(move || Aws::new(creds, &region).terminate(&id)).await {
            Ok(()) => ui::success(format!("Deleted {} ({})", i.id, i.region)),
            Err(e) => {
                failed += 1;
                ui::error(format!("{} ({}): {e:#}", i.id, i.region));
            }
        }
    }
    Ok(if failed == 0 { 0 } else { 1 })
}

/// For `tvpn doctor`: where `tvpn cloud` would get credentials from.
pub fn credentials_summary() -> Option<String> {
    Credentials::load().ok().map(|c| c.source)
}

/// For `tvpn status`: a short description of a session's cloud server.
pub fn describe(server: &CloudServer) -> String {
    let ip = server.public_ip.map(|ip| format!(" at {ip}")).unwrap_or_default();
    format!("{} {}{ip} ({})", server.provider.to_uppercase(), server.instance_id, server.region)
}
