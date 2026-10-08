# Clean-Room Engineering Policy

OpenMapper is independently implemented.

Permitted implementation sources:
- public standards;
- public API/protocol documentation;
- OpenMapper neutral behavioural specifications (`docs/behaviour/`);
- permissively compatible open-source dependencies with recorded provenance;
- independently created tests and fixtures.

## Reference research order

| Level | Technique | Status |
|---|---|---|
| L0 | Public MadMapper docs, standards, tutorials, standard network protocols | Always permitted |
| L1 | Lawful black-box operation, OSC/OSCQuery, controlled screenshots/output captures | Preferred |
| L2 | Differential testing: vary one input, record one observable output | Preferred when L1 is insufficient |
| L3 | REA process capture/comparison and binary inventory | Requires a written need/purpose ticket |
| L4 | Static decompilation/disassembly | Exceptional; legal-purpose review first |
| Forbidden | Licence/activation bypass, TPM circumvention, copying code/assets/pseudocode into implementation | Never |

If a question can be answered via OSCQuery or a controlled frame comparison,
L3/L4 must not be used.

## Never transfer into the implementation repository

- proprietary source or decompiler pseudocode;
- extracted icons, shaders, presets, artwork, fonts or media;
- binary-derived code fragments, addresses, symbol names or constants;
- screenshots/reference captures unless cleared as documentation evidence;
- licence/activation implementation details.

## Never bypass or disable

- activation;
- licence enforcement;
- technological protection mechanisms.

## Repositories

`openmapper-reference` is permanently private and holds all raw evidence.
`openmapper` may receive only neutral behaviour descriptions, written as
observable input → output statements, e.g.

> Given normalized quad corners A..D, moving upper-right X from 0.8 to 0.9
> while all other parameters are constant produces this observable mapping…

and never as implementation detail ("function sub_XXXXXX uses constant 0x…").

The CI provenance check (`cargo xtask provenance`) rejects tracked files that
look like reference evidence or binary-analysis output.

## Neutral specification record

Every file in `docs/behaviour/` records:
- public sources checked;
- experiments performed;
- behaviour observed;
- uncertainty;
- provenance reviewer;
- date/version tested.

When in doubt, defer the feature rather than contaminate the implementation.

Before any public beta: archive the licence agreement presented by the
legitimately installed MadMapper version. The Australian IP solicitor's
review of the clean-room record that this policy originally required was
**waived by the project owner on 2026-10-08** (D-033,
docs/release/legal-posture.md); the provenance scan report is published with
every release in its place, and the review may still be commissioned later.
