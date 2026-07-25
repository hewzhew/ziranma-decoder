# Repository instructions

## Scope

This repository studies a local, privacy-first, explainable noisy-channel
decoder for Ziranma double pinyin. Keep each change inside the milestone
described in `docs/design.md`; do not silently expand the project into a full
IME, GUI, Rime integration, Windows TSF service, or neural model.

## Engineering rules

- Use stable Rust.
- Prefer the standard library. Explain and justify every new dependency before
  adding it.
- Keep the project as one crate until a measured need justifies a split.
- Make decoding deterministic.
- Every candidate must expose its spelling choices, correction operation, and
  score breakdown.
- Keep the pinyin-to-Ziranma mapping centralized in `src/codec.rs`; fixtures
  must not silently maintain a second manual mapping.
- Treat `cargo run -- evaluate` as a regression instrument, not a claim of
  real-world input accuracy.
- Preserve the sentence decoder's explicit global error budget; do not let
  each word silently receive its own independent correction.
- Preserve the conservative sentence ordering that ranks complete zero-error
  paths ahead of corrected paths.
- Keep trie changes covered by exhaustive-reference parity tests.
- Add focused tests with each behavioral change.
- Do not perform broad refactors without tests that preserve behavior.
- Treat synthetic frequency weights as experimental configuration, never as
  measured linguistic truth.
- Treat the demo bigram corpus as hand-authored test configuration, not as a
  representative Chinese corpus.

## Privacy rules

- Never commit real chats, real keystroke histories, personal dictionaries,
  secrets, logs, or derived private models.
- Tests and fixtures must be public, manually constructed, or synthetic.
- Keep personal data out of Git even when the remote repository is private.
- Do not add telemetry, network submission, or implicit disk logging.
- Do not read from `data/private/`, `data/raw/`, `logs/`, or
  `models/private/` unless the user explicitly requests a privacy-reviewed
  local experiment.

## Required verification

After code changes, run:

```powershell
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

Report failed checks and known limitations. Do not claim real-world accuracy
from the tiny public demo lexicon.
