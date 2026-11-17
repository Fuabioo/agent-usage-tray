//! Charm Hyper provider.
//!
//! Calls `GET https://hyper.charm.land/v1/credits` with an API key — from
//! [`FetchOptions::api_key`] (which the CLI fills from its config file) or the `HYPER_API_KEY`
//! environment variable — and returns a `Credits` pool window.
//!
//! Hyper's subscription model:
//!   - a weekly allowance of [`ALLOWANCE`] HC that refills at a fixed weekly reset (unused
//!     allowance expires)
//!   - a five-hour session cap layered on top of the allowance
//!   - prepaid (purchased) credits, consumed *after* the subscription credits and never expiring
//!
//! The API returns only a single `balance` (int): the allowance left this week plus the prepaid
//! credits. The five-hour session is not part of it and the API exposes it nowhere, so this
//! provider reports no session window.
//!
//! To derive `total` we need the prepaid count: each weekly cycle (keyed by its reset instant) we
//! re-derive it as `max(0, balance - ALLOWANCE)`, but never below the last known baseline —
//! prepaid credits persist across cycles, so a mid-cycle cold start (when `balance` no longer
//! reflects a full allowance) must not undercount them. Every reading then clamps the baseline
//! into the only band the balance permits (see [`clamp_permanent`]), so a baseline that has gone
//! wrong heals instead of persisting. The baseline is cached in
//! `~/.cache/agent-usage/hyper.prepaid.json`.
//!
//! All of that is still an inference from one number, so [`FetchOptions::total_credits`] can
//! state the pool outright and skip it.
//!
//! The reset moment comes from [`FetchOptions::reset_time`] (the CLI's `--reset-time`, which the
//! macOS Settings window drives) or, when unset, the `HYPER_RESET_TIME` env var. Either is an
//! optional weekday, then `HH:MM`, then an optional zone — `Z`/`UTC`, `local`, or a fixed
//! `±HH:MM` offset (e.g. `Sun 00:00 UTC`, `mon 08:30 local`). **An omitted weekday means Sunday
//! and an omitted zone means UTC.** The zone matters because a subscription's reset is a
//! wall-clock time: writing `08:00 local` keeps the reset at 8 am through a DST change, where the
//! equivalent UTC value would silently drift by an hour twice a year. An unset value defaults to
//! Sunday midnight UTC, and a malformed value is a hard error rather than a silent fallback.
//!
//! The API exposes no reset field, so this cannot be discovered — and it cannot be inferred from
//! the balance either: a weekly refill and a *purchase* of prepaid credits both show up as the
//! balance jumping upward, so treating a jump as a reset would mis-key the prepaid baseline
//! every time credits are bought.

use agent_usage_core::{AgentInfo, FetchOptions, Provider, Usage, UsageError, Window};
use chrono::{
    DateTime, Datelike, Duration, FixedOffset, LocalResult, NaiveDate, TimeZone, Utc, Weekday,
};

use crate::http;

/// The weekly subscription allowance (the Business Casual plan's 180 HC).
const ALLOWANCE: i64 = 180;
const USAGE_URL: &str = "https://hyper.charm.land/v1/credits";

/// The API response shape.
#[derive(serde::Deserialize)]
pub struct CreditsResponse {
    pub balance: i64,
}

pub struct Hyper;

impl Hyper {
    pub const fn new() -> Self {
        Hyper
    }
}

impl Default for Hyper {
    fn default() -> Self {
        Hyper::new()
    }
}

