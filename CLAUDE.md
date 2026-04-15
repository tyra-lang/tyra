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
