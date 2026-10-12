# macOS: parity with Notion desktop, stability and performance

This document records how Lotion-rs was brought to rough feature parity with the
official Notion desktop app on macOS, and what was changed for stability and
rendering performance (especially with large databases).

## Method

Following the [REA](https://github.com/morluto/rea) workflow for Electron apps:

1. Downloaded the official Notion desktop app (7.37.1, Electron 43.6.0) and unpacked
   `app.asar` (`npx @electron/asar extract`).
2. Ran REA's static analysis
   (`npx rea-agents analyze-javascript-application Notion.app --integrity-policy record-and-continue`;
   the integrity policy is needed because Apple code-signing modifies unpacked native addons).
3. Read the main-process bundle for window options, tab model, menus/accelerators,
   navigation allowlists, memory management and Chromium switches.
4. Reimplemented the relevant behavior in Tauri/WKWebView and verified it on macOS
   with a debug-only in-page probe (`LOTION_DIAG`) and a scripted self-test
   (`LOTION_SELFTEST`), since Notion pages intentionally have no IPC access.

No Notion code is included in this repository.

## Architecture change

| Before | After |
|---|---|
| A frameless window; a fake title bar, tab list and window buttons were injected into every Notion page and pushed Notion's sidebar down by 38px. | The main window's own webview is a small bundled **tab strip** (`assets/index.html`) with the **native** traffic lights (`titleBarStyle: Overlay`, inset to centre them in the 36px strip, like Notion's `hiddenInset`). Each tab is a separate Notion child webview below it. |
| Injected UI called `window.__TAURI__.invoke`, which does not exist in Tauri v2 remote pages. Tabs never rendered and title updates never arrived. | The strip uses Tauri IPC; titles/URLs come from native WebKit callbacks (`on_document_title_changed`, `on_page_load`). |
| Notion's remote origin was in the IPC trust list. | **Only the bundled strip can invoke commands.** Notion pages get no IPC at all (smaller attack surface, no injected bridge). |
| Theme, title observer and network monitor were injected with `eval` right after webview creation, i.e. into the blank pre-navigation document, so they never reached Notion. | The theme is a document-start user script (survives reloads/navigation). The fetch/XHR monkey-patching network monitor was removed. |

## Feature parity (macOS)

| Notion desktop | Lotion-rs |
|---|---|
| Native traffic lights in a 36px tab bar | ✅ |
| Default 1320×860, min 640×480, window position/size restored | ✅ (off-screen positions are re-centred) |
| Tabs: ⌘T new, ⌘W close, ⇧⌘T reopen closed, ⌃Tab / ⌃⇧Tab, ⇧⌘] / ⇧⌘[, ⌥⌘→ / ⌥⌘←, ⌘1–⌘8, ⌘9 = last | ✅ |
| Drag to reorder tabs, middle-click to close | ✅ |
| ⌘W on the last tab closes the window; closing the window keeps the app in the Dock; Dock click reopens | ✅ |
| ⌘[ / ⌘] back/forward, ⌘R reload, ⇧⌘R force reload (bypasses cache) | ✅ |
| App-wide zoom ⌘+ / ⌘= / ⌘- / ⌘0, 0.5–4.9 in 0.1 steps, persisted | ✅ |
| ⌃⌘F full screen, ⌘M minimise, ⌘H / ⌥⌘H hide, standard Edit menu | ✅ (the old menu bound ⌘H to "Home", which shadowed macOS Hide) |
| Only the active tab loads at launch; others load when selected | ✅ |
| Inactive tabs unloaded after 1h, 3 most recent always kept | ✅ (`suspend_inactive_tabs_after_minutes`, default 60, 0 disables) |
| Embeds (YouTube, Figma, Loom, …) work | ✅ (they were blocked, and each one tried to open the system browser) |
| External links → system browser; Notion links → new tab | ✅ |
| SSO sign-in stays in-app (Google, Apple, Microsoft, Okta, OneLogin, JumpCloud, Duo, Atlassian) | ✅ (real popup windows keep `window.opener`) |
| Downloads/exports | ✅ (saved to ~/Downloads with a notification in the tab strip; previously cancelled) |
| Public `*.notion.site` pages | ✅ (were blocked) |
| No white flash in dark mode | ✅ (page background painted #191919 / #FFFFFF before first frame) |
| Spell check | ✅ native macOS spell checking (Hunspell is no longer linked on macOS) |
| Service-worker asset cache (`WantsServiceWorker` UA token) | ✅ token sent; WKWebView exposes `navigator.serviceWorker` |
| Local SQLite cache via `__electronApi` (better-sqlite3) | ❌ Not reimplemented. Notion's web build uses WASM SQLite on OPFS + SharedWorker instead; both are available in Lotion's WKWebView (verified). |
| `notion://` deep links, menu-bar icon, global shortcuts (⇧⌘K, ⇧⌘J), Dock unread badge, multiple windows | ❌ Not implemented (`notion://` would also conflict with an installed Notion app). |
| Crash recovery (auto-reload a crashed tab) | ❌ Tauri does not expose WebKit's web-content-process-terminated hook; ⌘R recovers a crashed tab. |

Keyboard routing: WKWebView hands ⌘-shortcuts to the page first and only
unhandled ones reach the menu (`WebViewImpl::performKeyEquivalent`). So Notion's
own shortcuts (⌘Z undo stack, ⌘\\, ⌘L, ⌘P, ⌘K, …) keep working, and the menu
accelerators above act as fallbacks.

## Stability and performance changes, with measurements

All numbers were measured on Apple Silicon, macOS 27, release builds. The baseline is
the upstream code at `79b434d` with only the macOS build fix applied.

### UI-thread stalls when moving or resizing the window

Every `Moved`, `Resized` and `Focused` event used to derive the state encryption key
(PBKDF2-SHA256, 100k iterations), encrypt, and write `state.json` **on the UI
thread**. Now the key is derived once, and saves are coalesced (500ms) and written
atomically (temp file + rename) on a background thread.

| | per window event, UI thread |
|---|---|
| Before | **35.3 ms** (≈ 2 s of blocked UI per second of dragging at 60 events/s) |
| After | **0.002 ms** (enqueue); the background write takes 0.19 ms once per burst |

### Live resize with several tabs open

The old code resized every tab's webview on every resize event, so hidden tabs with
large databases re-laid-out continuously. Now only the visible tab is resized;
hidden tabs are laid out once, when shown. Measured with 5 tabs and 40 consecutive
window size changes: the visible tab saw 4 (coalesced) resize events, and **every
hidden tab saw 0**.

### Memory with restored tabs

Same saved state with 5 tabs, ~35 s after launch:

| | WebKit + app processes | total RSS | total physical footprint |
|---|---|---|---|
| Before (all tabs load eagerly) | 8 | 1,097 MB | **613 MB** |
| After (only the active tab loads) | 5 | 441 MB | **179 MB** |

These were logged-out landing pages. Tabs holding large databases cost much more each,
so the saving grows with real workspaces. Idle-tab suspension caps memory later in
the session the same way.

### Rendering large databases

Rendering inside a page is WebKit's job. What the wrapper controls:

- Nothing is injected into Notion pages except the theme CSS. In particular, the
  per-request fetch/XHR wrapper is gone.
- Lotion identifies as the Safari version installed on the system, plus Notion
  desktop's `WantsServiceWorker` token. WKWebView's default UA omits
  `Version/… Safari/…`, which makes sites treat it as an unknown embedded browser.
- Hidden tabs use WebKit's *throttle* policy. Visible-tab work is never competing
  with relayout in hidden tabs.
- OPFS and SharedWorker, which Notion's WASM-SQLite cache needs, are available
  (verified in-page).

To compare scrolling on your own large databases, use
[`benchmarks/scroll-bench.js`](benchmarks/scroll-bench.js). It runs unchanged in
Lotion (⌥⌘I in a debug build, or `LOTION_SCROLLBENCH=20` to run and log it
automatically), in Notion desktop (⌥⌘I) and in Safari. It reports p50/p95/p99 frame
times, jank percentage and peak rendered rows.

## Debug-build diagnostics

Debug builds only; this code is compiled out of release builds.

| Env var | Effect |
|---|---|
| `LOTION_DIAG=<secs>` | Each tab logs URL, viewport, UA, OPFS/SharedWorker/service-worker status, rendered rows, resize count, etc. every N seconds. |
| `LOTION_SELFTEST=1` | Drives new/close/reopen/select/move/zoom/reload/suspend/resize and logs the tab list after each step. |
| `LOTION_SCROLLBENCH=<delay>` | Runs the scroll benchmark in the active tab after `delay` seconds. |