impl Provider for Hyper {
    fn id(&self) -> &'static str {
        "hyper"
    }

    fn label(&self) -> &'static str {
        "Charm Hyper"
    }

    fn source(&self) -> &'static str {
        "Hyper /v1/credits API"
    }

    /// Hyper is opt-in: it only joins the default `all` sweep once an API key is available, so a
    /// fresh install shows just the agents most people have (Claude, Codex). Whether the key came
    /// from the config file or the environment makes no difference here. `agent-usage hyper` still
    /// resolves directly and reports a clear "no Hyper API key" error when there is none.
    fn in_default_set(&self, opts: &FetchOptions) -> bool {
        resolve_api_key(opts).is_ok()
    }

    fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError> {
        let api_key = resolve_api_key(opts)?;
        let now = chrono::Utc::now();

        let bearer = format!("Bearer {api_key}");
        let body = http::get(
            USAGE_URL,
            &[
                ("Authorization", bearer.as_str()),
                ("Accept", "application/json"),
            ],
            opts.timeout,
        )?;

        let parsed: CreditsResponse =
            serde_json::from_str(&body).map_err(|e| UsageError::Parse(e.to_string()))?;

        let balance = parsed.balance;
        // An explicit option wins over the environment so a GUI can own this setting without
        // requiring a shell export — a `.app` launched from Finder inherits no shell profile.
        let raw = opts
            .reset_time
            .clone()
            .unwrap_or_else(|| std::env::var("HYPER_RESET_TIME").unwrap_or_default());
        let resets_at = next_reset(now, parse_reset_time(&raw)?);

        let permanent =
            resolve_permanent("hyper", balance, resets_at.timestamp(), opts.total_credits);

        let total = permanent as f64 + ALLOWANCE as f64;
        let remaining = balance as f64;

        Ok(Usage {
            agent: AgentInfo {
                id: self.id().to_string(),
                label: self.label().to_string(),
                source: self.source().to_string(),
            },
            // `total`/`remaining` carry the raw balance (weekly allowance + prepaid surplus), but
            // the percentage is measured against the allowance via `with_budget`: spending beyond
            // the week's allowance dips into the prepaid surplus and reads as "extra usage"
            // (over 100%). No `burn_per_day` — the grant is fixed, not an observed burn rate, so
            // there is nothing to project. Label is a lowercase noun ("credits") to match the
            // other windows' convention, which the UI renders as "<label> left".
            windows: vec![Window::pool("credits", total, remaining, None, Some(resets_at))
                .with_budget(ALLOWANCE as f64)],
        })
    }
}

/// Resolve the API key: the caller's resolved value (config file) first, then the
/// `HYPER_API_KEY` environment variable.
///
/// A config-supplied key is what makes this agent survive a reboot — an app started by launchd at
/// login inherits no shell profile, so an exported variable is invisible to it.
fn resolve_api_key(opts: &FetchOptions) -> Result<String, UsageError> {
    opts.api_key
        .as_deref()
        .map(str::trim)
        .filter(|k| !k.is_empty())
        .map(str::to_string)
        .or_else(|| {
            std::env::var("HYPER_API_KEY")
                .ok()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
        })
        .ok_or_else(|| {
            UsageError::CredentialsRead(
                "no Hyper API key: set `hyper_api_key` in the config file or export HYPER_API_KEY"
                    .to_string(),
            )
        })
}

/// The clock a [`ResetSpec`]'s time is read on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResetZone {
    /// The default, and what a bare `HH:MM` has always meant.
    Utc,
    /// The machine's timezone — what you mean when you say "it refreshes at 8 am".
    Local,
    Offset(FixedOffset),
}

/// A weekly reset expressed as a weekday and wall-clock time on a particular clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResetSpec {
    weekday: Weekday,
    hour: u32,
    minute: u32,
    zone: ResetZone,
}

/// The weekday an omitted one means.
const DEFAULT_WEEKDAY: Weekday = Weekday::Sun;

/// Compute the *next* reset instant from `now`. `None` defaults to Sunday midnight UTC.
fn next_reset(now: DateTime<Utc>, reset: Option<ResetSpec>) -> DateTime<Utc> {
    let Some(s) = reset else {
        return next_reset_in(now, DEFAULT_WEEKDAY, 0, 0, &Utc);
    };
    match s.zone {
        ResetZone::Utc => next_reset_in(now, s.weekday, s.hour, s.minute, &Utc),
        ResetZone::Local => next_reset_in(now, s.weekday, s.hour, s.minute, &chrono::Local),
        ResetZone::Offset(off) => next_reset_in(now, s.weekday, s.hour, s.minute, &off),
    }
}

