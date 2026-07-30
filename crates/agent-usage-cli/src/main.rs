//! `agent-usage` — one CLI, one output contract, for every coding agent's usage budget.
//!
//! Usage:
//!   agent-usage claude            # JSON snapshot for one agent (the default output)
//!   agent-usage claude --status   # human-readable report
//!   agent-usage all               # JSON array: every default agent (opt-in agents join once configured)
//!   agent-usage list              # list default agents and their sources
//!   agent-usage config            # show stored settings (`--save` writes them)
//!
//! The output shape is identical for every agent (see `output::Snapshot`); only the provider
//! behind a given subcommand differs. On failure the CLI still prints a valid JSON document
//! carrying an `error` object and exits non-zero, so GUI callers can always parse the result.
//!
//! Settings resolve **flag → environment → config file → built-in default**, so the config file
//! (`~/.config/agent-usage/config.json`, see [`agent_usage_core::Config`]) is the CLI's own
//! baseline while a frontend passing its settings as arguments keeps overriding it.

mod cache;
mod history;
mod output;

use std::path::PathBuf;
use std::time::Duration;

use agent_usage_core::{
    AgentInfo, Budget, Config, FetchOptions, Provider, Trend, Usage, UsageError, WindowKind,
};
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
    /// Which agent to query: an agent id (e.g. `claude`, `codex`), `all` for every default
    /// agent (opt-in agents like `hyper` join once their credentials are configured), `list` to
    /// list them, or `config` to show/update stored settings. A specific id always works, even
    /// for an opt-in agent.
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

    /// Expected usage percentage per work day. Defaults to one cycle split across `--work-days`
    /// (5 work days -> 20.0).
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

    /// When the daily cycle resets, for agents whose API doesn't report it (currently `hyper`).
    /// `HH:MM` plus an optional zone — UTC when omitted, or `Z`, `local`, or `±HH:MM`
    /// (e.g. `08:00 local`). Overrides `HYPER_RESET_TIME`.
    #[arg(long, value_name = "HH:MM[ ZONE]")]
    reset_time: Option<String>,

    /// Size of the credit pool, for agents whose API reports only a balance (currently `hyper`):
    /// permanent credits plus the daily grant. Left unset the provider infers it from successive
    /// balances; set it when you know the number and the inference has drifted. An empty value
    /// clears the stored setting and goes back to inferring.
    #[arg(long, value_name = "N")]
    total_credits: Option<CreditsArg>,

    /// Seconds a cached snapshot stays fresh: repeated calls within this window reuse it instead
    /// of re-hitting the usage source. 0 disables reuse (but still serves stale on error).
    #[arg(long, default_value_t = 60, value_name = "SECS")]
    cache_ttl: u64,

    /// Don't read or write the on-disk usage cache at all.
    #[arg(long)]
    no_cache: bool,

    /// With the `config` subcommand: write the given settings to the config file instead of
    /// printing it. Only the settings you pass are changed; the rest of the file is preserved.
    #[arg(long)]
    save: bool,

    /// With `config --save`: store the Charm Hyper API key. Pass `-` to read it from stdin, which
    /// is how a frontend should send it — a value passed as an argument is visible to every
    /// process on the machine via `ps`. Pass an empty value to remove the stored key.
    #[arg(long, value_name = "KEY")]
    hyper_api_key: Option<String>,
}

/// A `--total-credits` value: a whole number of credits, or empty to mean "no opinion" (which
/// under `config --save` clears the stored setting, the same way an empty `--reset-time` does).
///
/// Parsed by clap rather than downstream so a typo fails at the argument, once and loudly, instead
/// of quietly reverting to the inferred pool — the same reason a malformed reset time is a hard
/// error rather than a fallback to midnight.
#[derive(Debug, Clone, PartialEq, Eq)]
struct CreditsArg(Option<u32>);

impl std::str::FromStr for CreditsArg {
    type Err = String;

    fn from_str(raw: &str) -> Result<Self, String> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return Ok(CreditsArg(None));
        }
        trimmed
            .parse::<u32>()
            .map(|n| CreditsArg(Some(n)))
            .map_err(|_| {
                format!("expected a whole number of credits (or an empty value to clear), got {raw:?}")
            })
    }
}

fn main() {
    let cli = Cli::parse();
    std::process::exit(run(&cli));
}

