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

mod cache;
mod output;

use std::path::PathBuf;
use std::time::Duration;

use agent_usage_core::{AgentInfo, Budget, FetchOptions, Provider, UsageError};
use chrono::Utc;
use clap::Parser;
use serde::Serialize;
use serde_json::Value;

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

    /// Config directory to resolve this account's default credentials under (supports `~`).
    /// For Claude Code this reads `<DIR>/.credentials.json`, with the Keychain fallback still
    /// applying — the mechanism for a second login (e.g. `~/.claude-personal`).
    #[arg(long, value_name = "DIR")]
    config_dir: Option<String>,

    /// macOS: Keychain account (`security -a`) to disambiguate when one service holds an entry
    /// per login. Omit for the single-account case.
    #[arg(long, value_name = "ACCOUNT")]
    keychain_account: Option<String>,

    /// Override the emitted agent id (and its cache key) for this run. Lets one provider serve a
    /// second account as a distinct agent downstream, e.g. `--id claude-personal`.
    #[arg(long, value_name = "ID")]
    id: Option<String>,

    /// Override the emitted agent display label for this run (e.g. `--label "Claude (personal)"`).
    #[arg(long, value_name = "LABEL")]
    label: Option<String>,

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

    /// Seconds a cached snapshot stays fresh: repeated calls within this window reuse it instead
    /// of re-hitting the usage source. 0 disables reuse (but still serves stale on error).
    #[arg(long, default_value_t = 60, value_name = "SECS")]
    cache_ttl: u64,

    /// Don't read or write the on-disk usage cache at all.
    #[arg(long)]
    no_cache: bool,
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

/// Fetch and render a single agent. Returns 0 on success/stale, 1 on a usage error.
///
/// `--status` always fetches live (humans can retry); JSON output goes through the cache so the
/// app gets dedupe + stale-on-error resilience.
fn run_one(cli: &Cli, provider: &dyn Provider, budget: &Budget) -> i32 {
    let now = Utc::now();
    let id_ovr = cli.id.as_deref();
    let label_ovr = cli.label.as_deref();

    if cli.status {
        let opts = fetch_options(cli);
        return match provider.fetch(&opts) {
            Ok(mut usage) => {
                apply_identity(&mut usage.agent, id_ovr, label_ovr);
                print!("{}", output::render_status(&output::build_snapshot(&usage, budget, now), now));
                0
            }
            Err(err) => {
                let mut info = agent_info(provider);
                apply_identity(&mut info, id_ovr, label_ovr);
                let snap = output::build_error_snapshot(&info, &err, budget, now);
                eprint!("{}", output::render_status(&snap, now));
                1
            }
        };
    }

    let (value, code) = agent_json(cli, provider, budget, now, id_ovr, label_ovr);
    print_json(&value);
    code
}

/// Apply the CLI's `--id`/`--label` overrides to an agent identity, so one provider can serve a
/// second account under a distinct id/label. `source` is left as the provider reports it.
fn apply_identity(info: &mut AgentInfo, id: Option<&str>, label: Option<&str>) {
    if let Some(id) = id {
        info.id = id.to_string();
    }
    if let Some(label) = label {
        info.label = label.to_string();
    }
}

/// Fetch every known agent. JSON form is an array of snapshots; exits 1 if any agent errored.
fn run_all(cli: &Cli, budget: &Budget) -> i32 {
    let now = Utc::now();

    if cli.status {
        let opts = fetch_options(cli);
        let mut any_err = false;
        for provider in agent_usage_providers::all() {
            let snap = match provider.fetch(&opts) {
                Ok(usage) => output::build_snapshot(&usage, budget, now),
                Err(err) => {
                    any_err = true;
                    output::build_error_snapshot(&agent_info(provider.as_ref()), &err, budget, now)
                }
            };
            print!("{}", output::render_status(&snap, now));
        }
        return if any_err { 1 } else { 0 };
    }

    let mut values = Vec::new();
    let mut any_err = false;
    for provider in agent_usage_providers::all() {
        let (value, code) = agent_json(cli, provider.as_ref(), budget, now, None, None);
        if code != 0 {
            any_err = true;
        }
        values.push(value);
    }
    print_json(&values);
    if any_err {
        1
    } else {
        0
    }
}