/// The next instant at which the wall clock in `tz` reads `hour:minute` on `weekday`.
///
/// Resolved per calendar day in `tz` rather than by adding 7 days, so a DST shift moves the
/// reset with the wall clock instead of sliding it an hour.
fn next_reset_in<Tz: TimeZone>(
    now: DateTime<Utc>,
    weekday: Weekday,
    hour: u32,
    minute: u32,
    tz: &Tz,
) -> DateTime<Utc> {
    let mut date = now.with_timezone(tz).date_naive();
    // Eight days cover today plus a full week; the extra pass only guards a date that fails to
    // resolve at all.
    for _ in 0..9 {
        if date.weekday() == weekday {
            if let Some(candidate) = resolve_in(date, hour, minute, tz) {
                if candidate > now {
                    return candidate;
                }
            }
        }
        let Some(next) = date.succ_opt() else { break };
        date = next;
    }
    now + Duration::days(7)
}

/// Resolve a wall-clock time on `date` in `tz` to a UTC instant.
///
/// Both DST edge cases resolve to the earliest instant that has actually elapsed by that wall
/// time: an ambiguous time (clocks fall back — it happens twice) takes the first occurrence, and
/// a nonexistent one (clocks spring forward — it never reads that value) takes the moment the
/// clock jumps past it.
fn resolve_in<Tz: TimeZone>(
    date: NaiveDate,
    hour: u32,
    minute: u32,
    tz: &Tz,
) -> Option<DateTime<Utc>> {
    let naive = date.and_hms_opt(hour, minute, 0)?;
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
        LocalResult::None => match tz.from_local_datetime(&(naive + Duration::hours(1))) {
            LocalResult::Single(dt) => Some(dt.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, _) => Some(first.with_timezone(&Utc)),
            LocalResult::None => None,
        },
    }
}

/// Parse a `HYPER_RESET_TIME` value: an optional weekday (`Sun`, `monday`, … — Sunday when
/// omitted), then `HH:MM`, then an optional zone — omitted or `Z`/`UTC` for UTC, `local` for the
/// machine's timezone, or a fixed `±HH:MM` / `±HHMM` / `±HH` offset.
/// Examples: `Sun 00:00`, `mon 08:00 local`, `20:18`, `08:00Z`, `Sat 08:00-06:00`.
///
/// An empty value yields `None` (caller defaults to Sunday midnight UTC); a non-empty but
/// malformed value is a hard error, so a typo surfaces instead of silently moving the reset.
fn parse_reset_time(raw: &str) -> Result<Option<ResetSpec>, UsageError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let bad = || {
        UsageError::Unsupported(format!(
            "HYPER_RESET_TIME must be [weekday] HH:MM [zone] \
             (Sunday and UTC when omitted; zone `Z`, `local`, or `±HH:MM`), got {raw:?}"
        ))
    };

    let (weekday, rest) = split_weekday(raw).ok_or_else(bad)?;
    let (clock, zone_raw) = split_zone(rest);
    let (h, m) = clock.split_once(':').ok_or_else(bad)?;
    let hour = h.trim().parse::<u32>().ok().filter(|h| *h < 24).ok_or_else(bad)?;
    let minute = m.trim().parse::<u32>().ok().filter(|m| *m < 60).ok_or_else(bad)?;
    let zone = parse_zone(zone_raw).ok_or_else(bad)?;
    Ok(Some(ResetSpec { weekday, hour, minute, zone }))
}

/// Split a leading weekday token off `"Sun 08:00 local"`. A value starting with a digit has no
/// weekday and means [`DEFAULT_WEEKDAY`]; a leading word that is not a weekday is `None`.
fn split_weekday(raw: &str) -> Option<(Weekday, &str)> {
    if !raw.starts_with(|c: char| c.is_ascii_alphabetic()) {
        return Some((DEFAULT_WEEKDAY, raw));
    }
    let (word, rest) = raw.split_once(char::is_whitespace)?;
    Some((word.parse::<Weekday>().ok()?, rest.trim_start()))
}

