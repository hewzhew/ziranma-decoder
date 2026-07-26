# UD Chinese GSDSimp public test snapshot

- Upstream project: Universal Dependencies, UD Chinese GSDSimp
- Repository: https://github.com/UniversalDependencies/UD_Chinese-GSDSimp
- Upstream contributors named in metadata: Peng Qi and Koichi Yasuoka
- Pinned revision:
  `4231dfd59866fa5999ad4a6bc1fdecd7985b3b59`
- Snapshot path: `zh_gsdsimp-ud-test.conllu`
- Pinned source URL:
  https://raw.githubusercontent.com/UniversalDependencies/UD_Chinese-GSDSimp/4231dfd59866fa5999ad4a6bc1fdecd7985b3b59/zh_gsdsimp-ud-test.conllu
- Snapshot size: 1,136,613 bytes
- Snapshot SHA-256:
  `3af8046a6f32477b4d5cf3dd06bbf38682a380fe77aade3f68de97e51ab94900`
- Snapshot accounting: 14,510 lines and 500 sentences
- License: Creative Commons Attribution-ShareAlike 4.0 International

`LICENSE.txt` and `UPSTREAM_README.md` are copied byte-for-byte from the same
revision. The upstream metadata says the treebank includes text and identifies
its genre as wiki. Its changelog also notes that the license permission applies
to the UD annotations and that Google claims no ownership or copyright over the
underlying content. This repository therefore preserves the upstream notice,
attribution, revision, and source rather than making a broader ownership claim.

Only the official test split is vendored. It is used by the explicit
`public-calibrate` command, not by the small exhaustive `evaluate` command.
Natural sentence composition comes from UD; pronunciation choices and weights
come independently from the pinned Rime `pinyin-simp` snapshot. No user text,
keystrokes, chats, or private dictionaries are involved.
