# Conway Stroke Data public snapshot

This directory vendors an unchanged generated five-stroke lookup table for
explicit Tab-refinement experiments.

- Project: Conway Stroke Data, compiled manually by Conway (`@yawnoc`)
- Repository: https://github.com/stroke-input/stroke-input-data
- Pinned commit:
  `4449c63198292fd36d68d8068d39641bb6bbf86d`
- Snapshot file: `sequence-characters.txt`
- Source URL:
  https://raw.githubusercontent.com/stroke-input/stroke-input-data/4449c63198292fd36d68d8068d39641bb6bbf86d/sequence-characters.txt
- Snapshot size: 1,188,129 bytes
- Snapshot SHA-256:
  `e712d1ac5b67e4f12b1904aec020f2cb3e3c36c15fb11bdd7af671f66b41ca68`
- Upstream README source:
  https://raw.githubusercontent.com/stroke-input/stroke-input-data/4449c63198292fd36d68d8068d39641bb6bbf86d/README.md
- Upstream README SHA-256:
  `00be0d69159ccbf87c97a07398878e660bd4f735031f58b8bc110b8cb6640d2c`
- License: Creative Commons Attribution 4.0 International
- Complete license source:
  https://creativecommons.org/licenses/by/4.0/legalcode.txt
- Stored license SHA-256:
  `9ba9550ad48438d0836ddab3da480b3b69ffa0aac7b7878b5a0039e7ab429411`

`sequence-characters.txt` and `UPSTREAM_README.md` are stored byte-for-byte
from the pinned commit. `LICENSE.txt` is the complete CC BY 4.0 legal code
retrieved from Creative Commons. The upstream snapshot header carries Conway's
copyright and attribution notice.

The source table uses five numeric classes:

| Upstream digit | Meaning | Local Tab key |
| --- | --- | --- |
| `1` | horizontal / raise | `h` |
| `2` | vertical | `s` |
| `3` | left-slash | `p` |
| `4` | dot / right-press | `n` |
| `5` | turn / bend | `z` |

The local parser does not rewrite the vendored file. At explicit import time it
validates every row, maps the five digits to the keys above, preserves all
alternative sequences, sorts characters by Unicode scalar value, and sorts
each character's alternatives lexicographically.

Deterministic accounting at this revision:

- 60,243 physical lines: 11 comments, 5 blank lines, 60,227 data rows;
- 60,227 distinct numeric sequences;
- 63,005 character-to-sequence assignments;
- 28,165 distinct characters;
- 14,176 characters with two or more alternative sequences;
- at most 90 sequences for one character;
- maximum sequence length 52;
- at most 9 characters sharing one sequence row.

The upstream README says the underlying data was compiled manually and is
likely to contain mistakes. The source notes further state that forms mostly
follow the Kangxi dictionary with leniency and exceptions, and that stroke
order is intentionally lenient. This snapshot is therefore useful candidate
filtering evidence, not an authoritative PRC-standard stroke-order oracle.
Local mapping and validation code are project code; the copied table remains
under CC BY 4.0. No endorsement by Conway or the upstream project is implied.