fn run(cli: &Cli) -> i32 {
    // Read the config file first: it is the baseline every other source overrides. A broken one
    // is fatal rather than silently ignored — see `Config::load`.
    let config = match Config::load() {
        Ok(c) => c,
        Err(err) => {
            eprintln!("error: {err}");
            return 2;
        }
    };

    let budget = Budget {
        daily_budget: resolve_daily_budget(cli, &config),
        work_days: resolve_work_days(cli, &config),
    }
    .validated();

    match cli.agent.as_str() {
        "list" => {
            print_list(cli, &config);
            0
        }
        "config" => run_config(cli, config),
        "all" => run_all(cli, &budget, &config),
        id => match agent_usage_providers::get(id) {
            Some(provider) => run_one(cli, provider.as_ref(), &budget, &config),
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

/// `agent-usage config` — show the stored config, or with `--save` update it.
///
/// The CLI owns reading *and* writing the file so a frontend never has to reproduce its path
/// rules or its schema; getting either wrong would be silent (a file nobody reads) or fatal
/// (an unknown key, which is a hard error by design).
fn run_config(cli: &Cli, mut config: Config) -> i32 {
    if !cli.save {
        return print_config(cli, &config);
    }

    let patch = match config_patch(cli) {
        Ok(p) => p,
        Err(err) => {
            eprintln!("error: {err}");
            return 2;
        }
    };
    config.merge(patch);

    match config.save() {
        Ok(path) => {
            if cli.status {
                println!("saved {}", path.display());
            }
            print_config(cli, &config)
        }
        Err(err) => {
            eprintln!("error: {err}");
            2
        }
    }
}

/// Build the update from the flags. An explicitly empty value clears a setting — a bare absence
/// has to mean "leave alone", so it cannot also mean "remove".
fn config_patch(cli: &Cli) -> Result<agent_usage_core::config::Patch, UsageError> {
    let mut patch = agent_usage_core::config::Patch {
        work_days: cli.work_days,
        daily_budget: cli.daily_budget,
        ..Default::default()
    };

    if let Some(raw) = cli.reset_time.as_deref() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            patch.clear_reset_time = true;
        } else {
            patch.reset_time = Some(trimmed.to_string());
        }
    }

    if let Some(CreditsArg(value)) = cli.total_credits {
        match value {
            Some(n) => patch.total_credits = Some(n),
            None => patch.clear_total_credits = true,
        }
    }

    if let Some(raw) = cli.hyper_api_key.as_deref() {
        // `-` means "the key is on stdin", so a secret never lands in this process's argv where
        // any user on the machine could read it out of `ps`.
        let value = if raw == "-" {
            let mut buf = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut buf)
                .map_err(|e| UsageError::Unsupported(format!("could not read key from stdin: {e}")))?;
            buf
        } else {
            raw.to_string()
        };
        let trimmed = value.trim();
        if trimmed.is_empty() {
            patch.clear_hyper_api_key = true;
        } else {
            patch.hyper_api_key = Some(trimmed.to_string());
        }
    }

    Ok(patch)
}

/// Print the config. The API key is only ever reported as present or absent — printing it would
/// put a secret into terminal scrollback and any log that captured this output.
fn print_config(cli: &Cli, config: &Config) -> i32 {
    let path = agent_usage_core::config_path();
    if cli.status {
        match &path {
            Some(p) if p.exists() => println!("config       = {}", p.display()),
            Some(p) => println!("config       = {} (not created yet)", p.display()),
            None => println!("config       = <no config directory>"),
        }
        println!(
            "work_days    = {}",
            config.work_days.map_or("<unset>".into(), |v| v.to_string())
        );
        println!(
            "daily_budget = {}",
            config
                .daily_budget
                .map_or("<unset>".into(), |v| format!("{v}"))
        );
        println!(
            "reset_time   = {}",
            config.reset_time.clone().unwrap_or_else(|| "<unset>".into())
        );
        println!(
            "total_credits= {}",
            config
                .total_credits
                .map_or("<unset>".into(), |v| v.to_string())
        );
        println!(
            "hyper_api_key= {}",
            if config.hyper_api_key.is_some() {
                "<set>"
            } else {
                "<unset>"
            }
        );
        return 0;
    }

    let doc = serde_json::json!({
        "path": path.as_ref().map(|p| p.display().to_string()),
        "exists": path.as_ref().is_some_and(|p| p.exists()),
        "work_days": config.work_days,
        "daily_budget": config.daily_budget,
        "reset_time": config.reset_time,
        "total_credits": config.total_credits,
        "hyper_api_key_set": config.hyper_api_key.is_some(),
    });
    print_json(&doc);
    0
}

