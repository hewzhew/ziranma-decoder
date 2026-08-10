# UD Chinese PUD public held-out snapshot

- Upstream project: Universal Dependencies, UD Chinese PUD
- Repository: https://github.com/UniversalDependencies/UD_Chinese-PUD
- Pinned revision:
  `2849afd946a8c01b3e9acdf3e7afa8670cf2777d`
- Snapshot path: `zh_pud-ud-test.conllu`
- Source URL:
  https://raw.githubusercontent.com/UniversalDependencies/UD_Chinese-PUD/2849afd946a8c01b3e9acdf3e7afa8670cf2777d/zh_pud-ud-test.conllu
- Snapshot size: 2,199,911 bytes
- Snapshot SHA-256:
  `e12582af2e2bbc2e27155ca7dc12b6fc4a037dbc716e1f805d67e34ffb2596b3`
- Snapshot accounting: 28,415 lines, 1,000 sentences, 21,415 syntactic
  tokens, and 2,902 punctuation tokens
- License: Creative Commons Attribution-ShareAlike 3.0
- Upstream license URL:
  https://raw.githubusercontent.com/UniversalDependencies/UD_Chinese-PUD/2849afd946a8c01b3e9acdf3e7afa8670cf2777d/LICENSE.txt
- Stored unchanged license: `LICENSE.txt`
- Stored license SHA-256:
  `b278eb53fe50b8bb7fa0d90fb8536c35fdcaa80f9d63812cb51db539555d2a89`
- Upstream README URL:
  https://raw.githubusercontent.com/UniversalDependencies/UD_Chinese-PUD/2849afd946a8c01b3e9acdf3e7afa8670cf2777d/README.md
- Stored unchanged README: `UPSTREAM_README.md`
- Stored README SHA-256:
  `4abc2a54f3b38f8da61c9a751f26d8e3292c99461e9e7fe28b6ad545d3fbd4e7`

The upstream metadata identifies the treebank as 1,000 parallel sentences
from news and Wikipedia, translated professionally and annotated for the
CoNLL 2017 shared task. It preserves the full contributor list and explains
the copyright and annotation boundaries in `UPSTREAM_README.md`; this
repository does not make a broader ownership claim.

This snapshot was not read until the stricter single-character context
development selector, profile grid, final safety gate, and focused synthetic
tests were implemented. It is used once as the final held-out corpus after UD
Chinese GSDSimp dev selects a profile. It must not be used to tune thresholds
or filters after its result is observed. Pronunciations and candidate order
come independently from the pinned public Rime snapshot. No user text,
keystrokes, chats, or private dictionaries are involved.
