# Contributing

Thank you for helping improve this privacy-first, explainable Ziranma
double-pinyin decoder experiment.

## License

Unless a file or adjacent directory states otherwise, contributions to
project-authored source code, tests, documentation, and configuration are
submitted under the Mozilla Public License 2.0 (`MPL-2.0`).

By submitting a contribution, you represent that you have the right to provide
it under that license. Identify copied or adapted material and retain every
required upstream notice. Do not paste third-party code or data merely because
it is publicly accessible.

Third-party snapshots under `data/public/` retain their own licenses. Adding or
updating one requires:

- an exact upstream revision;
- the applicable license and attribution;
- stable source URLs;
- SHA-256 checksums;
- deterministic transformation and import statistics;
- a focused scale or accounting test;
- an update to `THIRD_PARTY_NOTICES.md`.

## Privacy boundary

Never submit:

- real chats, prompts, document contents, or screenshots containing them;
- raw or reconstructed keystroke histories;
- personal dictionaries, correction histories, preferences, or private models;
- event capsules, protected session segments, logs, databases, or credentials;
- output copied from `data/private/`, `data/raw/`, `logs/`,
  `models/private/`, or `.local/`.

Private data is not acceptable even when redacted informally, when the
repository is private, or when it seems helpful for reproducing a bug. Build a
public, manually constructed, or synthetic reproduction instead. See
`PRIVACY.md` for the complete policy.

Do not ask maintainers to inspect private files in a public issue. A report
derived from private data must first have an explicitly reviewed, bounded,
non-recoverable aggregate schema.

## Project scope

Keep changes inside the current research milestone. This repository does not
silently expand into a complete IME, graphical shell, Windows TSF service,
Rime distribution, telemetry service, or neural model.

Decoder behavior remains:

- deterministic;
- explainable through spelling, correction, and score evidence;
- bounded by one explicit global sentence correction budget;
- conservative about unresolved input;
- covered by focused parity or regression tests.

Discuss broad architectural changes before investing in a large patch.

## Development workflow

Use stable Rust and prefer the standard library. Explain any new dependency
and the measured need it serves.

Before submitting a change, run these commands separately:

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
& .\scripts\release-audit.ps1
```

The release audit reads only Git candidate files and pinned public snapshots.
It does not scan ignored private directories. For a final release candidate,
also require a clean worktree:

```powershell
& .\scripts\release-audit.ps1 -RequireClean
```

Add focused tests for behavioral changes. Tiny fixtures must be public,
manually constructed, or synthetic. Do not present demo-corpus results or one
machine's timings as general real-world accuracy or performance.

## Bug reports

A useful report contains:

- the public or synthetic input;
- the exact command and version;
- expected and observed behavior;
- a minimal candidate explanation or redacted counter;
- operating-system information when the behavior is Windows-specific.

Remove usernames, absolute personal paths, process identifiers from real
sessions, emails, tokens, and private text before posting. If a safe synthetic
reproduction is not possible, describe the behavioral shape without attaching
the original material.
