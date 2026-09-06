//! Frontmatter guards for every skill directory in the repo.
//!
//! Two surfaces, one contract. `plugin/skills/` is embedded in the binary and
//! installed into `~/.claude/plugins/local/dispatch/`; `.claude/skills/` is a
//! plain tracked directory Claude Code auto-discovers for any session inside
//! this repo. They are different delivery mechanisms for the same file format,
//! held to the same rule, and checked here by the same code — an earlier split
//! gave them two parsers and two contradictory definitions of "has a trigger
//! clause", which is exactly the drift these tests exist to catch.
//!
//! Content assertions on the shipped skills' *bodies* stay in
//! `src/setup/plugins.rs`, where they can read the embedded copy.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::path::{Path, PathBuf};

/// Both skill directories, relative to the repo root.
const SKILL_ROOTS: &[&str] = &["plugin/skills", ".claude/skills"];

fn repo_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

/// Every skill directory under `root`, as (skill name, SKILL.md path).
fn skills_in(root: &str) -> Vec<(String, PathBuf)> {
    let dir = repo_path(root);
    let mut found: Vec<(String, PathBuf)> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("{root} must exist: {e}"))
        .map(|e| e.expect("readable dir entry"))
        .filter(|e| e.file_type().expect("entry type").is_dir())
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let path = e.path().join("SKILL.md");
            (name, path)
        })
        .collect();
    found.sort();
    found
}

/// The frontmatter block at the head of a skill file.
fn frontmatter(path: &Path) -> String {
    let body = fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    body.strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---"))
        .map(|(front, _)| front.to_string())
        .unwrap_or_else(|| panic!("{} must open with a YAML frontmatter block", path.display()))
}

/// One key's value out of a frontmatter block. Handles both shapes the repo
/// uses: `key: value` on one line, and a folded `key: >-` block whose value is
/// the indented lines that follow.
fn frontmatter_value(front: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let mut lines = front.lines();
    let first = lines.find_map(|l| l.strip_prefix(&prefix))?.trim();
    if !matches!(first, ">-" | ">" | "|" | "|-") {
        return Some(first.trim_matches('"').to_string());
    }
    let folded: Vec<&str> = lines
        .take_while(|l| l.starts_with(' ') || l.starts_with('\t'))
        .map(str::trim)
        .collect();
    Some(folded.join(" "))
}

/// A loose `.md` file directly under a skills directory is not a skill. Claude
/// Code discovers `<root>/<name>/SKILL.md`, so a bare file there is never
/// loaded and its contents reach nobody — which is what happened to a `lint.md`
/// that sat unread. Failing loudly beats failing silently.
#[test]
fn no_loose_markdown_file_masquerades_as_a_skill() {
    for root in SKILL_ROOTS {
        let stray: Vec<String> = fs::read_dir(repo_path(root))
            .expect("skills dir must exist")
            .map(|e| e.expect("readable dir entry"))
            .filter(|e| e.file_type().expect("entry type").is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            stray.is_empty(),
            "{root}/ holds loose file(s) {stray:?} — a skill must be a directory containing \
             SKILL.md, or Claude Code never loads it"
        );
    }
}

/// Every skill declares a `name:` matching its directory. The directory name is
/// what the slash command resolves to either way, so a missing or mismatched
/// `name:` is silent.
#[test]
fn every_skill_declares_a_name_matching_its_directory() {
    for root in SKILL_ROOTS {
        for (skill, path) in skills_in(root) {
            let name = frontmatter_value(&frontmatter(&path), "name")
                .unwrap_or_else(|| panic!("{root}/{skill}/SKILL.md must declare a `name:`"));
            assert_eq!(
                name, skill,
                "{root}/{skill}/SKILL.md declares name `{name}`, which does not match its directory"
            );
        }
    }
}

/// Every skill's description says *when* to invoke it, not only what it does.
/// A description with no trigger clause is unreachable except by typing the
/// slash command, however good the skill is — the model has nothing to match
/// the user's request against.
///
/// One predicate, not a list of accepted phrasings. An earlier version
/// enumerated eight ("use when", "use after", "use to", …), which was a
/// transcription of the current corpus rather than a rule: every new skill
/// failed until someone appended its exact wording, and `use to` matched
/// inside "refuse to". The convention is a sentence that starts with `Use `,
/// and a skill that does not fit it should gain the clause rather than the
/// predicate gaining a branch.
#[test]
fn every_skill_description_says_when_to_use_it() {
    for root in SKILL_ROOTS {
        for (skill, path) in skills_in(root) {
            let desc = frontmatter_value(&frontmatter(&path), "description")
                .unwrap_or_else(|| panic!("{root}/{skill}/SKILL.md must declare a `description:`"));
            let has_use_sentence = desc
                .split_terminator(['.', '!', '?', '—', ';'])
                .any(|sentence| sentence.trim_start().starts_with("Use "));
            assert!(
                has_use_sentence,
                "{root}/{skill}'s description needs a sentence starting `Use ` saying when to \
                 invoke it, not only what it does — got: {desc}"
            );
        }
    }
}

