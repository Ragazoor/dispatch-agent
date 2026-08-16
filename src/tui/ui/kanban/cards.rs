//! Task and epic card rendering.

use chrono::{DateTime, Utc};
use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};

use crate::models::{format_age, Epic, EpicSubstatus, Staleness, SubStatus, Task, TaskStatus};
use crate::tui::{App, EpicStatsMap};

use super::super::palette::{CYAN, FG, FLASH_BG, GREEN, MUTED, PURPLE, RED, YELLOW};
use super::super::shared::{staleness_color, truncate};
use super::{
    card_border_color, card_surface_color, column_color, cursor_border_color,
    selected_card_surface_color, status_icon,
};

/// Format the title text for a task card (line 1 only — status annotations are on line 2).
fn format_task_title(task: &Task, max_title: usize) -> String {
    truncate(&task.title, max_title)
}

// ---------------------------------------------------------------------------
// CardIndicator — what to show on line 2 of a task card
// ---------------------------------------------------------------------------

/// Classifies a task's current state into a single display indicator.
/// Priority order matters: dispatching > unprovisioned > conflict >
/// detached-review > crashed > stale > blocked > detached-running > running >
/// review-pr > done-merged > idle. The `Dispatching` variant covers a task from
/// before its claim until the dispatch worker reports success or failure, so it
/// is reachable for a Backlog task (not yet claimed) *and* for a Running one
/// with no worktree (claimed, being provisioned). Its top priority is what keeps
/// that second state from rendering as a live agent — see `SpansTheClaim` on the
/// `DispatchingFeedback` surface in `docs/specs/dispatch.allium`.
///
/// `Unprovisioned` sits directly below it because every indicator beneath
/// describes a state that presupposes a worktree. It is also gated on
/// [`App::dispatch_may_be_in_flight`], because `Dispatching` membership only
/// spans the claims this TUI made: the epic chain claims inside the MCP close
/// path and a board restart empties the set, so the freshness gate covers the
/// dispatches `SpansTheClaim` cannot. See `UnprovisionedIndicator` in
/// `docs/specs/dispatch.allium`.
#[derive(Debug)]
#[cfg_attr(test, derive(PartialEq))]
enum CardIndicator {
    Dispatching {
        spinner_frame: u8,
    },
    Unprovisioned,
    AutoDispatchFailed,
    Conflict,
    DetachedReview {
        pr_label: String,
    },
    Detached,
    Crashed,
    Stale {
        /// `None` when the task has no recorded PreToolUse timestamp (e.g.
        /// pre-v50 rows, or transient state right after a manual transition
        /// into Running before the SeedActivity write commits). The card
        /// omits "· Xm" in that case rather than rendering a misleading "0m".
        inactive_mins: Option<u64>,
    },
    /// A live background shell has been running long enough to be flagged as
    /// possibly-abandoned (past `SHELL_STALE_THRESHOLD`), rendered distinctly
    /// from plain `Stale` so it's clear the task has a shell that's been
    /// running unusually long, not that the agent has gone idle.
    StaleShell {
        /// `None` when `oldest_live_shell_started_at` isn't recorded (mirrors
        /// `Stale`'s `inactive_mins` handling).
        inactive_hours: Option<u64>,
    },
    Blocked,
    Running {
        subagents: u32,
        shells: u32,
    },
    ReviewPr {
        pr_label: String,
    },
    DoneMerged {
        pr_label: String,
    },
    Idle {
        status: TaskStatus,
        age: String,
        staleness: Staleness,
        plan_indicator: &'static str,
        tag_suffix: &'static str,
    },
}

