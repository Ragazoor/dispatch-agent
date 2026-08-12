# 3990 — Unify feed-stdout parsing across the three feed paths

## Correction to the task premise

The task description states that the manual "r" path *"skips `parse_feed_items`' tag
validation"* and that `parse_feed_items` *"warns per-item rather than failing the whole
emission on some malformed input"*. **Both claims are false as of `7c6f4288`.**

`src/feed/parse.rs::parse_feed_items` is, in its entirety:

```rust
pub(super) fn parse_feed_items(bytes: &[u8]) -> anyhow::Result<Vec<FeedItem>> {
    serde_json::from_slice(bytes).map_err(Into::into)
}
```

It is a bare `serde_json` call with an `anyhow` conversion. It holds **no** validation
logic and **no** per-item warning logic of its own. Every rule the task attributes to it
actually lives in the `Deserialize` impl on `FeedItem` (`src/models/tasks.rs:359`):

- **Strict `tag`** — `tag: TaskTag` has no `#[serde(default)]`, so a missing or unknown
  tag fails the whole array. This is a property of the type, so **all three call sites
  already reject it identically**. `tests/cli.rs::verify_feed_missing_tag_fails` and
  `verify_feed_invalid_tag_fails` already prove it for the CLI path.
- **Lenient `signals`** — the drop-with-warning exception is
  `deserialize_lenient_signals` (`src/models/tasks.rs:428`), wired via
  `#[serde(deserialize_with = …)]`. Also a property of the type, so also already shared
  by all three paths.

So there is no behavioural divergence to fix. The three parses are:

| Path | Site | Call |
|---|---|---|
| auto-poll | `src/feed/mod.rs::FeedJob::run` | `parse::parse_feed_items(&stdout)` |
| manual "r" | `src/runtime/epics.rs::exec_trigger_epic_feed` | `serde_json::from_slice(&output.stdout)` |
| CLI | `src/main.rs::cmd_verify_feed` | `serde_json::from_str(stdout.trim())` |

`from_str(…trim())` vs `from_slice(…)` is a no-op difference — `serde_json` already
tolerates surrounding whitespace.

There is a **fourth** `serde_json`-on-`FeedItem` site, `src/setup/plugins.rs:435`, inside
`#[test] fn installed_example_script_emits_empty_feed_item_array`. It is test-only — it
asserts the shipped example script emits `[]` — not a production parse path, so it is
knowingly **excluded** from this unification. Noted here so a future reader can tell it
was seen rather than missed.

### What the real problems are

Re-scoping to what is actually broken:

