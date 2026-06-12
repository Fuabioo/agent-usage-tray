//! `agent-usage` — one CLI, one output contract, for every coding agent's usage budget.
//!
//! Usage:
//!   agent-usage claude            # JSON snapshot for one agent (the default output)
//!   agent-usage claude --status   # human-readable report
//!   agent-usage all               # JSON array: every known agent
//!   agent-usage list              # list available agents and their sources
//!
//! The output shape is identical for every agent (see `output::Snapshot`); only the provider
//! behind a given subcommand differs. On failure the CLI still prints a valid JSON document
//! carrying an `error` object and exits non-zero, so GUI callers can always parse the result.

mod output;

use std::path::PathBuf;
use std::time::Duration;

use agent_usage_core::{AgentInfo, Budget, FetchOptions, Provider};
use chrono::Utc;
use clap::Parser;
use serde::Serialize;

#[derive(Parser, Debug)]
#[command(
    name = "agent-usage",
    about = "Monitor any coding agent's usage budget (one JSON contract for all agents)",
    version
)]
struct Cli {
    /// Which agent to query: an agent id (e.g. `claude`, `codex`), `all` for every known
    /// agent, or `list` to list available agents.
    #[arg(value_name = "AGENT")]
    agent: String,

    /// Human-readable report instead of JSON.
    #[arg(long, conflicts_with = "json")]
    status: bool,

    /// Force JSON output (the default).
    #[arg(long)]
    json: bool,

    /// Path to a credentials file, overriding the agent's default location (supports `~`).
    #[arg(long, value_name = "PATH")]
    creds_path: Option<String>,

    /// Expected usage percentage per work day (default 20.0).
    #[arg(long, value_name = "PCT")]
    daily_budget: Option<f64>,

    /// Number of budget work days per cycle, 1-7 (default 5).
    #[arg(long, value_name = "N")]
    work_days: Option<u8>,

    /// HTTP request timeout in seconds.
    #[arg(long, default_value_t = 30, value_name = "SECS")]
    timeout: u64,

    /// macOS: Keychain generic-password service to read credentials from when no file exists.
    #[arg(long, value_name = "NAME")]
    keychain_service: Option<String>,

    /// Disable the macOS Keychain fallback (only read the credentials file).
    #[arg(long)]
    no_keychain: bool,
}

fn main() {
    let cli = Cli::parse();
    std::process::exit(run(&cli));
}

fn run(cli: &Cli) -> i32 {
    let budget = Budget {
        daily_budget: cli.daily_budget.unwrap_or(Budget::default().daily_budget),
        work_days: cli.work_days.unwrap_or(Budget::default().work_days),
    }
    .validated();

    match cli.agent.as_str() {
        "list" => {
            print_list(cli.status);
            0
        }
        "all" => run_all(cli, &budget),
        id => match agent_usage_providers::get(id) {
            Some(provider) => run_one(cli, provider.as_ref(), &budget),
            None => {
                eprintln!(
                    "error: unknown agent '{id}'. Known agents: {}. Try `agent-usage list`.",
                    agent_usage_providers::ids().join(", ")
                );
                2
            }
        },
    }
}

/// Fetch and render a single agent. Returns 0 on success, 1 on a usage error.
fn run_one(cli: &Cli, provider: &dyn Provider, budget: &Budget) -> i32 {
    let now = Utc::now();
    let opts = fetch_options(cli);

    match provider.fetch(&opts) {
        Ok(usage) => {
            let snap = output::build_snapshot(&usage, budget, now);
            if cli.status {
                print!("{}", output::render_status(&snap, now));
            } else {
                print_json(&snap);
            }
            0
        }
        Err(err) => {
            let info = AgentInfo {
                id: provider.id().to_string(),
                label: provider.label().to_string(),
                source: provider.source().to_string(),
            };
            let snap = output::build_error_snapshot(&info, &err, budget, now);
            if cli.status {
                eprint!("{}", output::render_status(&snap, now));
            } else {
                print_json(&snap);
            }
            1
        }
    }
}

/// Fetch every known agent. JSON form is an array of snapshots; exits 1 if any agent errored.
fn run_all(cli: &Cli, budget: &Budget) -> i32 {
    let now = Utc::now();
    let opts = fetch_options(cli);

    let mut snapshots = Vec::new();
    let mut any_err = false;

    for provider in agent_usage_providers::all() {
        let snap = match provider.fetch(&opts) {
            Ok(usage) => output::build_snapshot(&usage, budget, now),
            Err(err) => {
                any_err = true;
                let info = AgentInfo {
                    id: provider.id().to_string(),
                    label: provider.label().to_string(),
                    source: provider.source().to_string(),
                };
                output::build_error_snapshot(&info, &err, budget, now)
            }
        };
        snapshots.push(snap);
    }

    if cli.status {
        for snap in &snapshots {
            print!("{}", output::render_status(snap, now));
        }
    } else {
        print_json(&snapshots);
    }

    if any_err {
        1
    } else {
        0
    }
}

#[derive(Serialize)]
struct AgentListEntry {
    id: &'static str,
    label: &'static str,
    source: &'static str,
}

fn print_list(human: bool) {
    let entries: Vec<AgentListEntry> = agent_usage_providers::all()
        .iter()
        .map(|p| AgentListEntry {
            id: p.id(),
            label: p.label(),
            source: p.source(),
        })
        .collect();

    if human {
        for e in &entries {
            println!("{:<10} {:<14} — {}", e.id, e.label, e.source);
        }
    } else {
        print_json(&entries);
    }
}

fn fetch_options(cli: &Cli) -> FetchOptions {
    FetchOptions {
        creds_path: cli.creds_path.as_deref().map(expand_tilde),
        timeout: Duration::from_secs(cli.timeout),
        keychain_service: cli.keychain_service.clone(),
        no_keychain: cli.no_keychain,
    }
}

/// Expand `~` in a user-provided creds path. Kept here (not in core) since it's a CLI concern.
fn expand_tilde(path: &str) -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if path == "~" {
            return PathBuf::from(home);
        }
        if let Some(rest) = path.strip_prefix("~/") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn print_json<T: Serialize>(value: &T) {
    match serde_json::to_string_pretty(value) {
        Ok(s) => println!("{s}"),
        Err(e) => eprintln!("error: failed to serialize JSON: {e}"),
    }
}