fn classify_card_indicator(
    task: &Task,
    status: TaskStatus,
    app: &App,
    now: DateTime<Utc>,
) -> CardIndicator {
    if app.is_dispatching(task.id) {
        // No assertion here on purpose. Membership implies "not yet provisioned",
        // but only up to message-queue latency (`SpansTheClaim` in
        // docs/specs/dispatch.allium), so a board refresh can briefly pair a
        // worktree-bearing row with a live membership — and a render function is
        // the wrong place to panic over a state the spec calls racy. The
        // invariant is asserted where it is definitionally true instead, in
        // `mark_dispatching`.
        return CardIndicator::Dispatching {
            spinner_frame: app.spinner_tick,
        };
    }
    // A claim that is still being provisioned looks identical to one whose
    // worker died. Only the second is worth alarming about, so keep rendering
    // the ordinary running card until the claim ages past the dispatch
    // watchdog window.
    if task.is_unprovisioned() && !app.dispatch_may_be_in_flight(task, now) {
        return CardIndicator::Unprovisioned;
    }
    // A subtask an epic chain claimed and then failed to provision. It sits in
    // backlog looking exactly like one that was never dispatched, so without
    // this the stalled epic is invisible (AutoDispatchFailureIndicator in
    // docs/specs/epics.allium). Below the dispatching check on purpose: a retry
    // in flight outranks the failure it is resolving.
    if app.auto_dispatch_failed(task.id) {
        return CardIndicator::AutoDispatchFailed;
    }
    if task.sub_status == SubStatus::Conflict {
        return CardIndicator::Conflict;
    }
    if task.is_detached() {
        if let (TaskStatus::Review, Some(u)) = (status, task.url.as_ref()) {
            let pr_label = u.label();
            return CardIndicator::DetachedReview { pr_label };
        }
        return CardIndicator::Detached;
    }
    if task.sub_status == SubStatus::Crashed {
        return CardIndicator::Crashed;
    }
    if task.sub_status == SubStatus::Stale {
        // Source of truth matches ClassifyAgentActivity so the label survives
        // TUI restart. None handling lives on the `inactive_mins` field doc.
        let inactive_mins = task.last_pre_tool_use_at.map(|ts| {
            now.signed_duration_since(ts)
                .num_minutes()
                .max(0)
                .unsigned_abs()
        });
        return CardIndicator::Stale { inactive_mins };
    }
    if task.sub_status == SubStatus::StaleShell {
        let inactive_hours = task.oldest_live_shell_started_at.map(|ts| {
            now.signed_duration_since(ts)
                .num_hours()
                .max(0)
                .unsigned_abs()
        });
        return CardIndicator::StaleShell { inactive_hours };
    }
    if status == TaskStatus::Running && task.sub_status == SubStatus::NeedsInput {
        return CardIndicator::Blocked;
    }
    if status == TaskStatus::Running {
        return CardIndicator::Running {
            subagents: task.live_subagents.max(0) as u32,
            shells: task.live_shells.max(0) as u32,
        };
    }
    if let (TaskStatus::Review, Some(u)) = (status, task.url.as_ref()) {
        let pr_label = u.label();
        return CardIndicator::ReviewPr { pr_label };
    }
    if let (TaskStatus::Done, Some(u)) = (status, task.url.as_ref()) {
        let pr_label = u.label();
        return CardIndicator::DoneMerged { pr_label };
    }

    let age = format_age(task.updated_at, now);
    let staleness = Staleness::from_age(task.updated_at, now);
    let plan_indicator = if task.plan_path.is_some() && status == TaskStatus::Backlog {
        "▸ "
    } else {
        ""
    };
    let tag_suffix = match task.tag {
        Some(crate::models::TaskTag::Bug) => " [bug]",
        Some(crate::models::TaskTag::Feature) => " [feat]",
        Some(crate::models::TaskTag::Chore) => " [chore]",
        Some(crate::models::TaskTag::PrReview) => " [pr-rev]",
        Some(crate::models::TaskTag::Research) => " [research]",
        Some(crate::models::TaskTag::Fix) => " [fix]",
        Some(crate::models::TaskTag::Dependabot) => " [dep]",
        None => "",
    };
    CardIndicator::Idle {
        status,
        age,
        staleness,
        plan_indicator,
        tag_suffix,
    }
}

/// The border colour a card's state claims, or `None` if it claims none.
///
/// The card frame carries *state*, not identity (`core.allium`: "Selection" and
/// "Border as state"). Red is the four hard failures — the same states whose
/// indicator renders a `⚠`. Note that is an agreement between two independent
/// exhaustive matches over one enum, not a derivation: `render_card_indicator`
/// picks the glyph, this picks the border, and only
/// `every_indicator_claims_the_border_its_severity_earns` keeps them in step. A
/// `CardIndicator::severity()` consumed by both would make it true by
/// construction. Amber is the three that want a human's
/// attention without being broken.
///
/// `Dispatching` is deliberately absent despite rendering an amber indicator: it
/// is a transient expected state, and bordering it would make every ordinary
/// dispatch alarm for its duration.
///
/// Matched exhaustively on purpose — a new `CardIndicator` variant must decide
/// whether it is alarming rather than defaulting to silence.
fn state_border_color(indicator: &CardIndicator) -> Option<Color> {
    match indicator {
        CardIndicator::Unprovisioned
        | CardIndicator::AutoDispatchFailed
        | CardIndicator::Conflict
        | CardIndicator::Crashed => Some(RED),
        CardIndicator::Blocked | CardIndicator::Stale { .. } | CardIndicator::StaleShell { .. } => {
            Some(YELLOW)
        }
        CardIndicator::Dispatching { .. }
        | CardIndicator::DetachedReview { .. }
        | CardIndicator::Detached
        | CardIndicator::Running { .. }
        | CardIndicator::ReviewPr { .. }
        | CardIndicator::DoneMerged { .. }
        | CardIndicator::Idle { .. } => None,
    }
}

/// The card frame's colour: cursor, then state, then the resting neutral.
///
/// Shared by task and epic cards so the precedence is stated once. Epic cards
/// pass `None` — they carry no `CardIndicator` and so claim no state colour — but
/// the cursor rule is identical for both, and the point of the cursor white is
/// that it is the same everywhere.
fn resolve_frame_color(is_cursor: bool, state: Option<Color>) -> Color {
    if is_cursor {
        cursor_border_color()
    } else {
        state.unwrap_or_else(card_border_color)
    }
}

/// Braille spinner glyphs (10 frames). Indexed by `App::spinner_tick`,
/// advanced once per Tick while a dispatch is in flight.
const DISPATCHING_SPINNER: [&str; 10] = [
    "\u{280B}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283C}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280F}",
];

/// `"N {singular}"` / `"N {plural}"`, or `None` at zero (the card omits the
/// suffix entirely rather than rendering e.g. "running · 0 agents"). Shared
/// by the running card's subagent and shell counts.
fn count_suffix(n: u32, singular: &str, plural: &str) -> Option<String> {
    match n {
        0 => None,
        1 => Some(format!("1 {singular}")),
        n => Some(format!("{n} {plural}")),
    }
}

