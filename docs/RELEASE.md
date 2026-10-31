# Release checklist

Version: 0.1.0 (single workspace version).
Tag: annotated `v0.1.0`.

## Order

`cargo publish` follows the dependency graph:

1. `logreel-store`
2. `logreel-core`
3. `logreel-derive`
4. `logreel-file`
5. `logreel-store-redb`
6. `logreel`

## Before tagging

- `cargo xtask ci` is green.
- `cargo xtask fixtures check` matches the committed bytes and hashes.
- `cargo run -p logreel-bench --release -- --check` is within the budget.
- `CHANGELOG.md` has the release entry and the date.
- `cargo doc --workspace --no-deps --all-features` has no warnings.

## Dry-run limitation

`cargo publish --dry-run` resolves the packaged dependencies from crates.io.
A crate whose `logreel-*` dependency is not published yet fails in "prepare
local package" with `no matching package named ...`, before any build.

Verified on 2026-10-31 with this tree:

| crate | dry-run |
|---|---|
| `logreel-store` | passes (packaged, verified, upload aborted) |
| `logreel-derive` | passes (no logreel dependencies) |
| `logreel-core` | blocked: `logreel-store` not on crates.io |
| `logreel-file` | blocked: `logreel-store` not on crates.io |
| `logreel-store-redb` | blocked: `logreel-store` not on crates.io |
| `logreel` | blocked: `logreel-core` not on crates.io |

This is a property of the registry, not of the packages. The dry-run of the
remaining four becomes possible only in the same session as the real publish,
in the order above. Until then, `cargo package --list` is the packaging check
that does not need the registry.

## Tag

```bash
git tag -a v0.1.0 -m "logreel 0.1.0"
git push origin main --tags
```
