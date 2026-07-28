//! Charm Hyper provider.
//!
//! Calls `GET https://hyper.charm.land/v1/credits` with an API key
//! (from `HYPER_API_KEY` env var) and returns a `Credits` pool window.
//!
//! Hyper's subscription model:
//!   - 250 HC refresh every 24h (non-stackable, unused expires)
//!   - Permanent purchased credits are consumed *after* daily allocation
//!     and never expire
//!
//! The API returns only a single `balance` (int). To derive `total` we need the
//! permanent-credit count: each 24h cycle (keyed by its reset instant) we re-derive
//! it as `max(0, balance - 250)`, but never below the last known baseline — permanent
//! credits persist across cycles, so a mid-cycle cold start (when `balance` no longer
//! reflects a full daily grant) must not undercount them. The baseline is cached in
//! `~/.cache/agent-usage/hyper.permanent.json`.
//!
//! The reset moment comes from [`FetchOptions::reset_time`] (the CLI's `--reset-time`, which the
//! macOS Settings window drives) or, when unset, the `HYPER_RESET_TIME` env var. Either is `HH:MM`
//! plus an optional zone — `Z`/`UTC`, `local`, or a fixed `±HH:MM` offset. **A bare `HH:MM` means UTC**, so existing
//! configs keep their meaning. The zone matters because a subscription's reset is a wall-clock
//! time: writing `08:00 local` keeps the reset at 8 am through a DST change, where the equivalent
//! UTC value would silently drift by an hour twice a year. An unset value defaults to midnight
//! UTC, and a malformed value is a hard error rather than a silent fallback.
//!
//! The API exposes no reset field, so this cannot be discovered — and it cannot be inferred from
//! the balance either: a daily refresh and a *purchase* of permanent credits both show up as the
//! balance jumping upward, so treating a jump as a reset would mis-key the permanent baseline
//! every time credits are bought.

use agent_usage_core::{AgentInfo, FetchOptions, Provider, Usage, UsageError, Window};
use chrono::{DateTime, Duration, FixedOffset, LocalResult, NaiveDate, TimeZone, Utc};

use crate::http;

const DAILY: i64 = 250;
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

    /// Hyper is opt-in: it only joins the default `all` sweep once `HYPER_API_KEY` is set, so a
    /// fresh install shows just the agents most people have (Claude, Codex). `agent-usage hyper`
    /// still resolves directly and reports a clear "HYPER_API_KEY not set" error when unset.
    fn in_default_set(&self) -> bool {
        std::env::var_os("HYPER_API_KEY").is_some()
    }

    fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError> {
        let api_key = resolve_api_key()?;
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

        let permanent = resolve_permanent("hyper", balance, resets_at.timestamp());

        let total = permanent as f64 + DAILY as f64;
        let remaining = balance as f64;

        Ok(Usage {
            agent: AgentInfo {
                id: self.id().to_string(),
                label: self.label().to_string(),
                source: self.source().to_string(),
            },
            // `total`/`remaining` carry the raw balance (250 daily grant + permanent surplus),
            // but the percentage is measured against the daily grant via `with_budget`: spending
            // beyond the day's 250 dips into the permanent surplus and reads as "extra usage"
            // (over 100%). No `burn_per_day` — the grant is fixed, not an observed burn rate, so
            // there is nothing to project. Label is a lowercase noun ("credits") to match the
            // other windows' convention, which the UI renders as "<label> left".
            windows: vec![Window::pool("credits", total, remaining, None, Some(resets_at))
                .with_budget(DAILY as f64)],
        })
    }
}

/// Read the API key from the `HYPER_API_KEY` environment variable.
fn resolve_api_key() -> Result<String, UsageError> {
    std::env::var("HYPER_API_KEY")
        .map_err(|_| UsageError::CredentialsRead("HYPER_API_KEY not set".to_string()))
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

/// A daily reset expressed as a wall-clock time on a particular clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ResetSpec {
    hour: u32,
    minute: u32,
    zone: ResetZone,
}

/// Compute the *next* reset instant from `now`. `None` defaults to midnight UTC.
fn next_reset(now: DateTime<Utc>, reset: Option<ResetSpec>) -> DateTime<Utc> {
    let Some(spec) = reset else {
        return next_reset_in(now, 0, 0, &Utc);
    };
    match spec.zone {
        ResetZone::Utc => next_reset_in(now, spec.hour, spec.minute, &Utc),
        ResetZone::Local => next_reset_in(now, spec.hour, spec.minute, &chrono::Local),
        ResetZone::Offset(off) => next_reset_in(now, spec.hour, spec.minute, &off),
    }
}