/// Fetch and render a single agent. Returns 0 on success/stale, 1 on a usage error.
///
/// `--status` always fetches live (humans can retry); JSON output goes through the cache so the
/// app gets dedupe + stale-on-error resilience.
fn run_one(cli: &Cli, provider: &dyn Provider, budget: &Budget, config: &Config) -> i32 {
    let now = Utc::now();
    let id_ovr = cli.id.as_deref();
    let label_ovr = cli.label.as_deref();

    if cli.status {
        let opts = fetch_options(cli, config);
        return match provider.fetch(&opts) {
            Ok(mut usage) => {
                apply_identity(&mut usage.agent, id_ovr, label_ovr);
                let trend = track_trend(cli, &usage.agent.id, &usage, now, true);
                let snap = output::build_snapshot(&usage, budget, now).with_trend(trend, budget);
                print!("{}", output::render_status(&snap, now));
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

    let (value, code) = agent_json(cli, provider, budget, config, now, id_ovr, label_ovr);
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

/// Fetch every default agent (opt-in agents join once configured; see `Provider::in_default_set`).
/// JSON form is an array of snapshots; exits 1 if any agent errored.
fn run_all(cli: &Cli, budget: &Budget, config: &Config) -> i32 {
    let now = Utc::now();

    if cli.status {
        let opts = fetch_options(cli, config);
        let mut any_err = false;
        for provider in agent_usage_providers::all()
            .into_iter()
            .filter(|p| p.in_default_set(&opts))
        {
            let snap = match provider.fetch(&opts) {
                Ok(usage) => {
                    let trend = track_trend(cli, &usage.agent.id, &usage, now, true);
                    output::build_snapshot(&usage, budget, now).with_trend(trend, budget)
                }
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
    let default_set_opts = fetch_options(cli, config);
    for provider in agent_usage_providers::all()
        .into_iter()
        .filter(|p| p.in_default_set(&default_set_opts))
    {
        let (value, code) = agent_json(cli, provider.as_ref(), budget, config, now, None, None);
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
    config: &Config,
    now: chrono::DateTime<Utc>,
    id_override: Option<&str>,
    label_override: Option<&str>,
) -> (Value, i32) {
    // An id override gives this account its own cache file, so a second Claude login doesn't
    // clobber the primary's cached usage.
    let cache_id = id_override.unwrap_or_else(|| provider.id());
    let use_cache = !cli.no_cache;

    // Fresh cache: recompute the snapshot from cached usage without touching the source.
    // The trend is read but not appended to — a cache hit is the same reading as before, and
    // recording it again would flatten the burn rate with samples that carry no new information.
    if use_cache && cli.cache_ttl > 0 {
        if let Some((age, mut usage)) = read_cached_usage(cache_id) {
            if age < Duration::from_secs(cli.cache_ttl) {
                apply_identity(&mut usage.agent, id_override, label_override);
                let trend = track_trend(cli, cache_id, &usage, now, false);
                let snap = output::build_snapshot(&usage, budget, now)
                    .with_fetched_at(fetched_at(now, age))
                    .with_trend(trend, budget);
                return (serde_json::to_value(&snap).unwrap_or(Value::Null), 0);
            }
        }
    }

    let opts = fetch_options(cli, config);
    match provider.fetch(&opts) {
        Ok(mut usage) => {
            apply_identity(&mut usage.agent, id_override, label_override);
            if use_cache {
                write_cached_usage(cache_id, &usage);
            }
            let trend = track_trend(cli, cache_id, &usage, now, true);
            let snap = output::build_snapshot(&usage, budget, now).with_trend(trend, budget);
            (serde_json::to_value(&snap).unwrap_or(Value::Null), 0)
        }
        Err(err) => {
            // Serve last good usage on a transient failure, recomputed and marked stale.
            if use_cache && is_transient(&err) {
                if let Some((age, mut usage)) = read_cached_usage(cache_id) {
                    apply_identity(&mut usage.agent, id_override, label_override);
                    // Stale data is a repeat of an old reading, so it is read-only here too.
                    let trend = track_trend(cli, cache_id, &usage, now, false);
                    let mut snap = output::build_snapshot(&usage, budget, now)
                        .with_fetched_at(fetched_at(now, age))
                        .with_trend(trend, budget);
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

/// Track an agent's multi-day window over time and report what the samples say about its pace.
///
/// Only multi-day windows are tracked: a short rolling window already brakes on its own, and it
/// is the agents that bill *one* multi-day quota — with nothing stopping a single sitting from
/// eating the whole cycle — that need burn rate and burst to be reconstructed from history.
///
/// `record` distinguishes a live reading (append it) from a replayed one (a cache hit or stale
/// fallback: read the series, don't grow it). `--no-cache` opts out of the on-disk series
/// entirely, so no trend is reported — consistent with it meaning "touch no state".
fn track_trend(
    cli: &Cli,
    id: &str,
    usage: &Usage,
    now: chrono::DateTime<Utc>,
    record: bool,
) -> Option<Trend> {
    if cli.no_cache {
        return None;
    }
    let window = usage
        .windows
        .iter()
        .find(|w| w.kind == WindowKind::Weekly)?;

    let mut series = history::load(id);
    if record {
        series.record(now, window.used_pct());
        history::store(id, &series);
    }
    series.trend(now, window.used_pct(), window.resets_at)
}

/// When a cached reading was taken: `now` minus its age on disk.
///
/// Saturates at `now` rather than propagating a conversion failure — an age that won't convert
/// (negative, absurdly large) means the clock or the file's mtime is untrustworthy, and claiming
/// the reading is current is the *optimistic* answer, so it is the one to avoid. Falling back to
/// `now` here would restore exactly the overstated freshness this exists to fix, so an unusable
/// age reports the reading as maximally old instead.
fn fetched_at(now: chrono::DateTime<Utc>, age: Duration) -> chrono::DateTime<Utc> {
    match chrono::Duration::from_std(age) {
        Ok(d) => now - d,
        Err(_) => chrono::DateTime::<Utc>::MIN_UTC,
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

fn print_list(cli: &Cli, config: &Config) {
    let opts = fetch_options(cli, config);
    let entries: Vec<AgentListEntry> = agent_usage_providers::all()
        .iter()
        .filter(|p| p.in_default_set(&opts))
        .map(|p| AgentListEntry {
            id: p.id(),
            label: p.label(),
            source: p.source(),
        })
        .collect();

    if cli.status {
        for e in &entries {
            println!("{:<10} {:<14} — {}", e.id, e.label, e.source);
        }
    } else {
        print_json(&entries);
    }
}

/// Settings resolve **flag → environment → config file → built-in default**.
///
/// The config file is the baseline, so a frontend that passes its own settings as arguments keeps
/// overriding it exactly as before. The environment sits between the two because it is the more
/// specific, more ephemeral of the two ambient sources — and because that ordering preserves what
/// an existing `HYPER_RESET_TIME` export already did.
fn resolve_work_days(cli: &Cli, config: &Config) -> u8 {
    cli.work_days
        .or(config.work_days)
        .unwrap_or(Budget::default().work_days)
}

/// Unspecified, the daily budget is **derived from the work days** — one cycle split evenly, the
/// same `100 / work_days` the macOS app applies to its own slider. A flat default would silently
/// under-budget any non-default split: `work_days = 4` at 20%/day only ever spends 80% of the
/// cycle, so the pace ceiling never reaches the budget the user actually has. For the default of
/// 5 work days this is 20.0, exactly as before.
fn resolve_daily_budget(cli: &Cli, config: &Config) -> f64 {
    if let Some(explicit) = cli.daily_budget.or(config.daily_budget) {
        return explicit;
    }
    let work_days = resolve_work_days(cli, config).clamp(1, 7);
    100.0 / work_days as f64
}

fn resolve_reset_time(cli: &Cli, config: &Config) -> Option<String> {
    resolve_reset_time_from(
        cli.reset_time.clone(),
        std::env::var("HYPER_RESET_TIME").ok(),
        config.reset_time.clone(),
    )
}

/// Pure resolution from the three sources, so it can be tested without depending on (or mutating)
/// process-global environment state — the same reason `core::cache::resolve_cache_dir` is split
/// this way. Blank values are treated as absent at every level, so a frontend can pass an empty
/// field through without it either erroring or shadowing the config file.
fn resolve_reset_time_from(
    flag: Option<String>,
    env: Option<String>,
    config: Option<String>,
) -> Option<String> {
    [flag, env, config]
        .into_iter()
        .flatten()
        .map(|s| s.trim().to_string())
        .find(|s| !s.is_empty())
}

/// The credit-pool size: the flag first, then the config file, else `None` (leave the provider to
/// infer it). No environment layer — unlike `reset_time` there is no exported variable predating
/// the config file whose meaning has to be preserved.
fn resolve_total_credits(cli: &Cli, config: &Config) -> Option<u32> {
    cli.total_credits
        .as_ref()
        .and_then(|c| c.0)
        .or(config.total_credits)
}

fn fetch_options(cli: &Cli, config: &Config) -> FetchOptions {
    FetchOptions {
        creds_path: cli.creds_path.as_deref().map(expand_tilde),
        creds_dir: cli.config_dir.as_deref().map(expand_tilde),
        timeout: Duration::from_secs(cli.timeout),
        keychain_service: cli.keychain_service.clone(),
        keychain_account: cli.keychain_account.clone(),
        no_keychain: cli.no_keychain,
        reset_time: resolve_reset_time(cli, config),
        total_credits: resolve_total_credits(cli, config),
        api_key: resolve_hyper_api_key(config),
    }
}

/// The Hyper API key: environment first, then the config file.
///
/// Deliberately **not** a CLI flag — an argument is visible to every process on the machine via
/// `ps`, so a frontend passing a secret that way would leak it. Writing it goes through
/// `config --save`, which reads the value on stdin for the same reason.
fn resolve_hyper_api_key(config: &Config) -> Option<String> {
    std::env::var("HYPER_API_KEY")
        .ok()
        .or_else(|| config.hyper_api_key.clone())
        .map(|k| k.trim().to_string())
        .filter(|k| !k.is_empty())
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

    fn cli_with(work_days: Option<u8>, daily_budget: Option<f64>, reset: Option<&str>) -> Cli {
        Cli {
            agent: "hyper".into(),
            status: false,
            json: true,
            creds_path: None,
            config_dir: None,
            keychain_account: None,
            id: None,
            label: None,
            daily_budget,
            work_days,
            timeout: 30,
            keychain_service: None,
            no_keychain: false,
            reset_time: reset.map(str::to_string),
            total_credits: None,
            cache_ttl: 60,
            no_cache: false,
            save: false,
            hyper_api_key: None,
        }
    }

    fn config_with(work_days: Option<u8>, daily_budget: Option<f64>, reset: Option<&str>) -> Config {
        Config {
            work_days,
            daily_budget,
            reset_time: reset.map(str::to_string),
            total_credits: None,
            hyper_api_key: None,
        }
    }

    #[test]
    fn a_flag_overrides_the_config_file() {
        let cli = cli_with(Some(3), Some(33.0), None);
        let config = config_with(Some(6), Some(16.0), None);
        assert_eq!(resolve_work_days(&cli, &config), 3);
        assert_eq!(resolve_daily_budget(&cli, &config), 33.0);
    }

    #[test]
    fn the_config_file_applies_when_no_flag_is_given() {
        let cli = cli_with(None, None, None);
        let config = config_with(Some(6), Some(16.0), None);
        assert_eq!(resolve_work_days(&cli, &config), 6);
        assert_eq!(resolve_daily_budget(&cli, &config), 16.0);
    }

    #[test]
    fn the_built_in_default_applies_when_neither_is_given() {
        let cli = cli_with(None, None, None);
        let config = Config::default();
        assert_eq!(resolve_work_days(&cli, &config), Budget::default().work_days);
        assert_eq!(
            resolve_daily_budget(&cli, &config),
            Budget::default().daily_budget,
            "the 5-work-day default must still be 20%/day"
        );
    }

    /// An unspecified daily budget follows the work-day split rather than staying flat —
    /// otherwise a 4-day week silently budgets only 80% of the cycle.
    #[test]
    fn an_unset_daily_budget_is_derived_from_the_work_days() {
        assert_eq!(
            resolve_daily_budget(&cli_with(Some(4), None, None), &Config::default()),
            25.0
        );
        assert_eq!(
            resolve_daily_budget(&cli_with(None, None, None), &config_with(Some(4), None, None)),
            25.0
        );
        // Every split spends exactly one cycle.
        for days in 1u8..=7 {
            let got = resolve_daily_budget(&cli_with(Some(days), None, None), &Config::default());
            assert!((got * days as f64 - 100.0).abs() < 1e-9, "{days} days -> {got}");
        }
    }

    #[test]
    fn an_explicit_daily_budget_still_wins_over_the_derivation() {
        assert_eq!(
            resolve_daily_budget(&cli_with(Some(4), Some(33.0), None), &Config::default()),
            33.0
        );
        assert_eq!(
            resolve_daily_budget(&cli_with(Some(4), None, None), &config_with(None, Some(15.0), None)),
            15.0
        );
    }

    /// The frontend's job: an explicit argument must win over everything ambient.
    #[test]
    fn a_reset_time_flag_overrides_env_and_config() {
        assert_eq!(
            resolve_reset_time_from(
                Some("09:30Z".into()),
                Some("20:18".into()),
                Some("08:00 local".into())
            )
            .as_deref(),
            Some("09:30Z")
        );
    }

    #[test]
    fn a_reset_time_env_var_overrides_the_config_file() {
        assert_eq!(
            resolve_reset_time_from(None, Some("20:18".into()), Some("08:00 local".into()))
                .as_deref(),
            Some("20:18")
        );
    }

    #[test]
    fn a_reset_time_falls_back_to_the_config_file() {
        assert_eq!(
            resolve_reset_time_from(None, None, Some("08:00 local".into())).as_deref(),
            Some("08:00 local")
        );
    }

    /// A GUI passing an empty field through must not shadow the next source down, nor error.
    #[test]
    fn a_blank_reset_time_is_treated_as_absent() {
        assert_eq!(
            resolve_reset_time_from(
                Some("   ".into()),
                Some(String::new()),
                Some("08:00 local".into())
            )
            .as_deref(),
            Some("08:00 local")
        );
        assert_eq!(resolve_reset_time_from(None, None, None), None);
    }

    /// A cached reading must report when it was *taken*, not when it was re-rendered — the whole
    /// point of the field for a consumer deciding whether to trust the numbers.
    #[test]
    fn a_cached_reading_reports_its_real_age() {
        let now = Utc::now();
        assert_eq!(fetched_at(now, Duration::from_secs(0)), now);
        assert_eq!(
            fetched_at(now, Duration::from_secs(600)),
            now - chrono::Duration::seconds(600)
        );
        // An age that won't convert must not read as fresh: erring toward "current" is the one
        // direction that misleads.
        assert!(fetched_at(now, Duration::from_secs(u64::MAX)) < now);
    }

    #[test]
    fn a_total_credits_flag_overrides_the_config_file() {
        let mut cli = cli_with(None, None, None);
        cli.total_credits = Some(CreditsArg(Some(1527)));
        let mut config = config_with(None, None, None);
        config.total_credits = Some(1600);
        assert_eq!(resolve_total_credits(&cli, &config), Some(1527));
    }

    /// The app passes the field through on every run, so an empty one must fall through to the
    /// config file rather than shadowing it — the same contract an empty `--reset-time` has.
    #[test]
    fn a_blank_total_credits_falls_through_to_the_config_file() {
        let mut cli = cli_with(None, None, None);
        cli.total_credits = Some(CreditsArg(None));
        let mut config = config_with(None, None, None);
        config.total_credits = Some(1600);
        assert_eq!(resolve_total_credits(&cli, &config), Some(1600));
        assert_eq!(
            resolve_total_credits(&cli, &config_with(None, None, None)),
            None,
            "nothing anywhere means the provider keeps inferring"
        );
    }

    #[test]
    fn total_credits_parses_a_whole_number_or_a_clear() {
        use std::str::FromStr;
        assert_eq!(CreditsArg::from_str("1527"), Ok(CreditsArg(Some(1527))));
        assert_eq!(CreditsArg::from_str("  1527 "), Ok(CreditsArg(Some(1527))));
        assert_eq!(CreditsArg::from_str(""), Ok(CreditsArg(None)));
        assert_eq!(CreditsArg::from_str("   "), Ok(CreditsArg(None)));
        for bad in ["1527.5", "-10", "many", "1_527"] {
            assert!(CreditsArg::from_str(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    /// An emptied field is how the app says "go back to inferring", so it has to clear rather
    /// than be ignored — the third state a bare `Option` can't carry.
    #[test]
    fn an_empty_total_credits_clears_it_on_save() {
        let mut cli = cli_with(None, None, None);
        cli.total_credits = Some(CreditsArg(Some(1527)));
        let patch = config_patch(&cli).unwrap();
        assert_eq!(patch.total_credits, Some(1527));
        assert!(!patch.clear_total_credits);

        cli.total_credits = Some(CreditsArg(None));
        let patch = config_patch(&cli).unwrap();
        assert_eq!(patch.total_credits, None);
        assert!(patch.clear_total_credits);

        cli.total_credits = None;
        let patch = config_patch(&cli).unwrap();
        assert!(patch.total_credits.is_none() && !patch.clear_total_credits);
    }
}
