# AGENTS.md

## Project overview

`agent-usage-tray` is a cross-platform coding-agent usage monitor. A **Rust CLI** (`agent-usage`) produces a single JSON contract consumed by a **macOS menu bar app** (Swift/AppKit + SwiftUI) and a future **Linux COSMIC applet**.

```
crates/
  agent-usage-core/       Pure logic: Provider trait, normalized schema, pace math, projection,
                           usage-sample history (burn rate / burst), config file, cache/config
                           directory resolution
  agent-usage-providers/  Concrete providers (Claude, Codex, Hyper) + credential helpers + HTTP layer
  agent-usage-cli/        `agent-usage` binary: per-agent subcommands, caching, one JSON contract
macos/
  AgentUsageMenuBar/      macOS menu bar app (Swift) — spawns the CLI, renders results
  build-app.sh            Builds CLI + Swift app, assembles .app bundle
```

## Build & test

```sh
cargo build              # whole workspace (3 crates)
cargo test               # core + providers + CLI tests
cargo run -p agent-usage-cli -- claude --json
cargo run -p agent-usage-cli -- all --status
```

Build the macOS app:
```sh
macos/build-app.sh                    # release
macos/build-app.sh --debug            # debug
open macos/build/AgentUsageMenuBar.app
```

No CI, no Makefile, no `rust-toolchain.toml`. The macOS app requires Xcode Command Line Tools (Swift toolchain) in addition to Rust.

## Workspace crate graph

```
agent-usage-core  (serde, chrono, thiserror — no network, no GUI)
       ↑
agent-usage-providers  (+ ureq — blocking HTTP, credential helpers, Keychain)
       ↑
agent-usage-cli  (+ clap — arg parsing, caching, output rendering)
```

The macOS Swift app calls the CLI binary as a subprocess — it does **not** link the Rust crates. It spawns `agent-usage all --json` every 5 minutes and decodes the JSON array.

## Architecture: how data flows

1. **Provider trait** (`core/src/provider.rs`) — every agent implements `fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError>`. Three identity methods: `id()`, `label()`, `source()`.
2. **Normalized schema** (`core/src/schema.rs`) — `Usage` contains `AgentInfo` + `Vec<Window>`. Each `Window` has a `WindowKind` (Session/Weekly/Credits/Other) and a `Metric` which is either `Utilization { used_pct }` or `Pool { total, remaining, burn_per_day, budget }`. The `budget` field (added for Hyper) allows measuring consumption against a recurring daily allowance rather than the full pool — surplus beyond the allowance is "extra usage" and pushes `used_pct` past 100%. See `Window::with_budget()` constructor.
3. **Registry** (`providers/src/lib.rs`) — `all()` returns `Vec<Box<dyn Provider>>`, `get(id)` looks up by id. Adding a new agent = one module implementing `Provider` + one line in `all()` + one line in the test. Nothing else needs to change. Current providers: `claude`, `codex`, `hyper`.
4. **CLI** (`cli/src/main.rs`) — dispatches on subcommand (`claude`, `codex`, `hyper`, `all`, `list`) via the provider registry. JSON output goes through `output::build_snapshot` which applies pace coloring and pool projection, then `Snapshot::with_trend` attaches the sampled burn rate / burst (see *Trend*).
5. **macOS app** — `DataController` spawns `agent-usage all --json --work-days N --daily-budget B` as a subprocess, decodes the array of `AgentSnapshot` DTOs, renders per-agent bar indicators and a dashboard popover.

## Key patterns & conventions

### Adding a new agent provider
1. Add a module in `crates/agent-usage-providers/src/` implementing `Provider`.
2. Register it in `providers/src/lib.rs`: add `pub mod newagent;`, `pub use newagent::NewAgent;`, and `Box::new(NewAgent::new())` to `all()`.
3. Update the registry test.
4. If the agent has a brand logo, add `<id>.pdf` to `macos/AgentUsageMenuBar/Resources/agents/`.

