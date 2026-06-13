# ADR-003: Snapshot Caching and Stale-on-Error

| Field   | Value      |
| ------- | ---------- |
| Status  | Accepted   |
| Date    | 2026-06-12 |
| Authors | fuabioo    |

## Context

The macOS app polls `agent-usage all --json` on every launch **and** every 5 minutes, once per
agent. During development (dozens of rebuild/relaunch cycles) this tripped the Anthropic usage
API's rate limit (HTTP 429). Two failure modes resulted:

1. **We hammered the endpoint** — rapid repeated calls each hit the live API.
2. **A single transient blip turned an agent into a bare "error"** — a 429 or network hiccup
   produced an error snapshot, even though we had perfectly good data moments earlier. The macOS
   app keeps an in-memory `lastGood`, but a *fresh* launch during an outage has no fallback.

The original tool's design (its ADR-002) explicitly called for a cache with stale-on-error; we
hadn't carried that over.

## Decision

A small **on-disk, per-agent cache** in the CLI, at `~/.cache/agent-usage/<id>.json`
(`$XDG_CACHE_HOME` honored), keyed by agent id with the file's mtime as its age. It does two jobs:

1. **Dedupe within a TTL.** A fetch whose cached snapshot is younger than `--cache-ttl`
   (default 60 s) reuses the cache instead of re-hitting the source. Normal 5-minute polls always
   fetch fresh; only rapid repeats (relaunch storms) are served from cache, which is what stops
   the hammering.

2. **Serve stale on a transient failure.** If a fetch fails with a *transient* error
   (`network`, `rate_limited`, `unexpected_status`, `parse`, `no_data`) and a cache exists, the
   last good snapshot is emitted instead — marked `"stale": true` with a `"stale_reason"`, exit
   code 0. Errors the user must act on (`unauthorized`, the `credentials_*` family, `unsupported`)
   still surface as error snapshots.

`--status` always fetches live (a human can retry); `--no-cache` disables both behaviors.

The cache stores the agent's **raw `Usage`** (the normalized windows), not the rendered
snapshot. The snapshot — pace, work-day index, reset countdowns — is **recomputed from the cached
usage on every call** against the current `budget` and `now`. So a fresh-cache hit still reflects
the latest work-days setting and live countdowns (an earlier design that cached the *rendered*
snapshot showed a stale work-day denominator for up to a TTL after the setting changed, and froze
the countdowns at cache time). The core schema (`Usage`/`Window`/`Metric`/…) derives
`Serialize`/`Deserialize` for this.

**macOS integration.** The app's timer/launch polls use the default TTL; the manual **Refresh**
button (and the right-click menu item) force a live fetch with `--cache-ttl 0`, which skips the
fresh-cache reuse but *still* falls back to stale on a transient error. The dashboard footer shows
a "cached" marker when any displayed agent is stale.

## Consequences

- **Positive:** the app stops tripping rate limits from rapid relaunches; a transient blip shows
  last-known data (honest `fetched_at`, absolute reset times still correct) instead of "error";
  the whole thing is a CLI concern — frontends benefit for free.
- **Negative:** a `--cache-ttl`-window of staleness on reads (≤ 60 s by default), and a stale
  snapshot computed its pace with the budget config of its cache time; both are acceptable for a
  short window. A snapshot can't be served stale before the cache is first populated — the
  *current* outage still shows an error until one success lands.

## Alternatives considered

- **Rely only on the app's in-memory `lastGood`** — rejected: a fresh launch during an outage has
  no fallback; nothing persists across process restarts.
- **Cache the rendered snapshot instead of raw usage** — simpler (no core `Serialize`), but it
  bakes the work-days config and reset countdowns in at cache time: changing the work-days setting
  showed a stale "day N/M" until the TTL expired, and countdowns froze. Rejected in favor of
  caching raw `Usage` and recomputing, which makes the core types `Serialize`/`Deserialize` (a
  small, clean addition) and keeps every render config- and time-correct.
- **Back off polling on 429 instead of caching** — addresses hammering but not the "shows error"
  symptom, and the real spike came from launch-time fetches, not the 5-minute cadence.
