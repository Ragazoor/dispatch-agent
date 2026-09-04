//! The one definition of where dispatch's files live inside the operator's
//! Claude Code configuration directory.
//!
//! Two halves of the system name these locations and they must agree. The
//! **reader** is the spawn constant (`src/dispatch/prompts.rs::DISPATCH_PLUGIN_DIR`),
//! which names them to `claude` as fixed `~/`-relative literals inside a shell
//! command line. The **writers** are startup, which keeps the statusLine
//! settings file current, and `dispatch setup`, which installs the plugin;
//! both build a `PathBuf` from a resolved directory instead.
//!
//! Those two mechanisms stay separate, but the *strings* they are built from
//! are stated here, once. Changing a name is then a single edit that moves both
//! halves together — there is no second copy for it to miss. Both halves are
//! still pinned to hand-written expectations (`src/dispatch/tests.rs` for the
//! constant, `src/setup/mod.rs`'s tests for the layout) so the change has to be
//! confirmed on each side rather than carrying through silently.
//!
//! These are `macro_rules!` rather than `const`s because `concat!` accepts
//! literals and macro expansions but never a `const` item, and the spawn
//! constant is assembled with `concat!`.
//!
//! Why the reader is a `const` at all is a repo convention rather than a
//! language requirement — both launch sites interpolate it with `format!`
//! (`src/dispatch/agents.rs`), and the `--mcp-config` argument spliced onto the
//! same command line *is* a runtime path, shell-quoted via
//! `crate::process::shell_quote`. What the `const` buys is that the flags
//! cannot go missing: `claude` refuses to start without the settings file, and
//! a value with no runtime source cannot be absent.
//!
//! What this does NOT settle: whether the configuration directory is found
//! under the home directory at all. `~` is resolved by the launching shell, so
//! the two halves agree only while the lookup reads nothing but `$HOME` — see
//! `crate::setup::claude_dir` and docs/specs/dispatch.allium:
//! `SpawnSitesAndStartupNameTheSameConfigurationDirectory`.
//!
//! Nor does it settle what happens when there is no home directory to find it
//! under. That is `crate::setup::home_dir`, the one place `$HOME` is read and
//! the one definition of what makes it unavailable — see
//! docs/specs/dispatch.allium:
//! `AnUnavailableHomeDirectoryIsAFailureNotAPath`.
//!
//! Note that these tokens are not simply "names inside `~/.claude`":
//! `claude_dir_name!()` names that directory itself, a sibling of the trust
//! store rather than something within it. What they have in common is a
//! spawn-side literal to agree with, which is why the trust store's own name
//! is stated in `crate::setup` and not here.

/// The configuration directory's name, directly beneath the home directory.
macro_rules! claude_dir_name {
    () => {
        ".claude"
    };
}

/// The dispatch-owned statusLine settings file, relative to the configuration
/// directory. Never `settings.json`, which dispatch deliberately never writes.
macro_rules! statusline_settings_name {
    () => {
        "dispatch-statusline.json"
    };
}

/// Where `dispatch setup` installs the agent-facing plugin, relative to the
/// configuration directory.
///
/// One `/`-joined string rather than nested segments so the same token serves
/// both a shell command line and a `Path::join`. POSIX-only, like the rest of
/// the crate.
macro_rules! plugin_dir_rel {
    () => {
        "plugins/local/dispatch"
    };
}

pub(crate) use {claude_dir_name, plugin_dir_rel, statusline_settings_name};
