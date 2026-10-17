//! `fldr`: command-line client for the fldr source-control service.
//!
//! This file is only the clap layer; HTTP and error mapping live in the
//! library ([`fldr_cli::Client`]).

use std::process::ExitCode;

use clap::{Parser, Subcommand};
use fldr_cli::{Client, DEFAULT_HOST, Error, OrDash, format_uptime};
use fldr_core::api::ComponentStatus;

/// Exit code when the server answers but reports itself unhealthy.
const EXIT_NOT_OK: u8 = 1;
/// Exit code for connection, timeout, protocol and usage errors.
const EXIT_ERROR: u8 = 2;
/// Exit code for commands that exist only as placeholders (`EX_USAGE`).
const EXIT_NOT_IMPLEMENTED: u8 = 64;

#[derive(Debug, Parser)]
#[command(
    name = "fldr",
    version,
    about = "Client for fldr, a centralized content-addressed source-control service",
    after_help = "Commands marked [planned] are placeholders and exit with code 64."
)]
struct Cli {
    /// Server base URL (http or https).
    #[arg(long, global = true, env = "FLDR_HOST", default_value = DEFAULT_HOST)]
    host: String,

    /// Print the server's raw JSON response instead of a summary.
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Check server health (exit 0 ok, 1 unhealthy, 2 unreachable/error).
    Health,
    /// Show service info: version, API version and capabilities.
    Info,
    /// [planned] Lay out a repository's files locally.
    Clone(Planned),
    /// [planned] Show local changes against the checked-out changeset.
    Status(Planned),
    /// [planned] Upload changes as a changeset and land it.
    Commit(Planned),
    /// [planned] Show changeset history.
    Log(Planned),
    /// [planned] Show differences between changesets or the workspace.
    Diff(Planned),
    /// [planned] Print a file's contents at a changeset.
    Cat(Planned),
}

/// Placeholder commands accept and ignore any arguments, so trying a
/// future invocation yields "not implemented" instead of a usage error.
#[derive(Debug, clap::Args)]
struct Planned {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, hide = true)]
    _args: Vec<String>,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let name = match &cli.command {
        Command::Health => return run(&cli, health),
        Command::Info => return run(&cli, info),
        Command::Clone(_) => "clone",
        Command::Status(_) => "status",
        Command::Commit(_) => "commit",
        Command::Log(_) => "log",
        Command::Diff(_) => "diff",
        Command::Cat(_) => "cat",
    };
    eprintln!("fldr {name}: not implemented yet (planned for M4)");
    ExitCode::from(EXIT_NOT_IMPLEMENTED)
}

/// Build a client and runtime, run `f`, and map any error to exit code 2.
fn run<F>(cli: &Cli, f: impl FnOnce(Client, bool) -> F) -> ExitCode
where
    F: Future<Output = Result<ExitCode, Error>>,
{
    let result = Client::new(&cli.host).and_then(|client| {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(Error::Runtime)?;
        runtime.block_on(f(client, cli.json))
    });
    result.unwrap_or_else(|e| {
        eprintln!("error: {e}");
        if let Some(cause) = root_cause(&e) {
            eprintln!("  cause: {cause}");
        }
        if let Some(hint) = hint(&e) {
            eprintln!("hint: {hint}");
        }
        ExitCode::from(EXIT_ERROR)
    })
}

async fn health(client: Client, json: bool) -> Result<ExitCode, Error> {
    let fetched = client.health().await?;
    let h = &fetched.value;
    if json {
        println!("{}", fetched.raw.trim_end());
    } else {
        println!(
            "{} {} {} ({})",
            if h.ok { "ok" } else { "NOT OK" },
            h.service,
            h.version,
            client.host()
        );
        println!("  gitSha    {}", OrDash(&h.git_sha));
        println!("  db        {}", status(h.db));
        println!("  schema    {}", OrDash(&h.schema));
        println!("  blobs     {}", status(h.blobs));
        println!("  volumeId  {}", OrDash(&h.volume_id));
        println!("  uptime    {}", format_uptime(h.uptime_secs));
    }
    Ok(if h.ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(EXIT_NOT_OK)
    })
}

async fn info(client: Client, json: bool) -> Result<ExitCode, Error> {
    let fetched = client.info().await?;
    let i = &fetched.value;
    if json {
        println!("{}", fetched.raw.trim_end());
    } else {
        println!("{} {} ({})", i.service, i.version, client.host());
        println!("  apiVersion    {}", i.api_version);
        let caps = if i.capabilities.is_empty() {
            "-".to_owned()
        } else {
            i.capabilities.join(", ")
        };
        println!("  capabilities  {caps}");
    }
    Ok(ExitCode::SUCCESS)
}

fn status(s: ComponentStatus) -> &'static str {
    match s {
        ComponentStatus::Ok => "ok",
        ComponentStatus::Error => "error",
        ComponentStatus::Memory => "memory (in-memory fallback, dev only)",
    }
}

/// The innermost error in `e`'s source chain. For transport errors this is
/// the useful part ("Connection refused", a DNS or TLS failure), which
/// reqwest's own message omits.
fn root_cause(e: &Error) -> Option<&dyn std::error::Error> {
    let mut cause = std::error::Error::source(e)?;
    while let Some(next) = cause.source() {
        cause = next;
    }
    Some(cause)
}

fn hint(e: &Error) -> Option<&'static str> {
    match e {
        Error::Connect { .. } | Error::Timeout { .. } => {
            Some("check that the server is running, or point --host / FLDR_HOST elsewhere")
        }
        Error::Unexpected { .. } => {
            Some("the host answered but not like a fldr server; check --host / FLDR_HOST")
        }
        _ => None,
    }
}
