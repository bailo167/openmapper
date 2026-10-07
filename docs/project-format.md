# OpenMapper Project Format

## Principles

- Human-readable JSON (`.omproj`).
- Versioned from the first commit.
- Deterministically serialised (stable key order, stable float formatting).
- Unknown extension fields survive round-trip.
- No machine-specific absolute path is authoritative.
- Every incompatible schema change has a migration.
- A project is never modified in place before a successful replacement file
  has been fully written.

## Root

```json
{
  "format": "openmapper-project",
  "version": 1,
  "project_id": "...",
  "name": "...",
  "timebase": { "ticks_per_second": "254016000000" },
  "assets": {},
  "media": [],
  "surfaces": [],
  "outputs": [],
  "show": { "cues": [], "timelines": [] },
  "extensions": {}
}
```

Large integers that may exceed 2^53 (ticks) are serialised as decimal strings.

## Asset references

Prefer:
1. content hash;
2. project-relative path;
3. last-known absolute path only as a recovery hint.

## IDs

Persistent objects use stable opaque IDs (ULID strings). Array position is
never identity.

## Numeric values

Persistent geometry uses finite validated floating-point values.
Time uses exact integer/rational representations.
NaN and Infinity are rejected on load and on command validation.

## Recovery

Normal save:

```
project.omproj.tmp → write → fsync → validate (re-parse) → atomic rename → project.omproj
```

Accepted commands since the last successful save are appended to the recovery
journal (`project.omproj.journal`, JSON lines) and replayed only when the
journal's base revision matches the saved project's revision.

## Compatibility

Readers:
- reject unknown format families and future versions;
- migrate supported old versions forward;
- preserve unknown `extensions` payloads.

Writers:
- only write the newest supported schema.