/// Split `"08:00 local"` / `"08:00-06:00"` / `"08:00Z"` / `"20:18"` into clock and zone.
///
/// The zone begins at the first sign or letter *after* the leading character, so a value that
/// merely starts with `-` (e.g. `"-1:00"`) is left intact and rejected as a bad hour rather than
/// being mistaken for an offset.
fn split_zone(raw: &str) -> (&str, &str) {
    if let Some(i) = raw.find(char::is_whitespace) {
        return (&raw[..i], raw[i..].trim_start());
    }
    match raw
        .char_indices()
        .skip(1)
        .find(|(_, c)| *c == '+' || *c == '-' || c.is_ascii_alphabetic())
    {
        Some((i, _)) => (&raw[..i], &raw[i..]),
        None => (raw, ""),
    }
}

fn parse_zone(raw: &str) -> Option<ResetZone> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(ResetZone::Utc); // a bare HH:MM has always meant UTC
    }
    match raw.to_ascii_lowercase().as_str() {
        "z" | "utc" | "gmt" => Some(ResetZone::Utc),
        "l" | "local" => Some(ResetZone::Local),
        _ => parse_offset(raw).map(ResetZone::Offset),
    }
}

/// Parse a fixed UTC offset: `±HH:MM`, `±HHMM`, or `±HH`.
fn parse_offset(raw: &str) -> Option<FixedOffset> {
    let (sign, rest) = match raw.chars().next()? {
        '+' => (1, &raw[1..]),
        '-' => (-1, &raw[1..]),
        _ => return None,
    };
    let (h, m) = match rest.split_once(':') {
        Some(parts) => parts,
        None if rest.len() == 4 => rest.split_at(2),
        None if !rest.is_empty() && rest.len() <= 2 => (rest, "0"),
        None => return None,
    };
    let hours: i32 = h.parse().ok().filter(|h| *h < 24)?;
    let mins: i32 = m.parse().ok().filter(|m| *m < 60)?;
    FixedOffset::east_opt(sign * (hours * 3_600 + mins * 60))
}

/// Resolve the permanent-credit baseline for `balance` in the cycle identified by
/// `cycle` (the reset instant, unix seconds).
///
/// A configured total wins outright. Otherwise: within a known cycle the cached value is reused,
/// and on a new (or uncached) cycle it is re-derived from the balance but never dropped below the
/// last known baseline (see [`derive_permanent`]). Either way the result is clamped into the band
/// the balance permits, so a baseline that has drifted corrects itself on the next reading rather
/// than being carried forever. The cache write is best-effort — a failure just means we re-derive
/// on the next fetch.
fn resolve_permanent(id: &str, balance: i64, cycle: i64, configured_total: Option<u32>) -> u32 {
    if let Some(total) = configured_total {
        return clamp_permanent(total.saturating_sub(ALLOWANCE as u32), balance);
    }

    let stored = perm::read(id);
    if let Some(r) = &stored {
        if r.cycle == cycle {
            // Re-checked even inside a known cycle: spending past the week's allowance eats into
            // permanent credits, so a cached baseline the balance can no longer support is stale,
            // not authoritative. Without this the baseline could only ever rise, and a single
            // overspend would inflate `total` for good.
            let value = clamp_permanent(r.value, balance);
            if value != r.value {
                perm::write(id, &perm::Record { value, cycle });
            }
            return value;
        }
    }
    let value = derive_permanent(stored.map_or(0, |r| r.value), balance);
    perm::write(id, &perm::Record { value, cycle });
    value
}

/// New-cycle baseline: `balance - ALLOWANCE` (exact only at reset, when the grant is full),
/// floored at the previously known baseline so a mid-cycle cold start cannot undercount
/// permanent credits.
fn derive_permanent(previous: u32, balance: i64) -> u32 {
    clamp_permanent(previous, balance)
}

