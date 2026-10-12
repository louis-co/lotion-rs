# Testing Lotion-rs on macOS

A hands-on test plan for the macOS work on branch `macos-parity-perf`. It is
written for a tester (human or agent with computer use) who can see the screen,
click and type. It checks what automated tests can't: how the app looks, real
keyboard routing, sign-in, and scrolling performance on large databases.

Background on what changed and why is in
[docs/MACOS_PARITY.md](docs/MACOS_PARITY.md).

## Ground rules

- **Don't enter credentials.** If Notion asks you to sign in, stop and ask the
  user to sign in themselves, then continue.
- **Don't change real content.** Create one scratch page (e.g. "Lotion test")
  for anything that edits, and delete it at the end. Only scroll/read the
  user's real databases.
- **Record evidence.** Take a screenshot for every failure and for the visual
  checks in section 1.
- **Report, don't fix.** Fill in the report template at the end. If something
  fails, note the exact steps, what you expected and what happened.

## 0. Setup

Requirements: macOS 12+, Rust, Tauri CLI (`cargo install tauri-cli --version "^2.0"`).

```bash
git clone https://github.com/louis-co/lotion-rs.git
cd lotion-rs
git checkout macos-parity-perf
cd src-tauri
cargo tauri build          # release app → target/release/bundle/macos/lotion-rs.app
cargo build                # debug binary → target/debug/lotion-rs (needed for section 7)
```

If DMG bundling fails (it scripts Finder), use `CI=true cargo tauri build`.
The `.app` is what matters.

Run the release app **from a terminal** so its log is visible:

```bash
RUST_LOG=info target/release/bundle/macos/lotion-rs.app/Contents/MacOS/lotion-rs
```

Useful paths:

| What | Where |
|---|---|
| Config (theme, zoom, tab suspension) | `~/Library/Application Support/lotion-rs/config.toml` |
| Saved tabs and window (encrypted) | `~/Library/Application Support/lotion-rs/state.json` |
| Downloads | `~/Downloads` |

To start from a clean slate, quit Lotion and delete `state.json`.

For the comparison in section 7, you also need **Notion desktop**: either
`/Applications/Notion.app` or a copy from <https://www.notion.com/desktop>.

## 1. Look and layout

Launch Lotion. Compare with Notion desktop side by side if you can.