/// Line 2 of a task card: the state indicator, then the chips.
///
/// `schedule_interval_secs` is the task's cadence, or `None` when it is not
/// scheduled. It renders as a leading chip rather than as a `CardIndicator`
/// variant precisely because it is *not* a lifecycle state — see "Scheduled
/// badge" in `docs/specs/core.allium` for why folding it into the indicator
/// would be backwards.
fn render_card_indicator(
    indicator: CardIndicator,
    labels: &[String],
    schedule_interval_secs: Option<i64>,
) -> Line<'static> {
    let (label, color) = match indicator {
        CardIndicator::Dispatching { spinner_frame } => {
            let glyph = DISPATCHING_SPINNER
                [(spinner_frame as usize) % crate::tui::DISPATCH_SPINNER_FRAMES as usize];
            (format!("{glyph} dispatching\u{2026}"), YELLOW)
        }
        CardIndicator::Unprovisioned => ("\u{26a0} no worktree".to_string(), RED),
        CardIndicator::AutoDispatchFailed => ("\u{26a0} auto-dispatch failed".to_string(), RED),
        CardIndicator::Conflict => ("\u{26a0} rebase conflict".to_string(), RED),
        CardIndicator::DetachedReview { pr_label } => (format!("\u{25cb} {pr_label}"), CYAN),
        CardIndicator::Detached => ("\u{25cb} detached".to_string(), MUTED),
        CardIndicator::Crashed => ("\u{26a0} crashed".to_string(), RED),
        CardIndicator::Stale { inactive_mins } => {
            let label = match inactive_mins {
                Some(m) => format!("\u{25c9} stale \u{00b7} {m}m"),
                None => "\u{25c9} stale".to_string(),
            };
            (label, YELLOW)
        }
        CardIndicator::StaleShell { inactive_hours } => {
            let label = match inactive_hours {
                Some(h) => format!("\u{25c9} shell stale \u{00b7} {h}h"),
                None => "\u{25c9} shell stale".to_string(),
            };
            (label, YELLOW)
        }
        CardIndicator::Blocked => ("\u{25c9} blocked".to_string(), YELLOW),
        CardIndicator::Running { subagents, shells } => {
            let icon = status_icon(TaskStatus::Running);
            let mut label = format!("{icon} running");
            if let Some(suffix) = count_suffix(subagents, "agent", "agents") {
                label.push_str(" \u{00b7} ");
                label.push_str(&suffix);
            }
            if let Some(suffix) = count_suffix(shells, "shell", "shells") {
                label.push_str(" \u{00b7} ");
                label.push_str(&suffix);
            }
            (label, CYAN)
        }
        CardIndicator::ReviewPr { pr_label } => (format!("\u{25cf} {pr_label}"), CYAN),
        CardIndicator::DoneMerged { pr_label } => (format!("\u{2714} {pr_label} merged"), GREEN),
        CardIndicator::Idle {
            status,
            age,
            staleness,
            plan_indicator,
            tag_suffix,
        } => {
            let icon = status_icon(status);
            (
                format!("{plan_indicator}{icon} {age}{tag_suffix}"),
                staleness_color(staleness),
            )
        }
    };
    let mut spans = vec![
        Span::raw("   "),
        Span::styled(label, Style::default().fg(color)),
    ];
    // Ahead of the task's own labels, so a feed script's first label keeps its
    // position regardless of a property the script knows nothing about.
    if let Some(secs) = schedule_interval_secs {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("[\u{23f1} {}]", crate::models::format_interval_secs(secs)),
            Style::default().fg(MUTED),
        ));
    }
    for label in labels {
        spans.push(Span::raw(" "));
        spans.push(Span::styled(
            format!("[{label}]"),
            Style::default().fg(MUTED),
        ));
    }
    Line::from(spans)
}

/// Render a decorative epic-header separator row (non-selectable).
/// Shows the epic's ancestor breadcrumb: `── root › … › self ──────────`.
pub(super) fn render_epic_header_item(
    epic: &Epic,
    epics: &[Epic],
    col_width: u16,
) -> ListItem<'static> {
    // Text budget matches the prior single-title layout: "── " + text + " " + rule
    // reserves `text_len + 5`, so the text may use up to `col_width - 5` chars.
    let budget = (col_width as usize).saturating_sub(5);
    let segments = crate::models::epics::ancestor_titles(epic, epics);
    let title = crate::tui::ui::shared::fair_truncate_segments(
        &segments,
        budget,
        crate::tui::ui::shared::BREADCRUMB_SEPARATOR,
    );
    let rule_count = (col_width as usize).saturating_sub(title.chars().count() + 5);
    let right_rule = "\u{2500}".repeat(rule_count);
    ListItem::new(Line::from(vec![
        Span::styled("\u{2500}\u{2500} ", Style::default().fg(MUTED)),
        Span::styled(title, Style::default().fg(PURPLE)),
        Span::styled(format!(" {}", right_rule), Style::default().fg(MUTED)),
    ]))
}

/// Per-column rendering context shared by every card in a column.
///
/// Bundles the column-level parameters threaded through the card/epic
/// renderers: the stripe colour, the column width, and the column's neutral
/// ground.
///
/// All three are constant across a column. `ground` is needed because a card is
/// inset by [`CARD_MARGIN`] on each side, so the renderer has to paint the
/// margin cells in the column's own ground rather than let them inherit the card
/// surface (`core.allium`: "Task card frame").
pub(super) struct ColRenderCtx {
    pub color: Color,
    pub width: u16,
    pub ground: Color,
}

