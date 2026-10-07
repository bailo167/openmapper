ROLE: Rendering Specialist + Feature Engineer.
TASK: Complete the first real vertical slice.

Implement wgpu device/surface management, offscreen render targets,
linear-light internal compositing, image upload, quad and triangle surfaces,
UV coordinates, homography/projective warp, opacity and display output.

Expose all mutations through om-command.

Create fixture UV grids and deterministic render goldens.
Connect the existing projector as a fullscreen output through om-output.

Acceptance:
- identity quad reproduces source within golden tolerance;
- arbitrary four-corner quad passes geometry fixtures;
- resize/fullscreen/device-loss paths do not corrupt document state;
- sustained 30-minute projector run has no increasing GPU resource count;
- all three OS builds pass.

Profile before optimising. Record benchmark hardware and results.
