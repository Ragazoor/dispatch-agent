# 258: Repo filter — `q` to exit

## Context

The repo filter popup (`f` key) can be closed with `Enter` or `Esc`, but not `q`. Since `q` is the standard "go back / quit" key throughout the TUI, it should also close the repo filter popup. The UI hints in the popup and status bar should reflect this.

## Changes

### 1. `src/tui/input.rs` — `handle_key_repo_filter` (line 655)

Add `KeyCode::Char('q')` to the close match arm:

```rust
// Before
KeyCode::Enter | KeyCode::Esc => self.update(Message::CloseRepoFilter),

// After
KeyCode::Enter | KeyCode::Esc | KeyCode::Char('q') => self.update(Message::CloseRepoFilter),
```

### 2. `src/tui/ui.rs` — popup help text (line 1710)

Update the help line in `render_repo_filter_overlay`:

```rust
// Before
Span::styled("Enter/Esc", key_style),

// After
Span::styled("q/Enter/Esc", key_style),
```

### 3. `src/tui/ui.rs` — status bar text (line 1868)

Update the status bar message for `InputMode::RepoFilter`:

```rust
// Before
"Filter repos: 1-9 toggle, (a)ll, Enter/Esc close"

// After
"Filter repos: 1-9 toggle, (a)ll, q/Enter/Esc close"
```

### 4. `src/tui/tests.rs` — add test

Add alongside existing `handle_key_repo_filter_close_enter` and `handle_key_repo_filter_close_esc`:

```rust
#[test]
fn handle_key_repo_filter_close_q() {
    let mut app = make_app();
    app.input.mode = InputMode::RepoFilter;
    app.handle_key(KeyEvent::from(KeyCode::Char('q')));
    assert_eq!(app.input.mode, InputMode::Normal);
}
```

## Verification

```bash
cargo test handle_key_repo_filter_close
cargo clippy
```