/// `allium-weed-loop` hands its weed subagent a list of spec files to check.
/// That list was written when `docs/specs/` held three files and was never
/// revisited; it now holds fifteen, so twelve specs sat outside the loop's
/// reach and it could report full alignment having never opened them.
///
/// All-or-nothing rather than a count: either the prompt names no spec file
/// (and points at the directory), or it names every one of them. A subset is
/// the failure mode.
#[test]
fn allium_weed_loop_prompt_does_not_enumerate_a_stale_subset_of_the_specs() {
    let prompt = fs::read_to_string(repo_path(".claude/skills/allium-weed-loop/prompt.md"))
        .expect("allium-weed-loop/prompt.md must exist");

    let all_specs: Vec<String> = fs::read_dir(repo_path("docs/specs"))
        .expect("docs/specs must exist")
        .map(|e| e.expect("readable dir entry"))
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".allium"))
        .collect();
    assert!(
        all_specs.len() > 3,
        "sanity: docs/specs should be populated"
    );

    let missing: Vec<&String> = all_specs.iter().filter(|s| !prompt.contains(*s)).collect();
    if missing.len() == all_specs.len() {
        return; // names none — points at the directory, the intended shape
    }
    assert!(
        missing.is_empty(),
        "allium-weed-loop/prompt.md names some spec files but not {missing:?} — a subset means \
         the weed agent never opens the rest. Point it at docs/specs/ instead of listing files."
    );
}

// -- Per-skill description content ----------------------------------------
//
// The generic rules above say every description has a name and a trigger.
// These pin facts specific to one skill, where a wrong or missing sentence has
// a named consequence. They live here rather than in `src/setup/plugins.rs`
// because they read frontmatter, and the parser lives here.

fn description_of(root: &str, skill: &str) -> String {
    let path = repo_path(root).join(skill).join("SKILL.md");
    frontmatter_value(&frontmatter(&path), "description")
        .unwrap_or_else(|| panic!("{root}/{skill} must declare a `description:`"))
}

fn shipped_description(skill: &str) -> String {
    description_of("plugin/skills", skill)
}

/// wrap-up's description is the summary an agent reads before committing to the
/// skill, and it once stated the step order backwards ("commits remaining
/// changes, then takes one of three paths"). The path is chosen first, and it
/// must be: retro's own behaviour branches on which action was picked, because
/// a fix it makes reaches `base_branch` on rebase and pr but never on done.
#[test]
fn wrap_up_description_puts_the_path_choice_before_the_commit() {
    let desc = shipped_description("wrap-up");
    let choice = desc
        .find("chooses")
        .or_else(|| desc.find("choose"))
        .expect("wrap-up description must say the user chooses a path");
    let commit = desc
        .find("commit")
        .expect("wrap-up description must say it commits remaining changes");
    assert!(
        choice < commit,
        "wrap-up's description must present the path choice before the commit, got: {desc}"
    );
    assert!(
        desc.contains("verif"),
        "wrap-up's description must name the verification step, got: {desc}"
    );
}

/// grill's description used to defer to "any 'grill' trigger phrase" — a list
/// that exists nowhere, so the clause told the model to fire when the user said
/// something that fires it. Name the phrases instead.
#[test]
fn grill_description_names_concrete_trigger_phrases() {
    let desc = shipped_description("grill").to_lowercase();
    assert!(
        !desc.contains("trigger phrase"),
        "grill's description must name its trigger phrases, not refer to a list, got: {desc}"
    );
    assert!(
        desc.contains("stress-test") || desc.contains("poke holes"),
        "grill's description must name at least one concrete trigger phrase, got: {desc}"
    );
}

/// learnings covers four tools; its description listed three.
#[test]
fn learnings_description_covers_the_whole_lifecycle() {
    let desc = shipped_description("learnings").to_lowercase();
    for verb in ["quer", "rate", "record", "delete"] {
        assert!(
            desc.contains(verb),
            "learnings' description must cover '{verb}', got: {desc}"
        );
    }
}

/// summarize's Step 4 branches on whether it was invoked standalone or as a
/// sub-step of another skill, and the two endings differ (stop, versus resume
/// the caller in the same turn). The description described only the standalone
/// case.
#[test]
fn summarize_description_mentions_its_sub_step_role() {
    let desc = shipped_description("summarize").to_lowercase();
    assert!(
        desc.contains("sub-step") || desc.contains("another skill"),
        "summarize's description must say it also runs as another skill's sub-step, got: {desc}"
    );
}