/// Columns of column-ground left visible on each side of a card, so cards float
/// inside the column instead of tiling flush against its edges.
const CARD_MARGIN: usize = 1;

/// Total horizontal cells a card spends on chrome: two ground margins plus two
/// frame rails. Subtracted from the column width to get the content budget.
pub(super) const CARD_CHROME_WIDTH: usize = CARD_MARGIN * 2 + 2;

/// Wrap a card's two content lines in a complete rounded frame.
///
/// Returns the four lines of a framed card — top border, the two content lines
/// each flanked by rails, and the bottom border (`core.allium`: "Task card
/// frame"). Every card carries its own full frame; borders are never shared
/// between neighbours.
///
/// The whole card is lit, frame included: the caller sets the card surface as
/// the `ListItem`'s base background, so the border rows and rails inherit it and
/// a card's boundary is the change of colour at its outer edge. Only the margin
/// spans override that, back to the column ground.
///
/// `frame_color` is the caller's already-resolved state: the column's identity
/// colour for the selected card, the resting neutral otherwise.
fn frame_card(
    line1: Line<'static>,
    line2: Line<'static>,
    col_width: u16,
    frame_color: Color,
    ground: Color,
) -> Vec<Line<'static>> {
    let card_width = (col_width as usize).saturating_sub(CARD_MARGIN * 2);
    let content_width = card_width.saturating_sub(2);
    let style = Style::default().fg(frame_color);
    // A `&'static str`, not `" ".repeat(CARD_MARGIN)`: the repeat heap-allocates a
    // fresh String every call, and this closure runs eight times per card — twice
    // per rail and twice per border — on every frame.
    const MARGIN_STR: &str = " ";
    debug_assert_eq!(
        MARGIN_STR.len(),
        CARD_MARGIN,
        "margin literal must match CARD_MARGIN"
    );
    let margin = || Span::styled(MARGIN_STR, Style::default().bg(ground));
    let horizontal = "\u{2500}".repeat(content_width);

    let rail = |inner: Line<'static>| -> Line<'static> {
        let pad = content_width.saturating_sub(inner.width());
        let mut spans = vec![margin(), Span::styled("\u{2502}", style)];
        spans.extend(inner.spans);
        if pad > 0 {
            spans.push(Span::raw(" ".repeat(pad)));
        }
        spans.push(Span::styled("\u{2502}", style));
        spans.push(margin());
        Line::from(spans)
    };

    let border = |left: &str, right: &str| -> Line<'static> {
        Line::from(vec![
            margin(),
            Span::styled(format!("{left}{horizontal}{right}"), style),
            margin(),
        ])
    };

    vec![
        border("\u{256d}", "\u{256e}"),
        rail(line1),
        rail(line2),
        border("\u{2570}", "\u{256f}"),
    ]
}

/// Build a styled framed ListItem for a task card in a kanban column.
/// Line 1: stripe + title
/// Line 2: status icon + age/activity metadata
pub(super) fn build_task_list_item<'a>(
    task: &Task,
    status: TaskStatus,
    app: &App,
    now: DateTime<Utc>,
    is_cursor: bool,
    ctx: &ColRenderCtx,
) -> ListItem<'a> {
    let col_color = ctx.color;
    let col_width = ctx.width;

    let is_batch_selected = app.selected_tasks().contains(&task.id);
    let select_prefix = if is_batch_selected { "* " } else { "  " };

    // Same threshold the expiry sweep uses (`App::tick_message_flash`), read from
    // the one constant so the two cannot drift apart.
    let has_message_flash = app
        .agents
        .message_flash
        .get(&task.id)
        .is_some_and(|t| t.elapsed() < crate::tui::MESSAGE_FLASH_TTL);
    // Sent-flash sibling (task #4098): a distinct glyph, same fill/TTL. Both
    // can be true at once — a task that sent and received within the same
    // window shows both glyphs, per core.allium's "Message flash".
    let has_message_flash_sent = app
        .agents
        .message_flash_sent
        .get(&task.id)
        .is_some_and(|t| t.elapsed() < crate::tui::MESSAGE_FLASH_TTL);
    let any_message_flash = has_message_flash || has_message_flash_sent;

    // Prefix: select(2) + stripe(1) + " #NNN "(id_len+3) + optional flash glyphs
    // (" ✉"/" ➤", 2 each)
    let id_len = task.id.0.unsigned_abs().max(1).ilog10() as usize + 1;
    let flash_width =
        if has_message_flash { 2 } else { 0 } + if has_message_flash_sent { 2 } else { 0 };
    let prefix_width = 2 + 1 + 3 + id_len + flash_width + CARD_CHROME_WIDTH;
    let max_title = (col_width as usize).saturating_sub(prefix_width);
    let title_text = format_task_title(task, max_title);

    // Line 1: prefix + stripe + title.
    // One quarter block on every card, cursor included (core.allium: "Card
    // stripe"). Stripe weight no longer moves with the cursor — selection is
    // carried by the frame hue and the bold title.
    let stripe_char = "\u{258e}";
    let stripe_style = Style::default().fg(col_color);
    // Bold marks the selected card's title (core.allium: "Selection"). Its fill
    // is unchanged from a resting card's, by design.
    let title_style = if is_batch_selected || is_cursor {
        Style::default().add_modifier(Modifier::BOLD)
    } else {
        Style::default()
    };

    let mut line1_spans = vec![
        Span::styled(select_prefix.to_string(), title_style),
        Span::styled(stripe_char, stripe_style),
        Span::styled(format!(" #{} ", task.id), Style::default().fg(MUTED)),
        Span::styled(title_text.to_string(), title_style),
    ];
    if has_message_flash {
        line1_spans.push(Span::styled(" \u{2709}", Style::default().fg(YELLOW)));
    }
    if has_message_flash_sent {
        line1_spans.push(Span::styled(" \u{27a4}", Style::default().fg(YELLOW)));
    }

    let line1 = Line::from(line1_spans);

    let indicator = classify_card_indicator(task, status, app, now);
    // Read the severity off the indicator before it is consumed, so the border and
    // the glyph beneath it come from one classification rather than two.
    let state_border = state_border_color(&indicator);
    let line2 = render_card_indicator(indicator, &task.labels, task.schedule_interval_secs);

    // Precedence: cursor, then state, then neutral (`core.allium`: "Selection").
    //
    // The frame carries *state*, not identity — a hued border means something is
    // wrong, not that this is the column's colour. The cursor takes a white of its
    // own precisely so it is not competing for the alarm hues.
    //
    // The cursor winning means an unhealthy card that is *also* the cursor shows
    // white rather than its state colour. That is the accepted cost: its indicator
    // line still says so directly beneath, and the alternative hides the cursor on
    // the card you just navigated to, which is worse.
    //
    // The flash contributes nothing here. It is carried by its fill and its
    // glyph(s) — envelope for received, outgoing arrow for sent — which are
    // the only things no other card has.
    let frame_color = resolve_frame_color(is_cursor, state_border);
    // A flash replaces the card surface for its duration; that differing fill is
    // what keeps it distinguishable from the selection despite sharing the hue.
    // Sent and received share one fill — the glyph, not the fill, carries the
    // direction.
    //
    // The selected card asks for its own surface even though
    // `SelectionDoesNotLiftTheFill` pins it equal to a resting card's. Routing
    // the call this way keeps the equality load-bearing: if the two ever diverge,
    // the render follows and `selection_does_not_lift_the_fill` fails.
    let surface = if any_message_flash {
        FLASH_BG
    } else if is_cursor {
        selected_card_surface_color()
    } else {
        card_surface_color()
    };
    let mut item = ListItem::new(frame_card(line1, line2, col_width, frame_color, ctx.ground));

    // Base style for the whole card, so the frame rows inherit the surface and
    // the card reads as one lit object. Margin spans override back to ground.
    let mut style = Style::default().bg(surface).fg(FG);
    if any_message_flash {
        style = style.add_modifier(Modifier::BOLD);
    }
    item = item.style(style);

    item
}

