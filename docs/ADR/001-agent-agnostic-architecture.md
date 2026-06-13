# ADR-001: Agent-Agnostic Architecture

| Field      | Value                                    |
| ---------- | ---------------------------------------- |
| Status     | Accepted                                 |
| Date       | 2026-06-12                               |
| Supersedes | `claude-code-usage-indicator` (eventual) |
| Authors    | fuabioo                                  |

## Context

`claude-code-usage-indicator` ships a Rust core + CLI + COSMIC applet and a Swift macOS app that
monitor **Claude Code** usage. It works well, but its data model is Claude-specific (a fixed
`seven_day` + `five_hour` response). We want one tool that monitors **any** coding agent —
Claude, Codex, and others — with identical UI and CLI surface, while each agent keeps its own
way of reporting usage. The `Agent Usage Prototype (standalone).html` design makes this explicit:
a multi-agent menu bar, per-agent dashboard gauges, and a settings list where each agent declares
its own source ("via cc-usage CLI", "via local config", "via gcloud auth", "via hyper.charm.land").

## Decision

A Cargo workspace with a **pure-logic core**, a **providers** crate, and a **CLI**; GUIs (macOS,
COSMIC) come later and reuse the same crates / JSON contract.

### 1. Reuse Rust; keep dependencies minimal

Rust is retained — the COSMIC applet *requires* it, and the existing pace logic ports directly.
The CLI is one-shot, so it uses a tiny **blocking `ureq`** client instead of `reqwest` + `tokio`.
The core carries **no** network or GUI dependencies (only `serde`/`chrono`/`thiserror`), keeping
it trivially testable and linkable from any frontend.

### 2. A `Provider` trait per agent

```rust
trait Provider {
    fn id(&self) -> &'static str;      // "claude" — also the CLI subcommand
    fn label(&self) -> &'static str;   // "Claude Code"
    fn source(&self) -> &'static str;  // "Anthropic OAuth usage API"
    fn fetch(&self, opts: &FetchOptions) -> Result<Usage, UsageError>;
}
```

Providers live in `agent-usage-providers` behind a registry (`all()` / `get(id)`). Adding an
agent is a module + one registry line; it then appears in `list`, `<id>`, and `all` everywhere
with no downstream changes. `FetchOptions` is one shared struct (creds path, timeout, keychain);
providers use only the fields relevant to their source.

### 3. One normalized schema covering both usage shapes

Agents measure usage in fundamentally different ways, so the schema is designed up front for both:

- **Percent-utilization** windows (Claude's 5h session, weekly window; Codex's "5h limit").
- **Consumable credit pools** (a balance that burns down, may run dry before it refills).

```rust
struct Usage { agent: AgentInfo, windows: Vec<Window> }
struct Window { kind: WindowKind, label: String, metric: Metric, resets_at: Option<DateTime<Utc>> }
enum WindowKind { Session, Weekly, Credits, Other }
enum Metric {
    Utilization { used_pct: f64 },
    Pool { total: f64, remaining: f64, burn_per_day: Option<f64> },
}
```

Every window can report `used_pct` / `remaining_pct` regardless of metric, so downstream code
never special-cases an agent. This is the chief generalization over the old fixed model.

### 4. Pace, projection, and color in the core

> **Superseded by [ADR-002](002-pace-and-work-day-model.md).** The weekly ratio thresholds and
> UTC weekday counting below were replaced by today's-headroom bands (plus a surplus state) and a
> local-timezone work-day model. Projection and session/pool coloring are unchanged.

- **Weekly** → pace algorithm (ported verbatim, incl. weekday counting): ceiling =
  `work_day_index * daily_budget`; ratio `<0.75` green, `<1.0` yellow, else red.
- **Session** → fixed thresholds (`≤50` / `≤80`).
- **Credit pool** → burn-rate projection: `depletes_at = now + remaining/burn_per_day`; color is
  red if it depletes before reset (or `≥90%` used), yellow at `≥75%`. This powers the prototype's
  "out ~Thu at this rate" alert.

### 5. Per-agent subcommands, one output contract

`agent-usage <agent>` (e.g. `agent-usage claude --json`), plus reserved `list` and `all`. The
JSON document has the same shape for every agent — `agent`, `config`, `windows[]`, optional
weekly `pace` summary, and an `error` that is null on success. On failure the CLI still emits
valid JSON with an `error` object and exits non-zero, so GUI callers can always parse it. This
contract is what the macOS app and (optionally) the COSMIC applet consume.

## What we kept from the old project

- The pace-coloring algorithm and weekday-counting logic (ported with its full test suite).
- The credential strategy: read the agent's own credentials file, with a macOS Keychain fallback.
- The "valid JSON even on error, non-zero exit" CLI contract the macOS app relies on.

## Consequences

**Positive:** new agents are isolated, low-risk additions; the core is dependency-light and fully
unit-tested without network/credentials; the JSON contract is forward-compatible with credit-pool
agents already. **Negative:** a normalized schema is a thin abstraction every agent must map onto
(fine so far); some agents may expose usage shapes that need a new `WindowKind`/`Metric` variant
later. **Risks:** per-agent sources vary widely (API, local file, `gcloud`, remote URL) — the
`Provider` trait is the seam that absorbs that variance; agents whose APIs change are isolated to
their own module.

## Alternatives considered

- **Separate binaries per agent** (`cc-usage`, `codex-usage`): rejected — contradicts the "one
  surface, one output" goal and duplicates the contract.
- **`--agent <id>` flag instead of subcommands**: equivalent on the command line; subcommands
  read better and map 1:1 to the registry.
- **Defer the credit-pool model**: rejected — designing `windows[]` + `Metric::Pool` now avoids a
  breaking change to the JSON contract once a credit-based agent lands.
- **Non-Rust core (Go/TS)**: rejected — COSMIC requires Rust and the existing core ports directly;
  a second language would mean two cores to keep in sync.
