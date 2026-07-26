# Third-party notices

## windows-rs

The Windows-only `tracker-probe` binary uses Microsoft's
[`windows-rs`](https://github.com/microsoft/windows-rs) projection to call
the operating system's UI Automation and input APIs. The dependency is
feature-gated to the Windows namespaces used by the probe and is distributed
under the MIT or Apache License 2.0. It adds no network service or telemetry.

## rime-pinyin-simp

The unchanged public dictionary snapshot under
`data/public/rime-pinyin-simp/` comes from
[`rime/rime-pinyin-simp`](https://github.com/rime/rime-pinyin-simp), pinned
to commit `0c6861ef7420ee780270ca6d993d18d4101049d0`.

The upstream project identifies the dictionary as derived from the Android
Open Source Project Pinyin IME and distributes it under the Apache License,
Version 2.0. The complete upstream license, authorship notice, source URL,
snapshot hash, and transformation notes are retained beside the data.

This notice applies to the vendored dictionary data. It does not imply that
Rime, Android, or their contributors endorse this experiment.

## UD Chinese GSDSimp

The unchanged public CoNLL-U test snapshot under
`data/public/ud-chinese-gsdsimp/` comes from
[`UniversalDependencies/UD_Chinese-GSDSimp`](https://github.com/UniversalDependencies/UD_Chinese-GSDSimp),
pinned to commit `4231dfd59866fa5999ad4a6bc1fdecd7985b3b59`.

The upstream metadata names Peng Qi and Koichi Yasuoka as contributors and
licenses the treebank under Creative Commons Attribution-ShareAlike 4.0
International. The upstream license notice and README, complete CC BY-SA 4.0
legal code, exact train/test source URLs, snapshot hashes, and row accounting
are retained beside the data.

The upstream changelog says the license permission applies to the UD
annotations and that Google claims no ownership or copyright over the
underlying content. This repository preserves that qualification and does not
make a broader ownership claim. This notice does not imply endorsement by
Universal Dependencies, Google, or the named contributors.

## Conway Stroke Data

The unchanged generated stroke table under
`data/public/conway-stroke-data/` comes from
[`stroke-input/stroke-input-data`](https://github.com/stroke-input/stroke-input-data),
compiled manually by Conway (`@yawnoc`) and pinned to commit
`4449c63198292fd36d68d8068d39641bb6bbf86d`.

The upstream file is licensed under Creative Commons Attribution 4.0
International. Its complete license, attribution notice, source URL, snapshot
hash, deterministic import accounting, and local digit-to-key transformation
are retained beside the data. The copied table is not relicensed as project
code.

The upstream author warns that manually compiled records may contain mistakes.
This project uses the table only as replaceable candidate-filtering evidence
and does not claim that it is an authoritative regional stroke-order standard.
This notice does not imply endorsement by Conway or the upstream project.