/// Clamp a permanent-credit baseline into the only band `balance` allows.
///
/// Two invariants bound it, and both are what make a wrong baseline recoverable from the single
/// number the API reports:
/// - **At most `balance`** — permanent credits are part of the balance, so a baseline above it
///   describes credits that are not there. This is the direction the old floor could not walk
///   back: spending past the weekly allowance consumes permanent credits, and a baseline that only
///   ever rose kept reporting the pre-spend pool.
/// - **At least `balance - ALLOWANCE`** — no more than one weekly allowance of the balance can be
///   non-permanent, so anything above that much is permanent by definition.
fn clamp_permanent(value: u32, balance: i64) -> u32 {
    let low = (balance - ALLOWANCE).max(0) as u32;
    let high = balance.max(0) as u32;
    value.clamp(low, high)
}

/// Tiny permanent-credits baseline cache, stored alongside the main snapshot cache and
/// best-effort like it: read/write failures are swallowed, since a miss just triggers
/// re-derivation on the next fetch.
mod perm {
    use agent_usage_core::cache_dir;
    use serde::{Deserialize, Serialize};
    use std::path::PathBuf;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    pub struct Record {
        pub value: u32,
        /// Identity of the cycle this baseline was derived for: the reset instant as
        /// unix seconds. A different reset instant means a new cycle.
        pub cycle: i64,
    }

    /// Rename the file whenever the allowance model changes: a baseline derived under another
    /// model can sit inside the new clamp band, where nothing would ever correct it.
    fn cache_path(id: &str) -> Option<PathBuf> {
        cache_dir().map(|d| d.join(format!("{id}.prepaid.json")))
    }

    pub fn read(id: &str) -> Option<Record> {
        let path = cache_path(id)?;
        let contents = std::fs::read_to_string(&path).ok()?;
        serde_json::from_str(&contents).ok()
    }

