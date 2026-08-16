# Investigation: Claude Code `/sandbox` mode blast-radius boundary

Task #3984 — research question, no code changes to this repo. Answer below.

## Question

When a Claude Code session runs in sandbox mode, is it confined to the
current working directory and its children, or can it still reach files
outside that tree?

## Answer

**Sandbox mode restricts Bash commands, not the whole session** — Read/Edit/Write
tool calls made directly by the model are governed by the ordinary permission
system, not the OS sandbox. The sandbox is an OS-enforced jail applied only to
commands run through the `Bash` tool (and their child processes).

For sandboxed Bash commands, the boundary is a whitelist, not a strict
"cwd and children only" jail:

- **Write**: restricted to the current working directory + subdirectories,
  plus the session temp dir (`$TMPDIR`). Extra paths can be added via
  `sandbox.filesystem.allowWrite` in settings.
- **Read**: unrestricted across the filesystem by default — sandboxed
  commands *can* read outside the working directory unless specific paths
  are denied via `sandbox.filesystem.denyRead` or `sandbox.credentials`.
- **Always-denied writes** regardless of config: `.claude/` (settings, skills,
  hooks, commands), `.mcp.json`, `~/.bashrc`/`~/.zshrc`/`~/.gitconfig`,
  `.git/hooks`, `.git/config`, and symlinks pointing at any of these.
- **Network**: fully separate control — default-deny outbound, allowed only
  to domains listed in `sandbox.network.allowedDomains`. Enforced via an
  external proxy, not by the sandboxed process itself.

Enforcement mechanism differs by OS: macOS uses Seatbelt (`sandbox-exec`,
built in); Linux/WSL2 uses `bubblewrap` + `socat` (must be installed
separately: `dnf install bubblewrap socat` on Fedora). Native Windows and
WSL1 are not supported.

### Practical implication for blast-radius limiting

- Sandbox mode does **not** by itself stop the model's own Read/Write/Edit
  tool calls from touching files outside the worktree — permission rules
  (allow/deny lists in `settings.json`) are what constrain those.
- It **does** stop shell commands (`rm -rf /`, `curl evil.com`, arbitrary
  scripts) from writing outside the working directory or reaching
  unapproved network hosts, even if the model is tricked or misbehaves.
- Read access outside the directory is **not blocked by default** — a
  sandboxed `cat ~/.ssh/id_rsa` would succeed unless `~/.ssh` is added to
  `sandbox.credentials` or `sandbox.filesystem.denyRead`.
- Known escape vectors: env vars are inherited into the sandbox unless
  explicitly scrubbed (`sandbox.credentials.envVars`, `mode: deny`); Unix
  socket allowlisting (`allowUnixSockets`) can reach host services like
  `/var/run/docker.sock`; network domain matching is hostname-based without
  TLS inspection by default, so domain fronting is a theoretical bypass.

### Relevant settings.json keys

```json
{
  "sandbox": {
    "enabled": true,
    "filesystem": {
      "allowWrite": ["~/.kube"],
      "denyRead": ["~/"],
      "allowRead": ["./"]
    },
    "network": {
      "allowedDomains": ["github.com"]
    },
    "credentials": {
      "files": [{ "path": "~/.ssh", "mode": "deny" }],
      "envVars": [{ "name": "GITHUB_TOKEN", "mode": "deny" }]
    }
  }
}
```

Sources: Claude Code docs — sandboxing, permissions, settings, security
guides (code.claude.com/docs/en/{sandboxing,permissions,settings,security}.md).

## Recommendation for this workspace

If the goal is to cap blast radius of dispatched agents in `dispatch`
worktrees specifically: sandbox mode's write restriction (cwd + temp only,
by default) already lines up well with the existing "agents work from their
worktree" convention (`docs/module-map.md`/CLAUDE.md). It would add real
protection against a Bash command escaping the worktree — something today's
prompt-only instruction does not enforce (see the "Agent Working Directory"
section of the top-level CLAUDE.md). It would **not**, on its own, stop a
misbehaving Read/Edit tool call from reading/writing outside the worktree —
that still relies on the model following instructions plus any permission
rules configured for the session. No code change is needed to "get" this;
it is a Claude Code CLI setting, not something `dispatch` implements or
controls.