fn epic_substatus_color(substatus: &EpicSubstatus) -> Color {
    match substatus {
        EpicSubstatus::Blocked(_) => YELLOW,
        EpicSubstatus::InReview => CYAN,
        EpicSubstatus::Active | EpicSubstatus::Unplanned | EpicSubstatus::Planned => MUTED,
        EpicSubstatus::Done => MUTED,
    }
}

pub(super) fn render_epic_item(
    epic: &Epic,
    is_cursor: bool,
    app: &App,
    epic_stats: &EpicStatsMap,
    status: TaskStatus,
    ctx: &ColRenderCtx,
) -> ListItem<'static> {
    let col_width = ctx.width;
    let stats = epic_stats.get(&epic.id);

    let plan_indicator = if epic.plan_path.is_some() && status == TaskStatus::Backlog {
        " \u{25b8}" // ▸
    } else {
        ""
    };

    // Prefix: select(2) + stripe(1) + " #NNN "(id_len+3) + plan_indicator
    let id_len = epic.id.0.unsigned_abs().max(1).ilog10() as usize + 1;
    let prefix_width = 2 + 1 + 3 + id_len + plan_indicator.chars().count() + CARD_CHROME_WIDTH;
    let max_title = (col_width as usize).saturating_sub(prefix_width);
    let title_text = truncate(&epic.title, max_title);

    let is_batch_selected = app.selected_epics().contains(&epic.id);
    let select_prefix = if is_batch_selected { "* " } else { "  " };

    // Line 1: stripe + title. One quarter block on every card, cursor included
    // (core.allium: "Card stripe").
    let stripe_char = "\u{258e}";
    let title_style = Style::default().fg(PURPLE).add_modifier(Modifier::BOLD);
    let line1 = Line::from(vec![
        Span::raw(select_prefix.to_string()),
        Span::styled(stripe_char, Style::default().fg(PURPLE)),
        Span::styled(format!(" #{} ", epic.id), Style::default().fg(MUTED)),
        Span::styled(format!("{title_text}{plan_indicator}"), title_style),
    ]);

    // Line 2: colored status indicators + substatus label
    let line2 = if let Some(s) = stats.filter(|s| s.total > 0) {
        let mut spans = vec![Span::raw("    ".to_string())];
        let indicators: &[(usize, Color)] = &[
            (s.backlog, column_color(TaskStatus::Backlog)),
            (s.running, column_color(TaskStatus::Running)),
            (s.review, column_color(TaskStatus::Review)),
            (s.done, column_color(TaskStatus::Done)),
        ];
        for (count, color) in indicators {
            if *count > 0 {
                spans.push(Span::styled(
                    format!("\u{25cf}{count} "),
                    Style::default().fg(*color),
                ));
            }
        }
        spans.push(Span::styled(
            s.substatus.label(),
            Style::default().fg(epic_substatus_color(&s.substatus)),
        ));
        Line::from(spans)
    } else {
        Line::from(vec![
            Span::raw("    "),
            Span::styled("no subtasks", Style::default().fg(MUTED)),
        ])
    };

    // The cursor white applies to epic cards too, so "white frame" means cursor
    // everywhere rather than on task cards only. An epic's own PURPLE identity
    // stays on its stripe and title, which it keeps at rest.
    //
    // This matters more on an epic than on a task: an epic's title is bold
    // unconditionally, so the frame is the *only* cursor signal an epic card has.
    // Epics carry no CardIndicator and so claim no state colour; a resting epic
    // frame is neutral like any other.
    // Epics carry no CardIndicator, so they claim no state colour — but the
    // precedence itself is shared, so the rule lives in one place.
    let frame_color = resolve_frame_color(is_cursor, None);
    ListItem::new(frame_card(line1, line2, col_width, frame_color, ctx.ground))
        .style(Style::default().bg(card_surface_color()).fg(FG))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::models::SubStatus;
    use crate::tui::tests::{make_task, make_unprovisioned_task};

    /// Every `CardIndicator` variant, paired with the border colour it claims.
    ///
    /// Written out rather than generated so that adding a variant forces a
    /// deliberate entry here — the same reason `state_border_color` matches
    /// exhaustively. If this list and that match ever disagree, one of them is
    /// the mistake and the test says which.
    fn every_indicator() -> Vec<(CardIndicator, Option<Color>, &'static str)> {
        vec![
            // The four hard failures. This is the *membership* claim: these four
            // and no others are red. Only `Crashed` is exercised through the
            // renderer, so without this a state quietly promoted into or dropped
            // out of the set would pass every other test.
            (CardIndicator::Unprovisioned, Some(RED), "unprovisioned"),
            (
                CardIndicator::AutoDispatchFailed,
                Some(RED),
                "auto-dispatch failed",
            ),
            (CardIndicator::Conflict, Some(RED), "rebase conflict"),
            (CardIndicator::Crashed, Some(RED), "crashed"),
            // The two attention states.
            (CardIndicator::Blocked, Some(YELLOW), "blocked"),
            (
                CardIndicator::Stale {
                    inactive_mins: Some(7),
                },
                Some(YELLOW),
                "stale",
            ),
            (
                CardIndicator::StaleShell {
                    inactive_hours: Some(5),
                },
                Some(YELLOW),
                "stale shell",
            ),
            // Everything else claims nothing. `Dispatching` is the load-bearing
            // entry: it renders an amber *indicator* while claiming no border,
            // which is a judgement rather than a category and so the likeliest
            // exclusion to be "fixed" by someone who reads the glyph and assumes
            // the missing border was an oversight. It is also an absence, which is
            // what a renderer drifts into asserting by accident.
            (
                CardIndicator::Dispatching { spinner_frame: 0 },
                None,
                "dispatching",
            ),
            (
                CardIndicator::DetachedReview {
                    pr_label: "PR #1".to_string(),
                },
                None,
                "detached review",
            ),
            (CardIndicator::Detached, None, "detached"),
            (
                CardIndicator::Running {
                    subagents: 0,
                    shells: 0,
                },
                None,
                "running",
            ),
            (
                CardIndicator::ReviewPr {
                    pr_label: "PR #1".to_string(),
                },
                None,
                "review PR",
            ),
            (
                CardIndicator::DoneMerged {
                    pr_label: "PR #1".to_string(),
                },
                None,
                "done merged",
            ),
            (
                CardIndicator::Idle {
                    status: TaskStatus::Backlog,
                    age: "1h".to_string(),
                    staleness: Staleness::Fresh,
                    plan_indicator: "",
                    tag_suffix: "",
                },
                None,
                "idle",
            ),
        ]
    }

    #[test]
    fn every_indicator_claims_the_border_its_severity_earns() {
        for (indicator, expected, name) in every_indicator() {
            assert_eq!(
                state_border_color(&indicator),
                expected,
                "{name} claims the wrong border colour"
            );
        }
    }

    #[test]
    fn exactly_four_indicators_are_hard_failures() {
        // The counts are the membership claim stated a second way: the per-variant
        // test above would still pass if a variant were *added* to the red set and
        // its expectation updated in the same edit. These numbers make that edit
        // visible.
        let all = every_indicator();
        let red = all.iter().filter(|(_, c, _)| *c == Some(RED)).count();
        let amber = all.iter().filter(|(_, c, _)| *c == Some(YELLOW)).count();
        let none = all.iter().filter(|(_, c, _)| c.is_none()).count();
        assert_eq!(
            red, 4,
            "the hard-failure set must have exactly four members"
        );
        assert_eq!(
            amber, 3,
            "the attention set must have exactly three members"
        );
        assert_eq!(
            red + amber + none,
            all.len(),
            "a border colour outside {{red, amber, none}} has appeared"
        );
    }

    fn stale_task(last_pre_tool_use_at: Option<DateTime<Utc>>) -> crate::models::Task {
        let mut t = make_task(1, TaskStatus::Running);
        t.sub_status = SubStatus::Stale;
        t.worktree = Some("/repo/.worktrees/1-t".to_string());
        t.tmux_window = Some("task-1".to_string());
        t.last_pre_tool_use_at = last_pre_tool_use_at;
        t
    }

    fn line_text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    #[test]
    fn stale_card_with_known_timestamp_shows_minutes() {
        let now = Utc::now();
        let task = stale_task(Some(now - chrono::Duration::minutes(7)));
        let app = App::new(vec![]);
        let indicator = classify_card_indicator(&task, task.status, &app, now);
        assert_eq!(
            indicator,
            CardIndicator::Stale {
                inactive_mins: Some(7)
            },
        );
        let text = line_text(&render_card_indicator(indicator, &[], None));
        assert!(text.contains("stale · 7m"), "got {text:?}");
    }

    // --- scheduled badge --------------------------------------------------

    fn idle_indicator() -> CardIndicator {
        CardIndicator::Idle {
            status: TaskStatus::Backlog,
            age: "1h".to_string(),
            staleness: Staleness::Fresh,
            plan_indicator: "",
            tag_suffix: "",
        }
    }

    /// The badge is the only thing on the card distinguishing a scheduled task
    /// from one nobody has touched — a scheduled task sits idle in backlog
    /// between redispatches (core.allium: "Scheduled badge").
    #[test]
    fn a_scheduled_task_shows_its_cadence_as_a_badge() {
        let text = line_text(&render_card_indicator(idle_indicator(), &[], Some(600)));
        assert!(text.contains("\u{23f1} 10m"), "got {text:?}");
    }

    #[test]
    fn an_unscheduled_task_shows_no_badge_at_all() {
        let text = line_text(&render_card_indicator(idle_indicator(), &[], None));
        assert!(!text.contains('\u{23f1}'), "got {text:?}");
    }

    /// The badge is a standing property, not a lifecycle state: it must
    /// coexist with whatever the card is actually doing rather than displace
    /// it. A scheduled task whose last run died must still read as crashed.
    #[test]
    fn the_badge_coexists_with_an_alarming_indicator_rather_than_hiding_it() {
        let text = line_text(&render_card_indicator(
            CardIndicator::Crashed,
            &[],
            Some(7200),
        ));
        assert!(text.contains("crashed"), "got {text:?}");
        assert!(text.contains("\u{23f1} 2h"), "got {text:?}");
    }

    /// Chip order is stated in core.allium: the badge precedes the task's own
    /// labels, so a feed script's first label does not get pushed around by a
    /// property the script knows nothing about.
    #[test]
    fn the_badge_renders_before_the_tasks_own_labels() {
        let labels = vec!["alpha".to_string()];
        let text = line_text(&render_card_indicator(idle_indicator(), &labels, Some(600)));
        let badge_at = text.find('\u{23f1}').expect("badge missing");
        let label_at = text.find("[alpha]").expect("label missing");
        assert!(badge_at < label_at, "got {text:?}");
    }

    /// A cadence that divides no whole unit falls back to bare seconds rather
    /// than rounding — the badge's whole job is to state the cadence.
    #[test]
    fn an_inexact_cadence_renders_as_seconds() {
        let text = line_text(&render_card_indicator(idle_indicator(), &[], Some(650)));
        assert!(text.contains("\u{23f1} 650s"), "got {text:?}");
    }

    #[test]
    fn running_without_worktree_classifies_unprovisioned() {
        let now = Utc::now();
        let task = make_unprovisioned_task(1, TaskStatus::Running);
        let app = App::new(vec![]);
        let indicator = classify_card_indicator(&task, task.status, &app, now);
        assert_eq!(indicator, CardIndicator::Unprovisioned);
        let text = line_text(&render_card_indicator(indicator, &[], None));
        assert!(text.contains("no worktree"), "got {text:?}");
        assert!(!text.contains("running"), "got {text:?}");
    }

    #[test]
    fn review_without_worktree_classifies_unprovisioned() {
        let now = Utc::now();
        let mut task = make_unprovisioned_task(1, TaskStatus::Review);
        task.sub_status = SubStatus::AwaitingReview;
        task.url = Some(crate::models::TaskUrl::new(
            "https://github.com/org/repo/pull/7",
            crate::models::UrlType::Pr,
        ));
        let app = App::new(vec![]);
        assert_eq!(
            classify_card_indicator(&task, task.status, &app, now),
            CardIndicator::Unprovisioned,
        );
    }

    /// `@guarantee DispatchingOutranksIt` — the claim writes Running before the
    /// worktree exists, so every in-flight dispatch passes through the
    /// unprovisioned state. It must keep rendering "dispatching…"; otherwise
    /// every normal dispatch renders as broken while it is in flight.
    #[test]
    fn dispatching_outranks_unprovisioned() {
        let now = Utc::now();
        let task = make_unprovisioned_task(1, TaskStatus::Running);
        let mut app = App::new(vec![task.clone()]);
        app.mark_dispatching(task.id);
        let indicator = classify_card_indicator(&task, task.status, &app, now);
        assert!(
            matches!(indicator, CardIndicator::Dispatching { .. }),
            "got {indicator:?}",
        );
    }

    /// The epic auto-dispatch chain claims its next subtask inside the MCP
    /// handler and never enters `app.dispatching`, so the map alone would let
    /// every chained subtask render as broken for its whole provisioning
    /// window. A fresh claim stamp keeps it on the ordinary running card.
    #[test]
    fn freshly_claimed_task_not_in_dispatching_map_still_shows_running() {
        let now = Utc::now();
        let mut task = make_unprovisioned_task(1, TaskStatus::Running);
        task.last_pre_tool_use_at = Some(now - chrono::Duration::seconds(5));
        let app = App::new(vec![task.clone()]);
        assert!(
            !app.is_dispatching(task.id),
            "not in the map, by construction"
        );
        assert_eq!(
            classify_card_indicator(&task, task.status, &app, now),
            CardIndicator::Running {
                subagents: 0,
                shells: 0
            },
        );
    }

    /// Once the claim ages past the dispatch watchdog window, "slow" becomes
    /// "dead" and the card flips to the warning.
    #[test]
    fn stale_claim_flips_to_unprovisioned() {
        let now = Utc::now();
        let mut task = make_unprovisioned_task(1, TaskStatus::Running);
        task.last_pre_tool_use_at = Some(
            now - chrono::Duration::from_std(crate::tui::DISPATCH_WATCHDOG_TIMEOUT).unwrap()
                - chrono::Duration::seconds(1),
        );
        let app = App::new(vec![task.clone()]);
        assert_eq!(
            classify_card_indicator(&task, task.status, &app, now),
            CardIndicator::Unprovisioned,
        );
    }

    /// `@guarantee NotShownWhenProvisioned` — a worktree with no window is
    /// `detached` (resumable), the opposite of unprovisioned.
    #[test]
    fn running_with_worktree_no_window_stays_detached() {
        let now = Utc::now();
        let mut task = make_task(1, TaskStatus::Running);
        task.tmux_window = None;
        let app = App::new(vec![]);
        assert_eq!(
            classify_card_indicator(&task, task.status, &app, now),
            CardIndicator::Detached,
        );
    }

    #[test]
    fn running_with_worktree_and_window_stays_running() {
        let now = Utc::now();
        let task = make_task(1, TaskStatus::Running);
        let app = App::new(vec![]);
        assert_eq!(
            classify_card_indicator(&task, task.status, &app, now),
            CardIndicator::Running {
                subagents: 0,
                shells: 0
            },
        );
    }

    fn label_of(indicator: CardIndicator) -> String {
        render_card_indicator(indicator, &[], None)
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
    }

    #[test]
    fn running_card_shows_subagent_count() {
        let text = label_of(CardIndicator::Running {
            subagents: 3,
            shells: 0,
        });
        assert!(text.contains("running \u{00b7} 3 agents"), "got: {text:?}");
    }

    #[test]
    fn running_card_uses_the_singular_for_one_subagent() {
        let text = label_of(CardIndicator::Running {
            subagents: 1,
            shells: 0,
        });
        assert!(text.contains("running \u{00b7} 1 agent"), "got: {text:?}");
        assert!(!text.contains("1 agents"), "got: {text:?}");
    }

    #[test]
    fn running_card_omits_the_suffix_at_zero() {
        let text = label_of(CardIndicator::Running {
            subagents: 0,
            shells: 0,
        });
        assert!(
            !text.contains("agent"),
            "zero subagents must render no suffix; got: {text:?}"
        );
    }

    #[test]
    fn running_card_shows_shell_count() {
        let mut task = make_task(1, TaskStatus::Running);
        task.live_shells = 2;
        let app = App::new(vec![]);
        let indicator = classify_card_indicator(&task, task.status, &app, Utc::now());
        assert_eq!(
            indicator,
            CardIndicator::Running {
                subagents: 0,
                shells: 2
            }
        );
    }

    #[test]
    fn running_card_composes_subagents_and_shells() {
        let mut task = make_task(1, TaskStatus::Running);
        task.live_subagents = 1;
        task.live_shells = 1;
        let app = App::new(vec![]);
        let indicator = classify_card_indicator(&task, task.status, &app, Utc::now());
        assert_eq!(
            indicator,
            CardIndicator::Running {
                subagents: 1,
                shells: 1
            }
        );
    }

    #[test]
    fn stale_shell_sub_status_produces_a_distinct_indicator() {
        let mut task = make_task(1, TaskStatus::Running);
        task.sub_status = SubStatus::StaleShell;
        task.live_shells = 1;
        let app = App::new(vec![]);
        let indicator = classify_card_indicator(&task, task.status, &app, Utc::now());
        assert!(
            matches!(indicator, CardIndicator::StaleShell { .. }),
            "got {indicator:?}"
        );
    }

    #[test]
    fn running_label_shows_shell_count() {
        let text = label_of(CardIndicator::Running {
            subagents: 0,
            shells: 1,
        });
        assert!(text.contains("running \u{00b7} 1 shell"), "got: {text:?}");
    }

    #[test]
    fn running_label_uses_the_plural_for_multiple_shells() {
        let text = label_of(CardIndicator::Running {
            subagents: 0,
            shells: 3,
        });
        assert!(text.contains("running \u{00b7} 3 shells"), "got: {text:?}");
    }

    #[test]
    fn running_label_omits_shell_suffix_at_zero() {
        let text = label_of(CardIndicator::Running {
            subagents: 0,
            shells: 0,
        });
        assert!(
            !text.contains("shell"),
            "zero shells must render no suffix; got: {text:?}"
        );
    }

    #[test]
    fn stale_card_without_timestamp_omits_minutes() {
        let now = Utc::now();
        let task = stale_task(None);
        let app = App::new(vec![]);
        let indicator = classify_card_indicator(&task, task.status, &app, now);
        assert_eq!(
            indicator,
            CardIndicator::Stale {
                inactive_mins: None
            },
        );
        let text = line_text(&render_card_indicator(indicator, &[], None));
        assert!(text.contains("stale"), "got {text:?}");
        assert!(!text.contains('m'), "got {text:?}");
    }
}
