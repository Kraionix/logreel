# Contributing

This repository is developed agent-first. The rules below are part of the
contract, not etiquette.

## Operating rules

1. **One milestone per session.** Read `docs/PLAN.md` (the development plan),
   `docs/SPEC.md` and the milestone section; then implement only that
   milestone.
2. **Never touch** `repo/` (the proxy), the router, or any deployment
   configuration. The proxy is a read-only reference for behavior, not a build
   target.
3. **Never push, publish, tag, or create a GitHub repository** without an
   explicit instruction in the current session.
4. **No new dependency** without a one-line justification in the commit
   message; no `cargo install`; no network installs. Prefer the standard
   library.
5. **Commit small, conventional commits**, one concern per commit; never
   rewrite pushed history.
6. **Format changes are gated**: SPEC version bump + new fixtures + ADR
   amendment in the same change; old fixtures keep passing.
7. **Stop at acceptance.** Do not gold-plate, do not start the next milestone,
   do not refactor adjacent code.
8. **Report format**: `DONE Mx <commit range> <n tests> <bench|n/a> <next>` or
   `BLOCKED Mx: <reason>`. No silent partial work.
9. **Ask before**: adding a crate, changing a public signature after the M4
   freeze, changing the format, adding a backend, or anything that touches CI
   secrets.
10. **Numbers are recorded, not claimed**: if a benchmark is not run, write
    UNVERIFIED in `bench/RESULTS.md`.

## Local checks

```bash
cargo xtask ci        # fmt, clippy -D warnings, tests, docs
cargo xtask fixtures check
cargo xtask bench     # manual, writes bench/RESULTS.md
```

## Commits

Conventional commits, imperative subject, no trailing period:

```
feat(core): add interest cache for category filters
fix(file): stop at the first invalid frame
docs(spec): define retention order for a backwards clock
```

Every dependency added needs a sentence in the commit body: what it buys and
why the standard library is not enough.