### Error handling
- Single error type `UsageError` in `core/src/error.rs` with variants for network, auth, parse, credentials, rate-limit, etc.
- Every variant has a stable `kind()` discriminant used in the JSON contract.
- The CLI distinguishes **transient** errors (network, rate_limited, unexpected_status, parse, no_data) from **hard** errors (auth/credentials). On transient errors, the cached last-good `Usage` is served with `"stale": true` instead of an error snapshot.

### Caching (important gotchas)
- **The cache stores raw `Usage`, not the rendered `Snapshot`.** On every call, the snapshot is recomputed from cached usage against the current `budget` and `now`. This means work-days changes and live countdowns still update even when the source isn't re-fetched.
- Cache location: `~/.cache/agent-usage/<id>.json` (respects `$XDG_CACHE_HOME`).
- Default TTL: 60 seconds (`--cache-ttl`). 0 disables fresh-cache reuse but still serves stale data on transient errors.
- `--no-cache` disables both read and write entirely.
- `--status` bypasses the cache entirely (always fetches live).
- The macOS app uses `--cache-ttl 0` when the user manually refreshes, but still relies on the stale-on-error fallback.

### Pace coloring (weekly windows)
Pace is based on **today's headroom**, not cumulative ratio:
- `ceiling = work_day_index * daily_budget`
- `remaining = ceiling - used_pct`
- Thresholds scale with `daily_budget`: surplus ≥ 2x daily (banked), green > 0.5x daily, yellow > 0.25x daily, red ≤ 0.25x daily.
- Session: fixed thresholds (≤50% green, ≤80% yellow, else red).
- Credit pools: red if depletes_before_reset or ≥90%, yellow ≥75%, else green.

### Work-day counting (local timezone)
`days_into_cycle` counts work days elapsed in the user's **local** timezone. The cycle starts at `resets_at - 7 days`, split into 24h periods. Each period is attributed to the calendar day 12h into it (so evening resets map to the next day). Mon–Fri only when `work_days ≤ 5`; all days when >5. This is what makes "day N/M" match wall clock.

