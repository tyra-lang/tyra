# Launch-surface snippets

The `.ty` files here are **public-facing** snippets. The same code appears verbatim in:

- the repository [`README.md`](../../README.md) (the top "pricing model" example),
- the website hero (`tyra-lang/website`, `src/pages/index.astro`),
- launch posts (Hacker News first comment, dev.to, Zenn),
- the social-preview / OG image.

A broken demo at the moment of peak attention is the single worst self-inflicted
launch failure, so these snippets are **frozen and CI-gated**. `check.sh` verifies,
at HEAD, that each snippet is `tyra fmt`-clean, type-checks, compiles, runs (exit 0),
and — when an adjacent `<name>.out` exists — prints exactly that output.

```bash
bash examples/launch/check.sh ./target/release/tyra
```

This runs in the `Static Corpus` workflow on every push / pull request.

**Rule:** if you change a snippet here, update the copies in `README.md` and the
website hero so every public surface stays byte-identical, then re-run the gate.
