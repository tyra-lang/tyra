# RFCs

This directory holds proposals for future changes to the Tyra language
specification — new syntax, new semantics, or removal of an existing
restriction. It is distinct from `docs/design/` (ADRs), which records the
rationale of decisions already made — both language design and
reference-implementation choices (spec = what, ADR = why; see
`docs/design/README.md`).

**No RFCs have been filed yet.** This document describes the process for
when the first one is.

## Process

1. **Proposal.** Write a Markdown file under `docs/rfcs/` following the
   template below. Open a pull request.
2. **Discussion.** The proposal is discussed on the pull request. Because
   Tyra's spec is deliberately small and stable, the default answer to a
   new RFC is no — an RFC must argue why the addition doesn't compromise
   interpretive consistency (see `AGENTS.md` and `docs/strategy.md` §9).
3. **Decision.** The maintainer accepts, rejects, or requests changes.
   Rejected RFCs stay in the directory (or its history) as a record of
   what was considered and why it didn't land.
4. **Landing.** An accepted RFC results in:
   - a spec change in `docs/spec/ja/language-spec.md` (authoritative)
     mirrored in `docs/spec/en/language-spec.md`, and
   - an ADR under `docs/design/` recording the rationale for the
     decision.

Spec ambiguities that are *not* proposals for new behavior — e.g. "the
spec doesn't say what happens in case X" — are not RFCs. Those are filed
as GitHub issues with the `spec-clarification` label, per `AGENTS.md`'s
"When the Spec Is Ambiguous" section. An RFC is for proposing something
new or different, not for clarifying something already intended.

## Template

Copy this into a new file named `NNNN-short-title.md` (four-digit,
zero-padded, sequential):

```markdown
# RFC NNNN: Title

## Status

Draft | Discussion | Accepted | Rejected

## Motivation

What problem does this solve? Why is the current spec insufficient?

## Design

The proposed syntax, semantics, and behavior. Include examples.

## Alternatives

What other designs were considered, and why were they not chosen?
Include "do nothing" as an alternative when relevant.

## Spec sections affected

Which sections of `docs/spec/ja/language-spec.md` (and the English
mirror) would need to change if this RFC is accepted.
```
