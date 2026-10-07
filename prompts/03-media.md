ROLE: Media/I/O Engineer.
TASK: Implement om-media-core and om-media-ffmpeg.

FFmpeg must remain behind a dynamically linked adapter and must not leak
FFmpeg types into engine/render/project crates.

Implement image, image-sequence, video and audio sources; timestamps;
seek, loop, restart, playback rate; bounded decode queues; thumbnails;
missing/corrupt media errors.

Use exact OpenMapper time for scheduling. Do not use wall-clock frame
increments as the source of truth.

Add a generated media corpus including fractional frame rates and corrupt files.

Acceptance:
- deterministic seeks return expected logical frame;
- 10-minute synthetic A/V test has no accumulated timeline drift;
- pause/resume/reverse policy documented;
- decoder failure cannot crash render thread;
- LGPL provenance/build policy documented.
