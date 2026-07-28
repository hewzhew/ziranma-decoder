# Privacy policy

This project is local-first and privacy-bounded. Its open-source license
governs distributed project material; it does not turn a user's input,
keystrokes, chats, personal dictionary, private model, or local recordings
into project source code or public data.

## Data classes

### Public project material

The following may be committed after provenance and license review:

- project-authored source code, documentation, configuration, and tests;
- manually constructed or synthetic fixtures that contain no real user data;
- pinned public datasets with their exact revision, license, attribution,
  source URL, checksum, transformation notes, and deterministic import stats.

### Private local material

The following must never be committed, synced as project material, included in
a release archive, used as a public fixture, or submitted in an issue:

- real chats, prompts, document text, and composition text;
- raw or reconstructed keystroke histories;
- personal dictionaries, correction histories, and learned preferences;
- private event capsules, session segments, logs, databases, and derived
  private models;
- credentials, access tokens, environment files, encryption material, and
  other secrets.

The fixed local locations are excluded from Git:

```text
data/private/
data/raw/
logs/
models/private/
.local/
```

`.gitignore` is an accident-prevention layer, not an access-control or
encryption mechanism. Already tracked data remains in Git history until the
history is explicitly remediated.

## Collection and storage

The ordinary decoder and public evaluation commands do not collect input or
write telemetry. The project has no network submission path.

Private capture requires an explicit local command and a bounded target. The
continuous Codex recorder:

- targets only the Codex edit control;
- writes under `data/private/continuous-capture/`;
- protects segment payloads with Windows DPAPI for the current user;
- keeps v2 pipeline-integrity counters (input volume, callback failures,
  baseline epochs, and coarse close reasons) inside the protected payload;
- reports redacted lifecycle and, only in explicit health mode, aggregate
  behavioral count metadata;
- sends nothing over the network.

Integrity counters do not contain text, key values, exact selection offsets,
or per-key timing, but they can still reveal input volume, editing habits, and
continuity patterns. They are behavioral metadata, not anonymous data. Legacy
v1 segments have no such evidence; reports must mark it unavailable rather
than treating it as zero. The recorder cannot persist a write failure inside
the segment whose write failed, so it fails closed instead of claiming a zero
failure count.

The older `CAPTURE_HEALTH` aggregates, recorder lifecycle/control state, saved
event counts, session identifiers, and flush times are behavioral metadata too.
Redacted means that text, key values, personal paths, and recoverable model
state are absent; it does not mean that the remaining report is anonymous or
safe to publish without review.

The manual event-capsule path can store plaintext only when both an explicit
output path and the separate `--allow-private-plaintext` acknowledgement are
present. Plaintext capsules are private and are not suitable for publication.

DPAPI protects data at rest against simple file disclosure. It does not protect
against malware or another process already acting with the same Windows user
authority. The project currently provides no automatic retention or secure
deletion policy.

## Analysis boundary

Private data is read only for an explicitly requested, privacy-reviewed local
experiment. Tools must not scan private directories for a newest or convenient
file; callers name the exact input or an exact bounded session selector.

Reports intended for sharing must be redacted aggregates that contain no text,
key values, personal paths, or recoverable private model state. A metric derived
from private data is not automatically safe: its schema and minimum aggregation
must be reviewed before publication.

## Contributions and issue reports

Contributors must reproduce bugs with public, manually constructed, or
synthetic examples. Do not attach real input logs, screenshots containing
private conversations, private capsules, protected segments, personal
dictionaries, or derived user models.

Before committing or packaging a release:

1. inspect the exact Git file list and history for private paths;
2. scan the exact release tree for credentials and personal identifiers;
3. verify that private and local directories are absent from the archive;
4. verify all public datasets against their recorded checksums and licenses;
5. run the project's complete format, lint, and test checks.

If private material is committed accidentally, stop distribution. Removing the
working-tree file is not sufficient; assess history remediation and rotate any
exposed credentials before resuming publication.

## License separation

Project-authored material is licensed under MPL-2.0 unless a file or adjacent
directory says otherwise. Third-party public data retains its own license.
Private user material is neither contributed to this project nor distributed
under the project's open-source license.
