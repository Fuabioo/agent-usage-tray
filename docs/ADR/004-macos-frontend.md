# ADR-004: macOS Frontend

| Field      | Value                                |
| ---------- | ------------------------------------ |
| Status     | Accepted                             |
| Date       | 2026-06-12                           |
| Depends on | ADR-001 (JSON contract)              |
| Authors    | fuabioo                              |

## Context

The macOS menu bar app is the first GUI frontend (priority 2; the COSMIC applet is priority 3).
It must reproduce the prototype: a multi-agent menu bar, a "Today's pace" dashboard with per-agent
ring gauges and burn-rate alerts, and a settings panel. The core/providers are Rust; the question
is how a Swift app should consume them and how to render crisp, pace-tinted brand glyphs.

## Decision

### 1. Consume the CLI's JSON; don't link the Rust core

The app **spawns the bundled `agent-usage all --json`** and decodes the snapshot array — it does
not link the Rust library. The JSON contract (ADR-001) is the seam, so the frontend stays in pure
Swift and the CLI remains the single source of truth for fetching, pace, and projection. The CLI
is found via `$AGENT_USAGE_BIN` → bundled `Resources/agent-usage` → next to the executable →
`PATH`. `build-app.sh` assembles `AgentUsageMenuBar.app` with the CLI (and logo PDFs) inside,
ad-hoc signed.

### 2. AppKit `NSStatusItem` + SwiftUI, not `MenuBarExtra`

The bar item must show **colored** (non-template) text/glyphs tinted by pace, which `MenuBarExtra`
fights. So an AppKit `NSStatusItem` draws the bar content into a drawing-handler `NSImage` (re-run
in the button's current appearance, so adaptive pace colors resolve and the bar re-draws on
Light/Dark toggle), and a SwiftUI popover/settings window via `NSHostingController`.

### 3. Menu-bar display modes

A persisted `MenuBarMode` setting drives what the bar shows: *icon only*, *worst metric*,
*icon + worst*, *per-agent %*, *per-agent · both windows* (default), *only yellow/red* (an
attention filter), and *selected agent only*. Agents are separated by a faint divider. A
per-agent `displayPace` (red/yellow attention dominates, else surplus, else green) decides the
glyph tint, so a surplus agent's glyph goes mint.

### 4. Vector-PDF logos, tinted per pace

Each agent's brand SVG is rendered to a **transparent vector PDF** by `render-logos.sh` (headless
Chrome `--print-to-pdf` — it honors `viewBox`, `em` sizing, and `fill-rule` knockouts that AppKit's
CoreSVG mis-positioned and QuickLook baked onto white). The app loads `<id>.pdf` as a resolution-
independent `NSPDFImageRep` and tints it: Core Graphics (`sourceAtop`) for the menu bar, a `.mask`
over the full-resolution image for the dashboard — both crisp at any size, where SwiftUI's
`.renderingMode(.template)` looked grainy. Agents without a bundled logo fall back to an SF Symbol.

### 5. Settings + resilience details

Appearance, work days, per-agent enable, and the menu-bar mode persist in `UserDefaults`. Manual
**Refresh** forces a live fetch (`--cache-ttl 0`, see ADR-003); the footer flags stale data.

## Consequences

- **Positive:** the frontend is decoupled from Rust (JSON only), so the same contract will back
  the COSMIC applet; colored bar glyphs and crisp vector logos work across Light/Dark and any
  size; display modes cover the prototype's options.
- **Negative:** spawning a subprocess per poll (vs. linking) costs a process launch — negligible
  at a 5-minute cadence and deduped by the cache. The app is build-from-source (no notarized
  release yet).

## Lessons (bugs worth remembering)

- **`@Published` reentrancy.** A subscriber that synchronously reads a `@Published` property while
  it is mid-publish crashes (`EXC_BAD_ACCESS` in `swift_dynamicCast`). Defer such subscribers with
  `.receive(on: RunLoop.main)`, and never reassign a `@Published` inside its own `didSet`.
- **Don't fight CoreSVG.** Rendering the SVGs in a real browser engine (to PDF) was the only
  reliable way to get correct, transparent, knockout-preserving glyphs.

## Alternatives considered

- **`MenuBarExtra` (SwiftUI)** — rejected: awkward for colored, custom-drawn bar content.
- **Link the Rust core via a C ABI / UniFFI** — rejected: the JSON CLI is a simpler, already-
  required seam; linking would couple the app to Rust and duplicate the contract.
- **Bundle logos as PNGs** — rejected: grainy when downscaled in the rings; vector PDF stays crisp.
