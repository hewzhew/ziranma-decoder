# rime-pinyin-simp public dictionary snapshot

This directory vendors an unchanged upstream dictionary for explicit
large-lexicon experiments.

- Project: `rime/rime-pinyin-simp`
- Upstream: https://github.com/rime/rime-pinyin-simp
- Pinned commit: `0c6861ef7420ee780270ca6d993d18d4101049d0`
- Source file:
  `https://raw.githubusercontent.com/rime/rime-pinyin-simp/0c6861ef7420ee780270ca6d993d18d4101049d0/pinyin_simp.dict.yaml`
- SHA-256:
  `e341598343a0f0f2035bb1aafc34a7f3bb7887deeecb3f60796262aaa2983e6b`
- Upstream license: Apache License 2.0; see `LICENSE`
- Upstream attribution: see `AUTHORS`

`pinyin_simp.dict.yaml`, `LICENSE`, and `AUTHORS` are stored byte-for-byte
from that commit. The decoder does not rewrite the snapshot. At import time
it floors zero weights to one and reports every skipped unsupported,
overlong, or duplicate row.

The dictionary header states that it was derived from the Android Open Source
Project Pinyin IME. The upstream `AUTHORS` file attributes that derived
dictionary under Apache License 2.0.