/// Produce one agent's JSON snapshot as a `serde_json::Value`, applying the cache.
///
/// The cache stores the agent's **raw usage** (not the rendered snapshot), and the snapshot —
/// pace, work-day index, reset countdowns — is recomputed from it on every call against the
/// current `budget` and `now`. So a fresh cache hit still reflects the latest work-days setting
/// and live countdowns; only the underlying usage is reused to avoid re-hitting the source. On a
/// transient fetch error the last cached usage is served, marked `stale`; otherwise an error
/// snapshot is returned (exit code 1).
fn agent_json(
    cli: &Cli,
    provider: &dyn Provider,
    budget: &Budget,
    now: chrono::DateTime<Utc>,
    id_override: Option<&str>,
    label_override: Option<&str>,
) -> (Value, i32) {
    // An id override gives this account its own cache file, so a second Claude login doesn't
    // clobber the primary's cached usage.
    let cache_id = id_override.unwrap_or_else(|| provider.id());
    let use_cache = !cli.no_cache;

    // Fresh cache: recompute the snapshot from cached usage without touching the source.
    if use_cache && cli.cache_ttl > 0 {
        if let Some((age, mut usage)) = read_cached_usage(cache_id) {
            if age < Duration::from_secs(cli.cache_ttl) {
                apply_identity(&mut usage.agent, id_override, label_override);
                let snap = output::build_snapshot(&usage, budget, now);
                return (serde_json::to_value(&snap).unwrap_or(Value::Null), 0);
            }
        }
    }

    let opts = fetch_options(cli);
    match provider.fetch(&opts) {
        Ok(mut usage) => {
            apply_identity(&mut usage.agent, id_override, label_override);
            if use_cache {
                write_cached_usage(cache_id, &usage);
            }
            let snap = output::build_snapshot(&usage, budget, now);
            (serde_json::to_value(&snap).unwrap_or(Value::Null), 0)
        }
        Err(err) => {
            // Serve last good usage on a transient failure, recomputed and marked stale.
            if use_cache && is_transient(&err) {
                if let Some((_, mut usage)) = read_cached_usage(cache_id) {
                    apply_identity(&mut usage.agent, id_override, label_override);
                    let mut snap = output::build_snapshot(&usage, budget, now);
                    snap.stale = Some(true);
                    snap.stale_reason = Some(err.to_string());
                    return (serde_json::to_value(&snap).unwrap_or(Value::Null), 0);
                }
            }
            let mut info = agent_info(provider);
            apply_identity(&mut info, id_override, label_override);
            let snap = output::build_error_snapshot(&info, &err, budget, now);
            (serde_json::to_value(&snap).unwrap_or(Value::Null), 1)
        }
    }
}

fn read_cached_usage(id: &str) -> Option<(Duration, agent_usage_core::Usage)> {
    let (age, contents) = cache::read(id)?;
    let usage = serde_json::from_str(&contents).ok()?;
    Some((age, usage))
}

fn write_cached_usage(id: &str, usage: &agent_usage_core::Usage) {
    if let Ok(s) = serde_json::to_string(usage) {
        cache::write(id, &s);
    }
}

fn agent_info(provider: &dyn Provider) -> AgentInfo {
    AgentInfo {
        id: provider.id().to_string(),
        label: provider.label().to_string(),
        source: provider.source().to_string(),
    }
}

/// Errors worth serving stale data through (a passing blip), vs. ones the user must act on
/// (auth/credentials/unsupported), which should surface as errors.
fn is_transient(err: &UsageError) -> bool {
    matches!(
        err.kind(),
        "network" | "rate_limited" | "unexpected_status" | "parse" | "no_data"
    )
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
        creds_dir: cli.config_dir.as_deref().map(expand_tilde),
        timeout: Duration::from_secs(cli.timeout),
        keychain_service: cli.keychain_service.clone(),
        keychain_account: cli.keychain_account.clone(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn info() -> AgentInfo {
        AgentInfo {
            id: "claude".into(),
            label: "Claude Code".into(),
            source: "Anthropic OAuth usage API".into(),
        }
    }

    #[test]
    fn identity_override_remaps_id_and_label_keeps_source() {
        let mut i = info();
        apply_identity(&mut i, Some("claude-personal"), Some("Claude (personal)"));
        assert_eq!(i.id, "claude-personal");
        assert_eq!(i.label, "Claude (personal)");
        // Source is intrinsic to the provider and must survive an identity override.
        assert_eq!(i.source, "Anthropic OAuth usage API");
    }

    #[test]
    fn identity_override_is_noop_when_unset() {
        let mut i = info();
        apply_identity(&mut i, None, None);
        assert_eq!(i.id, "claude");
        assert_eq!(i.label, "Claude Code");
    }
}
