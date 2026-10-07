ROLE: Rendering Specialist.
TASK: Implement standard ISF support independently from the public ISF spec.

Parse ISF JSON metadata, validate inputs, compile supported GLSL into the
OpenMapper GPU shader path, provide TIME/TIMEDELTA/RENDERSIZE-equivalent
standard semantics where required by ISF, and implement image filters and
generators.

Do not reproduce MadMapper proprietary shader-library code or assets.

Create a conformance corpus from original minimal shaders exercising every
supported input type and multipass behaviour.

Acceptance:
- valid corpus renders deterministically;
- malformed shaders produce diagnostic errors without killing output;
- shader compilation is bounded/cancellable;
- OpenMapper-specific extensions use an explicit namespace.
