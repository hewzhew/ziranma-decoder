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
- Every candidate must expose its correction operation and score breakdown.
- Add focused tests with each behavioral change.
- Do not perform broad refactors without tests that preserve behavior.
- Treat synthetic frequency weights as experimental configuration, never as
  measured linguistic truth.

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
