# CLAUDE.md

This project uses `AGENTS.md` as the primary instruction file for all AI coding assistants. **Read `AGENTS.md` first.**

## Claude Code specific additions

In addition to `AGENTS.md`:

### Tool usage

- Use TodoWrite for multi-step tasks (parser implementation, refactoring across crates)
- Use extended thinking when resolving spec ambiguities
- Prefer reading spec files in full rather than grep-ing keywords; `docs/spec/ja/language-spec.md` is short enough to read entirely

### Subagent guidance

- For spec interpretation questions, do not delegate to subagents — the maintainer should be involved
- For mechanical refactors (renames, formatting), subagents are appropriate

### Conversation language

- Respond to the maintainer in Japanese (内容に応じて)
- Code, comments, identifiers, commit messages remain English (per AGENTS.md)

### Implementation review loop (mandatory)

For every non-trivial code change, run this loop before committing:

1. **Implement — Sonnet**: implementation by a Sonnet subagent (Agent tool, model: "sonnet").
2. **Review — Codex**: the diff is reviewed by Codex (subagent_type: "codex:codex-rescue").
3. **Verify findings — Opus**: an Opus subagent (model: "opus") judges each Codex
   finding: CONFIRMED (must fix) / REJECTED (false positive, with reason).
   Only CONFIRMED findings proceed.
4. **Fix — Sonnet**: a Sonnet subagent applies fixes for CONFIRMED findings.
5. Re-run 2–4 on the fix diff until Codex reports no findings or all findings are
   REJECTED. Cap: 3 iterations; then escalate to the maintainer.

The orchestrating session coordinates, runs tests between steps, and never
implements the change itself. Trivial changes (typo-level doc fixes) may skip the loop.
