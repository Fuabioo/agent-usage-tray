# AGENTS.md

## Project overview

`agent-usage-tray` is a cross-platform coding-agent usage monitor. A **Rust CLI** (`agent-usage`) produces a single JSON contract consumed by a **macOS menu bar app** (Swift/AppKit + SwiftUI) and a future **Linux COSMIC applet**.

```
crates/
  agent-usage-core/       Pure logic: Provider trait, normalized schema, pace math, projection,
                           cache directory resolution
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
4. **CLI** (`cli/src/main.rs`) — dispatches on subcommand (`claude`, `codex`, `hyper`, `all`, `list`) via the provider registry. JSON output goes through `output::build_snapshot` which applies pace coloring and pool projection.
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
- Hyper: `HYPER_API_KEY` env var (required). Reset time from `HYPER_RESET_TIME` env var (`HH:MM` UTC, e.g. `20:18`; defaults to midnight UTC if unset, hard error if malformed).
- All providers support `--creds-path` override. Claude also supports `--no-keychain`/`--keychain-service` flags.
- Credential helpers (`creds.rs`) use blocking file I/O and `security` CLI — no async.

### HTTP layer
- Blocking `ureq` (no async runtime). All requests are synchronous one-shot calls.
- `http::get()` maps HTTP status codes to `UsageError` variants: 401 → Unauthorized, 429 → RateLimited (honors Retry-After, capped at 300s), other non-2xx → UnexpectedStatus.

### Credit pools & the `budget` field

When a `Pool` carries a `budget` (set via `Window::with_budget()`), consumption is measured against the recurring allowance rather than the full `total`. The part of `total` beyond `budget` is surplus — it only counts as "used" once the daily allowance is spent. This means `used_pct` can exceed 100% ("extra usage").

- **Hyper**: `total` = permanent credits + 250 daily. `budget` = 250 (the daily recharge). Spending 100 of your 250 daily grant → 40% used. Spending 300 → 120% used (50 into surplus). The label is `"credits"` (a lowercase noun matching the other windows' convention) and the pool shows the raw balance.
- **Without** `budget` (e.g. a pure consumable pool): `used_pct = (total - remaining) / total * 100` as before.

### Swift app details
- CLI binary resolution order: `$AGENT_USAGE_BIN` → `Bundle.resources/agent-usage` → sibling to the .app → `PATH` via `/usr/bin/env agent-usage`.
- Settings persisted in `UserDefaults`: `workDays`, `appearance`, `disabledAgentIDs`, `menuBarMode`, `selectedAgentID`, `creditDisplay`.
- `creditDisplay` controls how credit pools are rendered in the menu bar and dashboard: `.credits` (raw balance like "1,620"), `.percentage` (remaining %), or `.both` ("1,620 · 98%").
- Pace colors are adaptive (light/dark variants) via `NSColor(name: dynamicProvider:)`.
- Agent logos are vector PDFs under `Resources/agents/<id>.pdf`. Hyper's diamond glyph is bundled; other agents fall back to SF Symbols defined in `Assets.symbolName(forID:)`.
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
  "stale": null, "stale_reason": null, "error": null
}
```
On failure: valid JSON with a non-null `error` object and exit code ≠ 0.

## Testing

- Tests are inline `#[cfg(test)]` modules within each source file.
- `agent-usage-core` tests are pure logic (no network).
- Provider tests cover identity, credential parsing, and JSON deserialization of API responses.
- The CLI output module has tests for snapshot building, pool windows, and error snapshots.
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
6. **Session window labels are dynamic for Codex** (`"5h limit"`, `"15m limit"`, or `"session"`) based on `limit_window_seconds`. Claude always uses `"session"`.
7. **`--status` output is NOT a stable contract** — it's human-readable. Only the JSON output is the stable contract.
8. **Hyper permanent-credits cache** — Stored at `~/.cache/agent-usage/hyper.permanent.json` (uses `core::cache::cache_dir()` shared path). A `{ value, cycle }` record keyed by reset unix timestamp. On each new 24h cycle the permanent baseline is re-derived as `max(previous, balance - 250)` and persisted. Never goes below the last known value so a mid-cycle cold start doesn't undercount.
9. **Hyper's window has no `burn_per_day`** — The grant is fixed at 250/day, not an observed burn rate, so there's nothing to project. `depletes_before_reset` can still be `true` when the pool is nearly empty.
10. **Malformed `HYPER_RESET_TIME` is a hard error** — returns `UsageError::Unsupported` so typos don't silently shift the reset to midnight UTC.