| # | Check | Expected |
|---|---|---|
| 1.1 | Window at first launch (no `state.json`) | About 1320×860, centered. Notion's page fills everything below a 36px tab strip. There's no blank band, double title bar or gray area. |
| 1.2 | Traffic lights | The native red/yellow/green buttons sit vertically centered in the tab strip, with the first tab to their right and not overlapping them. |
| 1.3 | Tab strip | It shows the back (‹) and forward (›) buttons, the tab(s) with title and ×, and a + button. The active tab is visually raised. |
| 1.4 | Light mode (System Settings → Appearance → Light) | Strip is off-white (#F9F8F7), active tab white, Notion page white. |
| 1.5 | Dark mode | The strip is dark gray (#202020) and the page #191919. There's no white flash when opening a new tab (⌘T). |
| 1.6 | Drag the window by an empty part of the strip | The window moves. |
| 1.7 | Double-click an empty part of the strip | The window zooms (maximizes) and restores, following your macOS "double-click title bar" setting. |
| 1.8 | Resize to the minimum | It stops at 640×480 and Notion is still usable. |
| 1.9 | Enter full screen (⌃⌘F or the green button) | The strip's left padding shrinks because the traffic lights are hidden, and Notion fills the screen. Exit with ⌃⌘F. |
| 1.10 | Sidebar | Notion's sidebar starts directly under the strip and is not pushed down. |

## 2. Sign-in

Only if the app is not already signed in. Ask the user to do this part.

| # | Check | Expected |
|---|---|---|
| 2.1 | Email + code login | Works inside the app. |
| 2.2 | "Continue with Google" / "Continue with Apple" | A popup window opens and, after sign-in, closes. The main tab ends up logged in. It must not get stuck on a blank popup or open the system browser. |
| 2.3 | SSO (if the user's workspace uses Okta, Microsoft, etc.) | Same as 2.2. |
| 2.4 | Quit (⌘Q) and relaunch | Still signed in. |

## 3. Tabs and keyboard shortcuts

Click into the Notion page first, so the page has keyboard focus (not the strip).

| # | Shortcut / action | Expected |
|---|---|---|
| 3.1 | ⌘T | A new tab opens to the right of the current one, loads Notion home, and is active. |
| 3.2 | Click + | Same as ⌘T. |
| 3.3 | Open 4–5 tabs to different pages | Each tab's title follows the page title. |
| 3.4 | Click a tab | It switches instantly and the page keeps its scroll position. |
| 3.5 | ⌃Tab / ⌃⇧Tab | Next / previous tab, wrapping around. |
| 3.6 | ⇧⌘] / ⇧⌘[ and ⌥⌘→ / ⌥⌘← | Same as 3.5. |
| 3.7 | ⌘1 … ⌘8 | Select that tab (if it exists). ⌘9 selects the last tab. |
| 3.8 | ⌘W | Closes the active tab, and the tab to its right becomes active (or the left one if it was last). |
| 3.9 | ⇧⌘T (repeat a few times) | Reopens closed tabs, most recent first, at the same page. |
| 3.10 | Middle-click a tab | Closes it. |
| 3.11 | Hover a tab, click its × | Closes it. |
| 3.12 | Drag a tab left/right in the strip | It reorders, and the order survives a restart. |
| 3.13 | Notion link to another page, opened with ⌘-click | Opens in a new Lotion tab. |
| 3.14 | ⌘[ / ⌘] and the ‹ › buttons | Notion back / forward inside the current tab. |
| 3.15 | ⇧⌘H | The current tab goes to Notion home. |
| 3.16 | ⌘R | Reloads the current tab. |
| 3.17 | ⇧⌘R | Force-reloads the current tab. |

**Notion's own shortcuts must still reach the page.** These are the most
important keyboard checks, because WebKit gives the page first chance at
⌘-shortcuts and the menu only acts as a fallback. Do them in the scratch page:

| # | Shortcut | Expected |
|---|---|---|
| 3.18 | Type text, then ⌘Z, then ⇧⌘Z | Notion undoes and redoes the edit; the Notion undo stack is used, not a browser text-field undo. |
| 3.19 | ⌘K (Notion search, or link when text is selected) | Opens Notion's dialog. It does **not** open a new tab or anything else. |
| 3.20 | ⌘P | Opens Notion's search/quick-find, not a print dialog. |
| 3.21 | ⌘\\ | Toggles Notion's sidebar. |
| 3.22 | ⌘B / ⌘I / ⌘U on selected text | Bold / italic / underline. |
| 3.23 | ⌘C / ⌘V / ⌘X, ⌘A | Normal copy/paste/cut/select-all, including pasting rich text and images into Notion. |
| 3.24 | ⌘L | Notion's copy-link behavior (a toast in Notion), not anything from Lotion. |
| 3.25 | Type a misspelled word ("teh") | It gets the red squiggle, and right-clicking it offers macOS spelling suggestions. |

## 4. Window, Dock and app lifecycle

| # | Check | Expected |
|---|---|---|
| 4.1 | Click the red close button | The window disappears and Lotion stays in the Dock (it does not quit). |
| 4.2 | Click Lotion's Dock icon | The window comes back with the same tabs. |
| 4.3 | ⌘W on the last remaining tab | The window hides like 4.1. Reopening shows one fresh tab. |
| 4.4 | ⇧⌘W | Closes (hides) the window. |
| 4.5 | ⌘M, ⌘H, ⌥⌘H | Minimize, hide, and hide others all work normally. |
| 4.6 | Window → Notion (⌥⌘0) while hidden | Shows the window. |
| 4.7 | Move and resize the window, quit with ⌘Q, relaunch | It reopens at the same position and size, with the same tabs, the same active tab and the same order. |
| 4.8 | 4.7, with 5+ tabs | Only the active tab loads at launch. Other tabs show their saved titles and load the moment they are clicked. |
| 4.9 | Launch the app a second time (e.g. `open` the .app again) | No second instance; the existing window comes forward. |
| 4.10 | Move the window to an external display, quit, unplug the display, relaunch | The window appears on the remaining screen, not off-screen. Skip if there's only one display. |
| 4.11 | Lotion → Settings… (⌘,) | `config.toml` opens in the default text editor. |

## 5. Zoom and themes

| # | Check | Expected |
|---|---|---|
| 5.1 | ⌘= or ⌘+ (several times), ⌘-, ⌘0 | Zooms in and out in small (10%) steps, and ⌘0 resets. The tab strip does not zoom; only Notion does. |
| 5.2 | Zoom in, switch tabs, open a new tab | All tabs use the same zoom. |
| 5.3 | Zoom in, quit, relaunch | The zoom level is kept. |
| 5.4 | Dark mode on, then View → Dark Mode Theme → Dracula, then Nord, then Notion (Default) | Notion's dark colors change immediately in all tabs, and the choice survives a restart. In light mode, themes have no effect (by design). |

## 6. Links, embeds, downloads

| # | Check | Expected |
|---|---|---|
| 6.1 | Click an external link (e.g. to `https://example.com`) in a page | It opens in the default browser. The Lotion tab stays on the Notion page. |
| 6.2 | Add a YouTube embed (`/video` with a YouTube URL) to the scratch page, then play it | It plays inline. No browser window opens. |
| 6.3 | A Figma, Loom, Google Maps or GitHub Gist embed, if available | It renders inline. |
| 6.4 | `•••` → Export the scratch page (Markdown or PDF) | The file lands in `~/Downloads` and a "Downloaded …" notice appears briefly in the tab strip. |
| 6.5 | Upload an image to the scratch page (drag & drop and `/image` → Upload) | Works, including the file picker. |
| 6.6 | Open a public `*.notion.site` page link | It opens in a Lotion tab and is not blocked. |
| 6.7 | Notion → Settings → "Connections" / integrations that open OAuth popups (only if already set up; don't add new ones) | The popup opens and completes. |

## 7. Large databases: smoothness and stability

This section is the main point of the work. Use the **biggest database** in the
workspace, ideally 1,000+ rows with several properties, in table view. If none
exists, ask the user which one to use, or skip 7.4–7.6 and note it.

Subjective checks (release build):

| # | Check | Expected |
|---|---|---|
| 7.1 | Open the database and scroll quickly from top to bottom with the trackpad, then fling | Rows render without long blank stretches, with no freezes over ~0.5s and no beachball. |
| 7.2 | Board and gallery views of the same database | Same as 7.1. |
| 7.3 | While the big database is open in one tab, have 3–4 other tabs open. Drag-resize the window continuously for ~5 seconds | Resizing stays fluid and the database reflows. There's no long stall after letting go. |
| 7.4 | Switch rapidly between the big database tab and other tabs | Switching is instant, and the database is still rendered and scrolled where you left it. |
| 7.5 | Open a row as a side peek / center peek, scroll inside it, close it | Smooth, with no blank page afterwards. |
| 7.6 | Leave the app with 5+ heavy tabs for a while and watch Activity Monitor (Memory column, processes "lotion-rs" and "com.apple.WebKit.WebContent") | Memory stays reasonable (a few hundred MB per heavy tab) and no tab turns permanently blank. A blank tab must recover with ⌘R. |

Measured comparison with the scroll benchmark. Run the **same view of the same
database** in both apps, at the same window size:

1. **Lotion (debug build):**
   ```bash
   cd src-tauri && RUST_LOG=info target/debug/lotion-rs
   ```
   Open the database, then press ⌥⌘I to open Web Inspector on the active tab.
   Paste the contents of `docs/benchmarks/scroll-bench.js` into the console and
   press Enter. Leave the window alone until it finishes (a few seconds), then
   record the result table.

   Alternatively, run
   `LOTION_SCROLLBENCH=30 RUST_LOG=info target/debug/lotion-rs` and open the
   database within 30 seconds. The benchmark runs automatically in the active
   tab and its result is logged in the terminal.
2. **Notion desktop:** open the same view, View → Toggle Developer Tools (⌥⌘I)
   → Console, paste the same script, and record the result.
3. Run each 3 times and keep the median run. Close other heavy apps first.

Record `p50ms`, `p95ms`, `p99ms`, `maxMs`, `jankPct`, `avgFps`, and the max
rendered rows for both apps. Lower ms / jank and higher fps are better. A result
of `{ error: 'no scrollable Notion view found' }` means the page wasn't a
scrollable database view; scroll the view once by hand and retry.

## 8. Known gaps (not bugs)

Don't report these as failures; they are documented as not implemented:

- `notion://` deep links, a menu-bar (tray) icon, global shortcuts (⇧⌘K / ⇧⌘J),
  an unread badge on the Dock icon, multiple windows.
- Notion's desktop-only local SQLite cache. Lotion relies on the web version's
  browser storage instead, so the first load of a huge workspace may be slower
  than in Notion desktop. Later loads should be comparable.
- A crashed tab does not reload itself; ⌘R recovers it.
- Developer Tools (⌥⌘I) only work in debug builds.
- The spellcheck language follows macOS settings, not Lotion's config.

## Report template

```markdown
# Lotion-rs macOS test report

- Commit: <git rev-parse --short HEAD>
- macOS version / Mac model:
- Notion desktop version (for comparison):
- Signed in: yes/no; largest database tested: <name, ~row count, view>

## Results

| # | Result | Notes |
|---|---|---|
| 1.1 | ✅ / ❌ / ⏭ skipped | |
| ... | | |

## Failures (one block per failure)

### <#> <short title>
- Steps:
- Expected:
- Actual:
- Screenshot:
- Terminal log lines around the time (if relevant):

## Scroll benchmark (median of 3)

| | p50 ms | p95 ms | p99 ms | max ms | jank % | avg fps | max rows |
|---|---|---|---|---|---|---|---|
| Lotion | | | | | | | |
| Notion desktop | | | | | | | |

## Subjective notes
Anything that felt slower, uglier or different from Notion desktop.
```