1. **Structural drift risk (the legitimate core of the task).** Three literal
   `serde_json` call sites is exactly the shape learning #284 forbids and exactly the
   shape that produced the `reviews_parent` bug (#3781/#3782) in the sync layer. Nothing
   *today* diverges; nothing *stops* a future change to the feed wire format from landing
   in one site and not the others. `parse_feed_items` is `pub(super)`, so the two
   out-of-module callers cannot reach it even if they wanted to.

2. **`verify-feed` silently swallows dropped signals (a genuine behavioural bug).**
   `deserialize_lenient_signals` reports a dropped signal via `tracing::warn!`.
   `cmd_verify_feed` is one of the few subcommands that installs **no**
   `tracing_subscriber` (contrast `src/main.rs:244`, `:313`, `:435`, `:735`), so that
   warning is written to a global no-op dispatcher and vanishes. A user debugging a feed
   script with a typo'd signal (`"reviwed"`) sees `✓ N valid items` and no indication the
   signal was discarded — the tool whose entire purpose is printing evidence throws the
   evidence away. This is the same failure mode the stderr-on-success fix in #3900
   addressed at this very call site.

Problem 2 is the answer to the judgement call the task poses ("what should `verify-feed`
report when an item warns rather than errors?"): it must report the drop on stderr, and
must **not** fail — `feeds.allium:182-192` specifies an unrecognised signal as
deliberately non-fatal, so exit status stays 0 and the item still counts as valid.

## Design decisions

**D1 — All three paths call `parse_feed_items`, and it must be fully `pub`.** Promote it
from `pub(super)` to **`pub`** (not `pub(crate)`) and re-export it from `src/feed/mod.rs`.
This makes the parse structurally shared rather than conventionally identical, matching
what #3900 did for exec and what `run_feed_sync_by_role` does for sync. It is a pure
refactor: no observable behaviour changes at any of the three sites.

`pub(crate)` is **not sufficient** and would fail to compile. `Cargo.toml` declares
package `dispatch-tui` (lib crate `dispatch_tui`) plus a separate
`[[bin]] name = "dispatch", path = "src/main.rs"`. `src/main.rs` is therefore its own
crate and reaches the library only through `dispatch_tui::…` (`src/main.rs:7-9`), so a
`pub(crate)` item in the lib is invisible to it. This is why the existing
`pub(crate) use exec::{exec_feed_command, resolve_base_branches}` at `src/feed/mod.rs:18`
is called from `src/runtime/epics.rs` but never from `main.rs` — `exec_feed_command`
(`src/feed/exec.rs:71`) is `pub(crate)` and structurally unreachable from the binary,
independently of the reasons `main.rs:477-484` gives for not calling it.

Consequently `parse_feed_items` needs its own `pub use parse::parse_feed_items;` line,
separate from the existing `pub(crate) use` line. Leave `exec_feed_command` and
`resolve_base_branches` at `pub(crate)` — nothing outside the crate needs them, and
widening them is not this task's business.

Presentation stays at the call sites — the anyhow error is rendered as a status-bar
string by the manual path and as a message-plus-500-char-preview by `verify-feed`. Only
the *decode* is shared. Do not push presentation into `parse_feed_items`.

**D2 — `verify-feed` installs a stderr `tracing_subscriber` before parsing.** Chosen
over the alternative of changing `parse_feed_items` to return
`(Vec<FeedItem>, Vec<Warning>)`, because:

- The signature stays identical for all three callers, so D1 remains a pure refactor.
- The warning text already exists and is already correct; only the sink is missing.
- It generalises: any future lenient-decode warning surfaces in `verify-feed` for free,
  rather than needing to be threaded through a bespoke return type.

The subscriber writes to **stderr**, not stdout — stdout carries the parsed-item table,
which a user may pipe. Exit code is unaffected.

**D3 — Do not fail `verify-feed` on a dropped signal.** Per `feeds.allium:182-192` the
drop is a deliberate forward-compatibility exception. Failing would defeat its purpose.

## Work plan (TDD — test first at every step)

### WP1 — Spec first

Per `CLAUDE.md` ("Behaviour changes start in the spec") and the spec→tests→code memory,
update `docs/specs/feeds.allium` before any code, using the `allium:tend` skill:

1. **`rule VerifyFeed` (line 319)** — add to the rule body that an item carrying an
   unrecognised `signals` value is accepted (the signal is dropped per the `FeedItem`
   `signals` contract) and the drop is **reported on stderr**, with exit status still 0
   and the item still counted in the valid-item total. Add to `@guidance` that the parse
   routes through `src/feed/parse.rs::parse_feed_items` and that `cmd_verify_feed`
   installs a stderr tracing subscriber so lenient-decode warnings are not swallowed.

2. **`FeedItem.signals` doc (lines 182-192)** — note that the lenient decode is a
   property of the `FeedItem` decode itself and therefore applies uniformly to all three
   feed entry points (auto-poll, manual "r", `verify-feed`), and that `verify-feed`
   surfaces the drop on stderr while the two runtime paths log it to `app.log`.

3. **Add a `-- FeedItemParse` block** next to the `FeedSync` dispatch block
   (around line 1111), stating that the JSON decode of a feed command's stdout lives in
   ONE shared function, `src/feed/parse.rs::parse_feed_items`, called by all three entry
   points — mirroring the wording that already covers `exec_feed_command` (line 1103)
   and `run_feed_sync_by_role` (line 1127). Name the three callers explicitly:
   `src/feed/mod.rs::FeedJob::run`, `src/runtime/epics.rs::exec_trigger_epic_feed`,
   `src/main.rs::cmd_verify_feed`. State that strict `tag`/`url_type` rejection and
   lenient `signals` dropping are properties of the `FeedItem` decode, so they cannot
   diverge per caller.

Run `allium check` on the file.

### WP2 — Tests for the `verify-feed` signal-drop surface (RED)

In `tests/cli.rs`, next to the existing `verify_feed_*` tests:

- `verify_feed_reports_dropped_unrecognised_signal` — feed a one-item array with
  `"signals":["reviewed","bogus"]`. Assert: exit status **success**, stdout contains the
  item row and `✓ 1 valid item`, and **stderr contains `dropping unrecognised feed
  signal` and `bogus`**. Fails today (stderr is empty).
- `verify_feed_recognised_signals_produce_no_warning` — same item with
  `"signals":["reviewed"]` only. Assert success and that stderr does **not** contain
  `dropping unrecognised`. Guards against a subscriber configured so noisily that every
  run nags.

### WP3 — Implement the `verify-feed` subscriber (GREEN)

In `src/main.rs::cmd_verify_feed`, before the parse, initialise a `tracing_subscriber`
writing to stderr. Use `.with_writer(std::io::stderr)`, no ANSI, and a filter defaulting
to `warn` that still honours `RUST_LOG` so a user can raise it. Ignore the init error
(`let _ =`), consistent with `src/main.rs:435` — a subscriber that is somehow already
installed must not abort the verify.

Add a comment explaining *why* the subscriber exists here specifically: this is the one
feed path with no `app.log` sink, and `feeds.allium`'s lenient-`signals` exception is
only observable through a warning.

### WP4 — Test that all three paths share one parse (RED)

The unification is structural, so the tests must pin the *shared decode*, not re-test
`serde_json`:

- In `src/feed/parse.rs::tests` — add `unrecognised_signal_dropped_not_fatal`, asserting
  `parse_feed_items` accepts an item with a bogus signal and yields only the recognised
  ones. This locks the lenient/strict split into the shared function's own test module,
  where a future change to the wire format will trip over it.
- In `src/runtime/tests.rs` — a manual-"r" test asserting a stdout payload with a
  **missing `tag`** produces a `FeedMessage::Failed` and upserts no task. This is the
  specific claim the task title makes; it currently holds by accident (both sites happen
  to call `serde_json`) and after WP5 will hold by construction. Model it directly on
  `src/runtime/tests.rs::exec_trigger_epic_feed_malformed_json` (`src/runtime/tests.rs:3318`),
  which is the same shape: create an epic, wire a `MockProcessRunner`, call
  `rt.exec_trigger_epic_feed(epic.id, title, "echo …".to_string(), false)`, await the mpsc
  channel with `TEST_TIMEOUT`, assert `Message::Feed(FeedMessage::Failed { .. })`. Swap the
  echoed payload for a JSON array whose single item omits `tag`.
- `tests/cli.rs` already covers the CLI path's strict-tag behaviour
  (`verify_feed_missing_tag_fails`, `verify_feed_invalid_tag_fails`, `tests/cli.rs:891-931`)
  — no new test needed there; confirm they still pass rather than duplicating them. They
  assert on `eprintln!`-written stderr text ("failed to parse"), which WP3's subscriber
  does not touch, so there is no conflict between the two stderr writers.

### WP5 — Route the two out-of-module callers through `parse_feed_items` (GREEN)

1. `src/feed/parse.rs` — change `pub(super) fn parse_feed_items` to **`pub fn`** (see D1
   for why `pub(crate)` will not compile).
2. `src/feed/mod.rs` — add a `pub use parse::parse_feed_items;` line. Do **not** add it to
   the existing `pub(crate) use` list at line 18. Internal callers keep using
   `parse::parse_feed_items`.
3. `src/runtime/epics.rs:274` — replace the `serde_json::from_slice` match with
   `crate::feed::parse_feed_items(&output.stdout)`, rendering the error as
   `format!("{e:#}")` (anyhow alternate form, matching how the rest of this file renders
   anyhow errors) instead of `e.to_string()`.
4. `src/main.rs:493` — replace `serde_json::from_str::<Vec<models::FeedItem>>(stdout.trim())`
   with `dispatch_tui::feed::parse_feed_items(output.stdout.as_slice())`. Keep the
   `stdout` `String` binding — the 500-char error preview still needs it. Update the error
   arm's message formatting to `{e:#}`.
5. Add a short comment at each of the three call sites naming the other two, in the style
   of the existing `// The SAME exec the auto-poll FeedRunner uses…` comment at
   `src/runtime/epics.rs:263`.

The `models` import in `src/main.rs` stays — it is used at a dozen-plus other sites
(`models::TaskStatus`, `models::TaskId`, `models::SubStatus`, `models::HookEventKind`, …),
so dropping the `Vec<models::FeedItem>` turbofish cannot orphan it. No import cleanup
needed.

### WP6 — Verify

```
cargo fmt --check && cargo test && ./scripts/check-doc-paths.sh
```

Also run `cargo clippy --all-targets -- -D warnings` (the pre-push gate; a plain build
will not catch an unused import or a bare `unwrap`), and `allium:weed` on
`docs/specs/feeds.allium` to confirm spec/code alignment. Manually sanity-check the new
stderr surface:

```
cargo run -- verify-feed 'echo "[{\"external_id\":\"1\",\"title\":\"T\",\"description\":\"\",\"status\":\"backlog\",\"tag\":\"bug\",\"signals\":[\"bogus\"]}]"'
```

Expect the table plus `✓ 1 valid item` on stdout and the drop warning on stderr, exit 0.

## Out of scope

- Changing the strict/lenient split itself. `tag` stays fatal, `signals` stays lenient;
  `feeds.allium:182-192` argues that split deliberately and this task is not the place to
  revisit it.
- Routing `cmd_verify_feed` through `exec_feed_command`. `src/main.rs:477-484` documents
  why it deliberately does not (no epic id/title, and it needs terminal output rather
  than `app.log`); WP3's stderr subscriber is scoped to the *warning sink*, not a merge of
  the exec paths.
- Per-item error recovery (accepting the valid items from a partly-malformed array). That
  is a real design question but a behaviour change to the wire contract, not a
  unification; it deserves its own spec discussion.

## Follow-up

Record a learning correcting the premise this task was filed under: the strict-`tag` and
lenient-`signals` rules live in `FeedItem`'s `Deserialize` impl
(`src/models/tasks.rs::deserialize_lenient_signals`), not in `parse_feed_items`, so they
were never per-path divergences. A future agent reading #3990's title would otherwise go
looking for validation logic in `src/feed/parse.rs` that has never been there.