    pub fn write(id: &str, rec: &Record) {
        let Some(path) = cache_path(id) else { return };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(s) = serde_json::to_string(rec) {
            let _ = std::fs::write(path, s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    #[test]
    fn identity() {
        let h = Hyper::new();
        assert_eq!(h.id(), "hyper");
        assert_eq!(h.label(), "Charm Hyper");
    }

    #[test]
    fn parses_balance() {
        let body = r#"{"balance": 610}"#;
        let parsed: CreditsResponse = serde_json::from_str(body).unwrap();
        assert_eq!(parsed.balance, 610);
    }

    #[test]
    fn reset_defaults_to_sunday_midnight_utc_when_unset() {
        let now = chrono::Utc::now();
        let r = next_reset(now, parse_reset_time("").unwrap());
        assert!(r > now);
        assert!(r - now <= Duration::days(7));
        assert_eq!(r.weekday(), Weekday::Sun);
        assert_eq!(r.hour(), 0);
        assert_eq!(r.minute(), 0);
    }

    fn spec(weekday: Weekday, hour: u32, minute: u32, zone: ResetZone) -> Option<ResetSpec> {
        Some(ResetSpec { weekday, hour, minute, zone })
    }

    /// A bare `HH:MM` means Sunday, UTC — the two parts a value may leave out.
    #[test]
    fn reset_time_parses_hh_mm() {
        assert_eq!(parse_reset_time("20:18").unwrap(), spec(Weekday::Sun, 20, 18, ResetZone::Utc));
        assert_eq!(parse_reset_time("00:00").unwrap(), spec(Weekday::Sun, 0, 0, ResetZone::Utc));
        assert_eq!(parse_reset_time("   ").unwrap(), None);
    }

    #[test]
    fn reset_time_parses_a_weekday() {
        let sun = spec(Weekday::Sun, 0, 0, ResetZone::Utc);
        assert_eq!(parse_reset_time("Sun 00:00").unwrap(), sun);
        assert_eq!(parse_reset_time("sunday 00:00 UTC").unwrap(), sun);
        assert_eq!(
            parse_reset_time("Mon 08:30 local").unwrap(),
            spec(Weekday::Mon, 8, 30, ResetZone::Local)
        );
        assert_eq!(
            parse_reset_time("SAT  08:00-06:00").unwrap(),
            spec(Weekday::Sat, 8, 0, ResetZone::Offset(FixedOffset::west_opt(6 * 3600).unwrap()))
        );
    }

    #[test]
    fn reset_time_parses_an_explicit_zone() {
        let at8 = |zone| spec(Weekday::Sun, 8, 0, zone);
        assert_eq!(parse_reset_time("08:00 local").unwrap(), at8(ResetZone::Local));
        assert_eq!(parse_reset_time("08:00local").unwrap(), at8(ResetZone::Local));
        assert_eq!(parse_reset_time("08:00 LOCAL").unwrap(), at8(ResetZone::Local));
        assert_eq!(parse_reset_time("08:00Z").unwrap(), at8(ResetZone::Utc));
        assert_eq!(parse_reset_time("08:00 utc").unwrap(), at8(ResetZone::Utc));

        let west6 = FixedOffset::west_opt(6 * 3600).unwrap();
        let east2 = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(parse_reset_time("08:00-06:00").unwrap(), at8(ResetZone::Offset(west6)));
        assert_eq!(parse_reset_time("08:00 -0600").unwrap(), at8(ResetZone::Offset(west6)));
        assert_eq!(parse_reset_time("08:00+02").unwrap(), at8(ResetZone::Offset(east2)));
    }

    #[test]
    fn malformed_reset_time_is_an_error() {
        for bad in [
            "20", "24:00", "12:60", "ab:cd", "-1:00", "08:00 pacific", "08:00+", "08:00-25:00",
            "08:00-06:99", "08:00 -06000", "Sun", "Sun08:00", "Funday 08:00", "Sun Mon 08:00",
        ] {
            assert!(parse_reset_time(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    /// The zone is what makes "8 am" mean 8 am: the same clock time resolves to a different
    /// instant per zone, and that instant is also the permanent-baseline cycle key.
    #[test]
    fn reset_resolves_against_its_zone() {
        let west6 = FixedOffset::west_opt(6 * 3600).unwrap();
        // Mon 2026-07-27 03:00 UTC = Sun 21:00 in UTC-6.
        let now = Utc.with_ymd_and_hms(2026, 7, 27, 3, 0, 0).unwrap();

        // Mon 08:00 UTC is 5 hours out.
        assert_eq!(
            next_reset(now, spec(Weekday::Mon, 8, 0, ResetZone::Utc)),
            Utc.with_ymd_and_hms(2026, 7, 27, 8, 0, 0).unwrap()
        );
        // Mon 08:00 in UTC-6 is 14:00 UTC — 11 hours out, though it is still Sunday there.
        assert_eq!(
            next_reset(now, spec(Weekday::Mon, 8, 0, ResetZone::Offset(west6))),
            Utc.with_ymd_and_hms(2026, 7, 27, 14, 0, 0).unwrap()
        );
        // Sun 08:00 UTC already passed this week, so it is six days out.
        assert_eq!(
            next_reset(now, spec(Weekday::Sun, 8, 0, ResetZone::Utc)),
            Utc.with_ymd_and_hms(2026, 8, 2, 8, 0, 0).unwrap()
        );
    }

    #[test]
    fn a_reset_already_past_today_rolls_to_next_week() {
        let west6 = FixedOffset::west_opt(6 * 3600).unwrap();
        // Mon 2026-07-27 20:00 UTC = 14:00 in UTC-6, after that day's 08:00 local reset.
        let now = Utc.with_ymd_and_hms(2026, 7, 27, 20, 0, 0).unwrap();
        assert_eq!(
            next_reset(now, spec(Weekday::Mon, 8, 0, ResetZone::Offset(west6))),
            Utc.with_ymd_and_hms(2026, 8, 3, 14, 0, 0).unwrap()
        );
    }

    /// A stand-in timezone with one US-style DST rule, so the gap and ambiguity paths can be
    /// exercised deterministically without depending on a timezone database.
    ///
    /// Winter UTC-5, summer UTC-4. Clocks spring forward at 2026-03-08 07:00 UTC (local 02:00 →
    /// 03:00, so local 02:00–02:59 never happens) and fall back at 2026-11-01 06:00 UTC (local
    /// 02:00 → 01:00, so local 01:00–01:59 happens twice).
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct DstZone;

    impl DstZone {
        const STD: i32 = -5 * 3600;
        const DST: i32 = -4 * 3600;

        fn spring() -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 3, 8, 7, 0, 0).unwrap()
        }
        fn fall() -> DateTime<Utc> {
            Utc.with_ymd_and_hms(2026, 11, 1, 6, 0, 0).unwrap()
        }
        fn offset_secs_at(utc: &chrono::NaiveDateTime) -> i32 {
            let utc = utc.and_utc();
            if utc >= Self::spring() && utc < Self::fall() {
                Self::DST
            } else {
                Self::STD
            }
        }
    }

    impl TimeZone for DstZone {
        type Offset = FixedOffset;

        fn from_offset(_: &FixedOffset) -> Self {
            DstZone
        }

        fn offset_from_local_date(&self, local: &NaiveDate) -> LocalResult<FixedOffset> {
            self.offset_from_local_datetime(&local.and_hms_opt(0, 0, 0).unwrap())
        }

        /// A candidate offset is real only if the instant it implies actually uses it — the
        /// standard round-trip check. Neither holding means the wall time was skipped (a gap);
        /// both holding means it occurred twice (ambiguous), earliest first.
        fn offset_from_local_datetime(&self, local: &chrono::NaiveDateTime) -> LocalResult<FixedOffset> {
            let mut hits = [Self::DST, Self::STD]
                .into_iter()
                .filter(|secs| Self::offset_secs_at(&(*local - Duration::seconds(*secs as i64))) == *secs)
                .map(|secs| FixedOffset::east_opt(secs).unwrap());
            match (hits.next(), hits.next()) {
                (Some(a), Some(b)) => LocalResult::Ambiguous(a, b),
                (Some(a), None) => LocalResult::Single(a),
                _ => LocalResult::None,
            }
        }

        fn offset_from_utc_date(&self, utc: &NaiveDate) -> FixedOffset {
            self.offset_from_utc_datetime(&utc.and_hms_opt(0, 0, 0).unwrap())
        }

        fn offset_from_utc_datetime(&self, utc: &chrono::NaiveDateTime) -> FixedOffset {
            FixedOffset::east_opt(Self::offset_secs_at(utc)).unwrap()
        }
    }

    /// A wall-clock reset must stay on the wall clock across a DST shift rather than sliding an
    /// hour — which is exactly what adding a fixed seven days would do.
    #[test]
    fn a_local_reset_tracks_a_dst_shift() {
        // DST ends Sun 2026-11-01: Sat 08:00 local is UTC-4 before and UTC-5 after.
        let before = Utc.with_ymd_and_hms(2026, 10, 30, 20, 0, 0).unwrap();
        let first = next_reset_in(before, Weekday::Sat, 8, 0, &DstZone);
        assert_eq!(
            first,
            Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap(),
            "08:00 at UTC-4 = 12:00 UTC"
        );
        let second = next_reset_in(first, Weekday::Sat, 8, 0, &DstZone);
        assert_eq!(
            second,
            Utc.with_ymd_and_hms(2026, 11, 7, 13, 0, 0).unwrap(),
            "08:00 at UTC-5 = 13:00 UTC — the wall clock held, the UTC instant moved"
        );
        assert_eq!(second - first, Duration::days(7) + Duration::hours(1));
    }

    /// Spring-forward skips 02:00–03:00 local; a reset nominally in that gap must still fire.
    #[test]
    fn a_reset_inside_a_dst_gap_still_resolves() {
        let now = Utc.with_ymd_and_hms(2026, 3, 8, 1, 0, 0).unwrap();
        let reset = next_reset_in(now, Weekday::Sun, 2, 30, &DstZone);
        assert_eq!(reset, Utc.with_ymd_and_hms(2026, 3, 8, 7, 30, 0).unwrap());
        assert!(reset > now, "a skipped wall time must not stall the reset");
    }

    /// Fall-back repeats 01:00–01:59 local; the first occurrence is the reset.
    #[test]
    fn an_ambiguous_reset_takes_the_first_occurrence() {
        let now = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(
            next_reset_in(now, Weekday::Sun, 1, 30, &DstZone),
            Utc.with_ymd_and_hms(2026, 11, 1, 5, 30, 0).unwrap()
        );
    }

    #[test]
    fn permanent_derivation_floors_at_previous_and_zero() {
        // At reset the allowance is full, so balance - ALLOWANCE recovers prepaid exactly.
        assert_eq!(derive_permanent(0, 515), 335);
        // Mid-cycle cold start (part of the allowance spent) keeps the known baseline.
        assert_eq!(derive_permanent(335, 434), 335);
        // Nothing known and balance below the allowance: zero, never negative.
        assert_eq!(derive_permanent(0, 150), 0);
        // A balance proving more prepaid than we knew raises the baseline.
        assert_eq!(derive_permanent(100, 515), 335);
    }

    /// The balance is the only ground truth there is, and it bounds the baseline from both
    /// sides. The upper bound is the one that matters in practice: without it, spending past the
    /// weekly allowance leaves a baseline describing credits that are already gone.
    #[test]
    fn a_baseline_the_balance_cannot_support_is_lowered() {
        // 1350 prepaid + a 180 allowance = a 1530 pool; spending 253 of it leaves 1277 in total,
        // which is therefore all the prepaid credit that can still exist.
        assert_eq!(clamp_permanent(1350, 1277), 1277);
        // Below the floor it rises: at least `balance - ALLOWANCE` of a balance is prepaid.
        assert_eq!(clamp_permanent(100, 515), 335);
        // Inside the band it is left alone — that is where the cached baseline carries the
        // information the balance cannot.
        assert_eq!(clamp_permanent(400, 515), 400);
        // Never negative, whatever the balance.
        assert_eq!(clamp_permanent(400, 0), 0);
    }

    /// The arc a floor-only baseline could not walk: a 1530 pool (1350 prepaid + the allowance),
    /// an overspend past the allowance, then the next refill. The overspend is the whole point —
    /// prepaid credits really did go down, and a baseline that only rises reports the pool the
    /// user had *before* it for every week after.
    #[test]
    fn an_overspend_shrinks_the_pool_instead_of_haunting_it() {
        // Spent 253 against a 180 allowance: 73 of the 1350 prepaid are gone, so 1277 is both
        // the balance and all the prepaid credit left.
        let permanent = clamp_permanent(1350, 1277);
        assert_eq!(permanent, 1277);
        assert_eq!(permanent as i64 + ALLOWANCE, 1457, "the pool shrank with the spend");

        // Next refill: the allowance is whole again, and the week starts at zero used rather
        // than carrying the 73 as a phantom deficit against a pool that no longer exists.
        let balance = permanent as i64 + ALLOWANCE;
        let next = derive_permanent(permanent, balance);
        assert_eq!(next, 1277);
        assert_eq!(next as i64 + ALLOWANCE - balance, 0);
    }

    /// A configured total states the pool outright, but is still bounded by the balance: one set
    /// below what is actually held would render a pool smaller than its own contents.
    #[test]
    fn a_configured_total_overrides_the_inference() {
        let cycle = 1_785_508_200;
        // 515 total = 335 prepaid + the 180 allowance, and the balance can support it.
        assert_eq!(resolve_permanent("hyper-test-cfg", 434, cycle, Some(515)), 335);
        // A total below the balance (credits were bought since) yields to the balance.
        assert_eq!(resolve_permanent("hyper-test-cfg", 3000, cycle, Some(515)), 2820);
        // A total under one allowance is a pool of nothing but the allowance.
        assert_eq!(resolve_permanent("hyper-test-cfg", 100, cycle, Some(150)), 0);
    }
}
