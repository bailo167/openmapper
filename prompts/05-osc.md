ROLE: Control/Show Engineer.
TASK: Implement OpenMapper OSC and OSCQuery.

Create a stable OpenMapper OSC namespace generated from command/property
metadata. Expose address, type, access, value and range through OSCQuery.
Implement discovery and UDP OSC transport.

Do not hard-code MadMapper's address namespace into core APIs.
Any future compatibility profile must be a separately generated adapter.

Build protocol tests that enumerate every exposed OpenMapper property,
set it over OSC, query it back and compare engine state.

Acceptance:
- complete OSCQuery tree can be discovered from another process;
- writable values round-trip;
- invalid types/ranges are deterministic;
- service restart/rebind works without project loss.