/// The next instant at which the wall clock in `tz` reads `hour:minute`.
///
/// Resolved per calendar day in `tz` rather than by adding 24 hours, so a DST shift moves the
/// reset with the wall clock instead of sliding it an hour.
fn next_reset_in<Tz: TimeZone>(
    now: DateTime<Utc>,
    hour: u32,
    minute: u32,
    tz: &Tz,
) -> DateTime<Utc> {
    let mut date = now.with_timezone(tz).date_naive();
    // Today and tomorrow suffice; the third pass only guards a date that fails to resolve at all.
    for _ in 0..3 {
        if let Some(candidate) = resolve_in(date, hour, minute, tz) {
            if candidate > now {
                return candidate;
            }
        }
        let Some(next) = date.succ_opt() else { break };
        date = next;
    }
    now + Duration::days(1)
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

/// Parse a `HYPER_RESET_TIME` value: `HH:MM` with an optional zone — omitted or `Z`/`UTC` for
/// UTC, `local` for the machine's timezone, or a fixed `±HH:MM` / `±HHMM` / `±HH` offset.
/// Examples: `20:18`, `08:00 local`, `08:00Z`, `08:00-06:00`.
///
/// An empty value yields `None` (caller defaults to midnight UTC); a non-empty but malformed
/// value is a hard error, so a typo surfaces instead of silently shifting the reset to midnight.
fn parse_reset_time(raw: &str) -> Result<Option<ResetSpec>, UsageError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(None);
    }
    let bad = || {
        UsageError::Unsupported(format!(
            "HYPER_RESET_TIME must be HH:MM with an optional zone \
             (UTC when omitted; `Z`, `local`, or `±HH:MM`), got {raw:?}"
        ))
    };

    let (clock, zone_raw) = split_zone(raw);
    let (h, m) = clock.split_once(':').ok_or_else(bad)?;
    let hour = h.trim().parse::<u32>().ok().filter(|h| *h < 24).ok_or_else(bad)?;
    let minute = m.trim().parse::<u32>().ok().filter(|m| *m < 60).ok_or_else(bad)?;
    let zone = parse_zone(zone_raw).ok_or_else(bad)?;
    Ok(Some(ResetSpec { hour, minute, zone }))
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
/// `cycle` (the reset instant, unix seconds). Within a known cycle the cached value is
/// reused; on a new (or uncached) cycle we re-derive it but never drop below the last
/// known baseline (see [`derive_permanent`]), then persist it. The cache write is
/// best-effort — a failure just means we re-derive on the next fetch.
fn resolve_permanent(id: &str, balance: i64, cycle: i64) -> u32 {
    let stored = perm::read(id);
    if let Some(r) = &stored {
        if r.cycle == cycle {
            return r.value;
        }
    }
    let value = derive_permanent(stored.map_or(0, |r| r.value), balance);
    perm::write(id, &perm::Record { value, cycle });
    value
}

