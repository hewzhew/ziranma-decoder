# Ziranma conversation overlay v1

This small conversational vocabulary is manually curated for the Ziranma TSF
Alpha. It is original project data distributed under the same MPL-2.0 license
as the surrounding source code; it is not copied from a third-party
dictionary or learned from private input.

The `frequency` column contains deterministic relative ordering weights, not
corpus measurements. The Alpha consults this overlay only for an exact,
complete Ziranma code and deduplicates it against the pinned public Rime
candidate snapshot and the separate technical overlay.

The overlay remains embedded in the local development DLL until the immutable
candidate-package format can represent and verify multiple sources with
separate provenance.
