# UD Chinese GSDSimp public train/test snapshots

- Upstream project: Universal Dependencies, UD Chinese GSDSimp
- Repository: https://github.com/UniversalDependencies/UD_Chinese-GSDSimp
- Upstream contributors named in metadata: Peng Qi and Koichi Yasuoka
- Pinned revision:
  `4231dfd59866fa5999ad4a6bc1fdecd7985b3b59`
- Test snapshot path: `zh_gsdsimp-ud-test.conllu`
- Test source URL:
  https://raw.githubusercontent.com/UniversalDependencies/UD_Chinese-GSDSimp/4231dfd59866fa5999ad4a6bc1fdecd7985b3b59/zh_gsdsimp-ud-test.conllu
- Test size: 1,136,613 bytes
- Test SHA-256:
  `3af8046a6f32477b4d5cf3dd06bbf38682a380fe77aade3f68de97e51ab94900`
- Test accounting: 14,510 lines, 500 sentences, 12,010 syntactic tokens, and
  1,691 punctuation tokens
- Train snapshot path: `zh_gsdsimp-ud-train.conllu`
- Train source URL:
  https://raw.githubusercontent.com/UniversalDependencies/UD_Chinese-GSDSimp/4231dfd59866fa5999ad4a6bc1fdecd7985b3b59/zh_gsdsimp-ud-train.conllu
- Train size: 9,321,012 bytes
- Train SHA-256:
  `956636fe612a1166e8b19e7413fee2e73d68231aca2f0455be2c616b947d629d`
- Train accounting: 118,599 lines, 3,997 sentences, 98,614 syntactic tokens,
  and 13,627 punctuation tokens
- License: Creative Commons Attribution-ShareAlike 4.0 International
- Complete license source:
  https://creativecommons.org/licenses/by-sa/4.0/legalcode.txt
- Stored complete license: `CC-BY-SA-4.0.txt`
- Stored complete license SHA-256:
  `28a9529c7d0bb4dc51f4bf5c116a3d16ef247a052f7591466768ddf563fd1cf5`

`LICENSE.txt` and `UPSTREAM_README.md` are copied byte-for-byte from the same
revision. `CC-BY-SA-4.0.txt` is the complete legal code retrieved from Creative
Commons; it supplements rather than replaces the unchanged upstream notice.
The upstream metadata says the treebank includes text and identifies its genre
as wiki. Its changelog also notes that the license permission applies to the UD
annotations and that Google claims no ownership or copyright over the
underlying content. This repository therefore preserves the upstream notice,
attribution, revision, and source rather than making a broader ownership claim.

Only the official train and test splits are vendored. `public-calibrate` trains
context evidence from train and evaluates only on test; the small exhaustive
`evaluate` command uses neither. Natural sentence composition comes from UD;
pronunciation choices and weights come independently from the pinned Rime
`pinyin-simp` snapshot. No user text, keystrokes, chats, or private
dictionaries are involved.