### Credential resolution
- Claude: `~/.claude/.credentials.json` (JSON with `claudeAiOauth.accessToken`), falls back to macOS Keychain (`security find-generic-password -s "Claude Code-credentials" -w`).
- Codex: `~/.codex/auth.json` (JSON with `tokens.access_token` and `tokens.account_id`).
- Hyper: `HYPER_API_KEY` env var. It's an **opt-in agent** — `Provider::in_default_set` returns false until the var is set, so a fresh install's `all`/`list`/menu bar show just Claude + Codex (the setup most people have). Once the key is set, Hyper joins automatically. `agent-usage hyper` always resolves directly and reports a clear "HYPER_API_KEY not set" error when the var is absent. Reset time from `FetchOptions::reset_time` (the CLI's `--reset-time`, which the macOS Settings window drives) or, when unset, the `HYPER_RESET_TIME` env var: `HH:MM` plus an optional zone — omitted or `Z`/`UTC`/`GMT` for UTC, `local` for the machine's timezone, or a fixed `±HH:MM` / `±HHMM` / `±HH` offset (e.g. `20:18`, `08:00 local`, `08:00-06:00`). **A bare `HH:MM` is UTC**, so pre-existing configs keep their meaning. Defaults to midnight UTC if unset, hard error if malformed.
- All providers support `--creds-path` override. Claude also supports `--no-keychain`/`--keychain-service` flags.
- Credential helpers (`creds.rs`) use blocking file I/O and `security` CLI — no async.

### Multiple Claude accounts (a second login as its own agent)
A second Claude Code login (e.g. a personal account under `$CLAUDE_CONFIG_DIR=~/.claude-personal`) is surfaced as a **distinct agent**, not a mode of the first. Nothing about the `Provider` trait changes — the same `Claude` provider is invoked again with overrides:
- `--config-dir <DIR>` resolves the *default* creds under `<DIR>/.credentials.json` (via `FetchOptions::creds_dir`), and — unlike `--creds-path` — **keeps the Keychain fallback** so a macOS account whose token lives only in the Keychain still resolves.
- `--keychain-service <NAME>` overrides which Keychain service to read. **This is how accounts are separated on macOS:** Claude Code namespaces the token by config dir — the default `~/.claude` uses the bare service `Claude Code-credentials`, and every other dir uses that name plus a suffix that is the first 4 bytes of `SHA-256(absolute config dir)` (e.g. `~/.claude-personal` → `Claude Code-credentials-791fb149`). The macOS app derives this (CryptoKit) in `ClaudeAccount.resolvedKeychainService` and passes it; the CLI itself does no derivation.
- `--keychain-account <ACCT>` maps to `security -a` (a further disambiguator when one service holds an entry per login). `read_keychain(service, account)` takes the optional account. Unused by the app but available to the standalone CLI.
- `--id`/`--label` remap the emitted `agent.id`/`label` (and the **cache key**, so account #2 caches to `claude-personal.json`, not clobbering `claude.json`). `apply_identity()` in `cli/src/main.rs` does this; `source` is left as the provider reports it. Only the single-agent path applies overrides — `all` passes `None`.
- **Each account must resolve a *different, valid* token** to show distinct numbers: the per-dir `.credentials.json` (Linux) or the config-dir-specific Keychain service (macOS, auto-derived). Like the primary, the CLI reads the *stored* access token and does not run Claude's refresh flow, so a stale token 401s until that account's `claude` next refreshes it.

### HTTP layer
- Blocking `ureq` (no async runtime). All requests are synchronous one-shot calls.
- `http::get()` maps HTTP status codes to `UsageError` variants: 401 → Unauthorized, 429 → RateLimited (honors Retry-After, capped at 300s), other non-2xx → UnexpectedStatus.

### Credit pools & the `budget` field

When a `Pool` carries a `budget` (set via `Window::with_budget()`), consumption is measured against the recurring allowance rather than the full `total`. The part of `total` beyond `budget` is surplus — it only counts as "used" once the daily allowance is spent. This means `used_pct` can exceed 100% ("extra usage").

- **Hyper**: `total` = permanent credits + 250 daily. `budget` = 250 (the daily recharge). Spending 100 of your 250 daily grant → 40% used. Spending 300 → 120% used (50 into surplus). The label is `"credits"` (a lowercase noun matching the other windows' convention) and the pool shows the raw balance.
- **Without** `budget` (e.g. a pure consumable pool): `used_pct = (total - remaining) / total * 100` as before.

### Settings precedence and the CLI config file

Settings resolve **flag → environment → config file → built-in default** (`resolve_work_days` /
`resolve_daily_budget` / `resolve_reset_time` in `cli/src/main.rs`).

- **Config file** (`core/src/config.rs`): `$XDG_CONFIG_HOME/agent-usage/config.json`, else
  `~/.config/agent-usage/config.json`. Deliberately the *config* dir, not the cache dir — this is
  authored state, not something the tool may discard. Keys mirror the long flags:
  `work_days`, `daily_budget`, `reset_time`. Every field is optional; an absent one means "no
  opinion" and falls through.
- **It is the lowest-precedence source**, so the macOS app passing its own settings as arguments
  keeps overriding it exactly as before. The env sits above the file because it is the more
  specific of the two ambient sources, and because that preserves what an existing
  `HYPER_RESET_TIME` export already did.
- **A missing file is fine; a broken one is fatal** (exit 2). `deny_unknown_fields` means a
  misspelled key is rejected by name rather than silently ignored — the whole point of the file is
  to stop a setting from quietly not applying.
- **`daily_budget` defaults to `100 / work_days`**, not a flat 20. A flat default silently
  under-budgets any non-default split (`work_days = 4` at 20%/day only ever spends 80% of the
  cycle). This matches `AppSettings.dailyBudget` in the Swift app. The 5-day default is still 20.0.
- Resolution helpers that read the environment are split into a pure `*_from(...)` function taking
  the env value as an argument, so tests don't depend on (or mutate) process-global state — the
  same pattern as `core::cache::resolve_cache_dir`.

### Trend: burn rate and the burst brake

A short rolling window is self-limiting — you cannot spend a week's budget in an afternoon while a
5-hour limit is also stopping you. Agents that bill a **single multi-day quota** have no such
brake, so `core/src/history.rs` reconstructs one from successive readings.

- **Sampling.** Every *live* fetch appends `{at, used_pct}` for the agent's `Weekly` window to
  `~/.cache/agent-usage/<id>.history.json` (`cli/src/history.rs`, same dir and id as the snapshot
  cache). Cache hits and stale fallbacks **read but do not append** — re-recording an unchanged
  reading would flatten the rate with samples carrying no new information. `--no-cache` opts out
  of the series entirely, so no trend is reported.
- **Derived numbers.** `burn_per_day` over the trailing 24h (needs ≥2h of history), a projected
  exhaustion timestamp, `exhausts_before_reset`, and `recent` — the **burst**, percent of the
  cycle consumed in the trailing 5h (needs ≥30 min). All of it degrades to *absent*, never to a
  guess: no `trend` key at all until there is enough history.
- **Self-healing series.** A reading more than 1% below its predecessor means the window reset, so
  the series restarts (deltas across a rollover are meaningless). Runs of identical readings
  collapse to their endpoints so idle polling doesn't grow the file. Samples older than 8 days are
  pruned; out-of-order and duplicate polls are dropped.
- **Spans are measured from the window, not the anchor.** When the anchor sample predates the
  lookback, its *value* is what utilization stood at when the window opened, so the divisor is the
  window length — otherwise a burst gets smeared over however long ago the last poll landed. A
  series that stopped updating before the window opened reports nothing rather than a stale rate.
- **Burst color** (`pace::compute_burst_color`) is measured against **one work day's budget**, not
  against what's left: ≥ a full day's budget in the burst window is Red, ≥ half is Yellow. So it
  warns on *speed* and scales with the user's `work_days` setting.
- **Frontends fold the burst into the agent's pace.** `AgentSnapshot.displayPace`/`worstPace`
  include `trend.recent.pace`, and the menu bar's two-number mode pairs the weekly reading with
  the session window *or*, for agents without one, the burst (`nearTermReading`).

### Swift app details
- CLI binary resolution order: `$AGENT_USAGE_BIN` → `Bundle.resources/agent-usage` → sibling to the .app → `PATH` via `/usr/bin/env agent-usage`.
- Settings persisted in `UserDefaults`: `workDays`, `appearance`, `disabledAgentIDs`, `menuBarMode`, `selectedAgentID`, `creditDisplay`, `hyperResetTime`, `claudeAccounts` (JSON-encoded `[ClaudeAccount]`).
- **`hyperResetTime` must be passed as `--reset-time`, not left to the environment.** A `.app`
  launched from Finder inherits no shell profile, so `HYPER_RESET_TIME` exported in `.zshrc` is
  invisible to it and Hyper would silently fall back to midnight UTC. The Settings field is
  debounced (600 ms) before triggering a refresh — it's typed a character at a time, and every
  intermediate value would otherwise spawn a CLI run and flash a parse error. Its inline feedback
  reads `controller.agents`, **not** `merged`, because the stale fallback substitutes the last
  good snapshot for an errored agent and would swallow the parse error being edited.
- **Extra Claude accounts** (`ClaudeAccount`: stable `claude-…` id + label + config dir + optional keychain-service override; `resolvedKeychainService` derives the service from the config dir). `DataController.runCLI` runs the built-in `all`, then one `agent-usage claude --json --id … --label … --config-dir … --keychain-service …` per account, appending each single snapshot. A base-`all` spawn/decode failure is fatal; an individual account that fails to spawn/decode is skipped (a per-agent *error document* decodes fine and is kept). `ClaudeAccount.makeID` derives a unique, `claude`-prefixed id so the account renders with the Claude-family glyph and never collides with the primary `claude`.
- `creditDisplay` controls how credit pools are rendered in the menu bar and dashboard: `.credits` (raw balance like "1,620"), `.percentage` (remaining %), or `.both` ("1,620 · 98%").
- Pace colors are adaptive (light/dark variants) via `NSColor(name: dynamicProvider:)`.
- Agent logos are vector PDFs under `Resources/agents/<id>.pdf`. Hyper's diamond glyph is bundled; other agents fall back to SF Symbols defined in `Assets.symbolName(forID:)`. A secondary Claude account (`Assets.isSecondaryClaude`, any `claude`-prefixed id ≠ `claude`) maps to the derived `claude-alt` variant (the burst in a ring) so two Claude accounts never render an identical glyph.
- The JSON decoder uses `.convertFromSnakeCase` and a custom ISO8601 with fractional seconds fallback.
- `LSUIElement = YES` in Info.plist keeps it out of the Dock; `.accessory` activation policy is the runtime equivalent.

### Output contract
The JSON snapshot is identical for all agents:
```jsonc
{
  "agent": { "id": "...", "label": "...", "source": "..." },
  "fetched_at": "...",
  "config": { "daily_budget": 20.0, "work_days": 5 },
  "windows": [{ "kind": "weekly|session|credits|other", "label": "...",
     "used_pct": 58.0, "remaining_pct": 42.0, "resets_at": "...",
     "resets_in_secs": 294981, "pace": "surplus|green|yellow|red",
     "pool": null }],
  "pace": { "work_day_index": 4, "daily_ceiling": 80.0, "remaining": 22.0,
            "reset_day_local": "Mon Jun 15, 8:00 PM" },
  // Absent until enough history is sampled; every inner field but `exhausts_before_reset`
  // is itself optional. See "Trend: burn rate and the burst brake".
  "trend": { "burn_per_day": 24.0, "measured_over_secs": 86400,
             "projected_exhaustion": "...", "exhausts_before_reset": true,
             "recent": { "used_pct": 20.0, "span_secs": 18000, "pace": "red" } },
  "stale": null, "stale_reason": null, "error": null
}
```
On failure: valid JSON with a non-null `error` object and exit code ≠ 0.

## Testing

- Tests are inline `#[cfg(test)]` modules within each source file.
- `agent-usage-core` tests are pure logic (no network).
- Provider tests cover identity, credential parsing, and JSON deserialization of API responses.
- The CLI output module has tests for snapshot building, pool windows, trend/burst serialization, and error snapshots.
- **To run a specific test**: `cargo test -p agent-usage-core -- pace::tests::day1_zero_is_green`

## Naming conventions

- Rust: `snake_case` for fields/variables, `PascalCase` for types, `SCREAMING_SNAKE_CASE` for constants.
- Swift: `camelCase` for properties, `PascalCase` for types. DTO property names match Rust snake_case via `.convertFromSnakeCase`.
- Provider ids are short lowercase strings (`claude`, `codex`, `hyper`).
- Pace colors as strings: `"surplus"`, `"green"`, `"yellow"`, `"red"`.

## Dependencies (workspace-level)

| Crate | Dependencies |
|-------|-------------|
| `core` | `serde`, `serde_json`, `chrono`, `thiserror` |
| `providers` | `core`, `serde`, `serde_json`, `chrono`, `ureq` |
| `cli` | `core`, `providers`, `serde`, `serde_json`, `chrono`, `clap` |
| macOS app | Swift/AppKit/SwiftUI (no third-party Swift packages) |

## Design decisions (ADRs)

- [ADR-001](docs/ADR/001-agent-agnostic-architecture.md) — Provider trait + normalized schema + workspace split.
- [ADR-002](docs/ADR/002-pace-and-work-day-model.md) — Headroom-based pace coloring, local-timezone work-day counting.
- [ADR-003](docs/ADR/003-caching-and-resilience.md) — Raw `Usage` caching, stale-on-transient-error.
- [ADR-004](docs/ADR/004-macos-frontend.md) — CLI subprocess approach, vector PDF logos, display modes.

## Common pitfalls

1. **Don't re-derive pace colors in consumers.** The `output.rs` module computes all per-window colors (via `window_color()`) and pool projections. Frontends consume the `pace` and `pool` fields from the JSON — never recompute them.
2. **Don't bypass the cache when reading from the CLI.** The Swift app uses the default 60s TTL. Force refresh only on explicit user action (`--cache-ttl 0`).
3. **Weekend handling matters.** `work_days_elapsed` skips Sat/Sun when `work_days ≤ 5` but counts them when `work_days > 5`. This is intentional and per the ADR.
4. **The cache stores serde-serialized `Usage`, not `Snapshot`.** The snapshot is recomputed on every read. If you change the `Usage` struct, old cache files will fail to deserialize (gracefully — falls through to a fresh fetch).
5. **Claude's Keychain fallback is `#[cfg(target_os = "macos")]` only.** On Linux, it only reads the file.
6. **Codex windows are classified by length, never by slot.** `rate_limit.primary_window` /
   `secondary_window` do **not** have fixed meanings — the slot that once held a rolling 5-hour
   limit now holds the weekly quota on plans that bill a single multi-day budget (with
   `secondary_window: null`). `window_kind()` reads `limit_window_seconds`: ≥ 24h → `Weekly`,
   shorter → `Session`, and the slot is only a fallback when the API omits the length. Getting
   this wrong is not cosmetic — a weekly quota misread as a session window is colored by fixed
   thresholds instead of pace, so a whole week's budget spent on day one still reads green.
   Labels follow the same source (`"5h limit"`, `"15m limit"`, `"weekly"`, `"3d limit"`); Claude
   always uses `"session"` / `"weekly"`.
11. **The trend needs history to exist, so it is absent on a cold start.** A fresh install reports
    no `trend` for ~30 min (burst) / ~2h (burn rate) of polling. That is deliberate — consumers
    must treat a missing `trend` as "not known yet", never as "not burning". Deleting
    `~/.cache/agent-usage/<id>.history.json` resets it.
12. **Only `Weekly` windows are sampled.** Session windows brake on their own and credit pools
    already carry a real `burn_per_day` from their provider, so neither is tracked.
7. **`--status` output is NOT a stable contract** — it's human-readable. Only the JSON output is the stable contract.
8. **Hyper permanent-credits cache** — Stored at `~/.cache/agent-usage/hyper.permanent.json` (uses `core::cache::cache_dir()` shared path). A `{ value, cycle }` record keyed by reset unix timestamp. On each new 24h cycle the permanent baseline is re-derived as `max(previous, balance - 250)` and persisted. Never goes below the last known value so a mid-cycle cold start doesn't undercount.
9. **Hyper's window has no `burn_per_day`** — The grant is fixed at 250/day, not an observed burn rate, so there's nothing to project. `depletes_before_reset` can still be `true` when the pool is nearly empty.
10. **Malformed `HYPER_RESET_TIME` is a hard error** — returns `UsageError::Unsupported` so typos
    don't silently shift the reset to midnight UTC. A **bare `HH:MM` still means UTC**: never
    reinterpret it as local, or every existing config silently shifts by its UTC offset. A
    zoned value resolves per local calendar day (`next_reset_in`), not by adding 24h, so a
    wall-clock reset holds through a DST change instead of sliding an hour.
13. **Hyper's reset cannot be auto-detected.** The API returns only `{"balance"}`, and a daily
    refresh is indistinguishable from a *credit purchase* — both are the balance jumping upward.
    Inferring the reset from balance jumps would mis-key the permanent baseline (`resolve_permanent`
    is keyed on the reset instant) every time credits are bought. It stays configured, not guessed.