/// New-cycle baseline: `balance - DAILY` (exact only at reset, when the grant is full),
/// floored at the previously known baseline so a mid-cycle cold start cannot undercount
/// permanent credits, and at zero.
fn derive_permanent(previous: u32, balance: i64) -> u32 {
    previous.max((balance - DAILY).max(0) as u32)
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

    fn cache_path(id: &str) -> Option<PathBuf> {
        cache_dir().map(|d| d.join(format!("{id}.permanent.json")))
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
    fn reset_defaults_to_midnight_when_unset() {
        let now = chrono::Utc::now();
        let r = next_reset(now, parse_reset_time("").unwrap());
        assert!(r > now);
        assert_eq!(r.hour(), 0);
        assert_eq!(r.minute(), 0);
    }

    fn spec(hour: u32, minute: u32, zone: ResetZone) -> Option<ResetSpec> {
        Some(ResetSpec { hour, minute, zone })
    }

    /// A bare `HH:MM` must keep meaning UTC — existing configs carry no zone.
    #[test]
    fn reset_time_parses_hh_mm() {
        assert_eq!(parse_reset_time("20:18").unwrap(), spec(20, 18, ResetZone::Utc));
        assert_eq!(parse_reset_time("00:00").unwrap(), spec(0, 0, ResetZone::Utc));
        assert_eq!(parse_reset_time("   ").unwrap(), None);
    }

    #[test]
    fn reset_time_parses_an_explicit_zone() {
        assert_eq!(parse_reset_time("08:00 local").unwrap(), spec(8, 0, ResetZone::Local));
        assert_eq!(parse_reset_time("08:00local").unwrap(), spec(8, 0, ResetZone::Local));
        assert_eq!(parse_reset_time("08:00 LOCAL").unwrap(), spec(8, 0, ResetZone::Local));
        assert_eq!(parse_reset_time("08:00Z").unwrap(), spec(8, 0, ResetZone::Utc));
        assert_eq!(parse_reset_time("08:00 utc").unwrap(), spec(8, 0, ResetZone::Utc));

        let west6 = FixedOffset::west_opt(6 * 3600).unwrap();
        let east2 = FixedOffset::east_opt(2 * 3600).unwrap();
        assert_eq!(parse_reset_time("08:00-06:00").unwrap(), spec(8, 0, ResetZone::Offset(west6)));
        assert_eq!(parse_reset_time("08:00 -0600").unwrap(), spec(8, 0, ResetZone::Offset(west6)));
        assert_eq!(parse_reset_time("08:00+02").unwrap(), spec(8, 0, ResetZone::Offset(east2)));
    }

    #[test]
    fn malformed_reset_time_is_an_error() {
        for bad in [
            "20", "24:00", "12:60", "ab:cd", "-1:00", "08:00 pacific", "08:00+", "08:00-25:00",
            "08:00-06:99", "08:00 -06000",
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

        // 08:00 UTC is 5 hours out.
        assert_eq!(
            next_reset(now, spec(8, 0, ResetZone::Utc)),
            Utc.with_ymd_and_hms(2026, 7, 27, 8, 0, 0).unwrap()
        );
        // 08:00 in UTC-6 is 14:00 UTC — 11 hours out, the same calendar day.
        assert_eq!(
            next_reset(now, spec(8, 0, ResetZone::Offset(west6))),
            Utc.with_ymd_and_hms(2026, 7, 27, 14, 0, 0).unwrap()
        );
    }

    #[test]
    fn a_reset_already_past_today_rolls_to_tomorrow() {
        let west6 = FixedOffset::west_opt(6 * 3600).unwrap();
        // Mon 2026-07-27 20:00 UTC = 14:00 in UTC-6, after that day's 08:00 local reset.
        let now = Utc.with_ymd_and_hms(2026, 7, 27, 20, 0, 0).unwrap();
        assert_eq!(
            next_reset(now, spec(8, 0, ResetZone::Offset(west6))),
            Utc.with_ymd_and_hms(2026, 7, 28, 14, 0, 0).unwrap()
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
    /// hour — which is exactly what a hand-converted fixed UTC value would do.
    #[test]
    fn a_local_reset_tracks_a_dst_shift() {
        // DST ends Sun 2026-11-01: 08:00 local is UTC-4 before and UTC-5 after.
        let before = Utc.with_ymd_and_hms(2026, 10, 30, 20, 0, 0).unwrap();
        assert_eq!(
            next_reset_in(before, 8, 0, &DstZone),
            Utc.with_ymd_and_hms(2026, 10, 31, 12, 0, 0).unwrap(),
            "08:00 at UTC-4 = 12:00 UTC"
        );
        let after = Utc.with_ymd_and_hms(2026, 11, 2, 20, 0, 0).unwrap();
        assert_eq!(
            next_reset_in(after, 8, 0, &DstZone),
            Utc.with_ymd_and_hms(2026, 11, 3, 13, 0, 0).unwrap(),
            "08:00 at UTC-5 = 13:00 UTC — the wall clock held, the UTC instant moved"
        );
    }

    /// Spring-forward skips 02:00–03:00 local; a reset nominally in that gap must still fire.
    #[test]
    fn a_reset_inside_a_dst_gap_still_resolves() {
        let now = Utc.with_ymd_and_hms(2026, 3, 8, 1, 0, 0).unwrap();
        let reset = next_reset_in(now, 2, 30, &DstZone);
        assert_eq!(reset, Utc.with_ymd_and_hms(2026, 3, 8, 7, 30, 0).unwrap());
        assert!(reset > now, "a skipped wall time must not stall the reset");
    }

    /// Fall-back repeats 01:00–01:59 local; the first occurrence is the reset.
    #[test]
    fn an_ambiguous_reset_takes_the_first_occurrence() {
        let now = Utc.with_ymd_and_hms(2026, 11, 1, 0, 0, 0).unwrap();
        assert_eq!(
            next_reset_in(now, 1, 30, &DstZone),
            Utc.with_ymd_and_hms(2026, 11, 1, 5, 30, 0).unwrap()
        );
    }

    #[test]
    fn permanent_derivation_floors_at_previous_and_zero() {
        // At reset the grant is full, so balance - DAILY recovers permanent exactly.
        assert_eq!(derive_permanent(0, 610), 360);
        // Mid-cycle cold start (balance below a full grant) keeps the known baseline.
        assert_eq!(derive_permanent(360, 200), 360);
        // Nothing known and balance below the grant: zero, never negative.
        assert_eq!(derive_permanent(0, 200), 0);
        // A balance proving more permanent than we knew raises the baseline.
        assert_eq!(derive_permanent(100, 610), 360);
    }
}
