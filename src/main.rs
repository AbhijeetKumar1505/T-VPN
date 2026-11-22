use clap::Parser;
use std::time::Duration;
use tvpn::cli::Cli;

fn main() {
    let cli = Cli::parse();

    if cli.no_color || std::env::var_os("NO_COLOR").is_some_and(|v| !v.is_empty()) {
        console::set_colors_enabled(false);
        console::set_colors_enabled_stderr(false);
    }
    init_logging(cli.verbose);

    let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("error: cannot start async runtime: {e}");
            std::process::exit(1);
        }
    };
    let code = match runtime.block_on(tvpn::commands::run(cli)) {
        Ok(code) => code,
        Err(e) => {
            tvpn::ui::error_chain(&e);
            1
        }
    };
    // Don't wait on blocking device reads that may still be parked.
    runtime.shutdown_timeout(Duration::from_millis(500));
    std::process::exit(code);
}

fn init_logging(verbose: u8) {
    use tracing_subscriber::EnvFilter;
    let filter = std::env::var("TVPN_LOG").or_else(|_| std::env::var("RUST_LOG")).unwrap_or_else(|_| {
        match verbose {
            0 => "warn,boringtun=off",
            1 => "info,boringtun=warn",
            2 => "debug,boringtun=info",
            _ => "trace",
        }
        .to_string()
    });
    let _ = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new(filter))
        .with_writer(std::io::stderr)
        .with_target(verbose >= 2)
        .without_time()
        .try_init();
}
