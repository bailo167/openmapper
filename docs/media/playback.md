# Playback behaviour

## Clocks

- The **show clock** (transport) is the single source of time. Its value is
  computed from total elapsed monotonic time since the last play/seek, never
  accumulated per frame, so it does not drift.
- Each media item's time = `(show − restart point) × speed`, exact rational
  arithmetic. Looping wraps at the media duration; non-looping media holds its
  last frame after the end and its first frame before the start.
- Frames come from container timestamps (exact rationals), snapped to the
  stream's frame grid when within a quarter frame (D-012). The frame shown at
  time t is the last frame whose presentation time ≤ t.

## Transport

| Action | Effect |
|---|---|
| Play | show clock runs from its current value |
| Pause | show clock holds; all media hold their frame; audio is silent |
| Restart (transport) | show clock jumps to 0, keeping play/pause state |
| Restart (media item) | that item's time restarts at 0 at the current show time |

Transport and per-item restarts are runtime state (not saved, not undoable).
Loop, speed and volume are document state (saved, undoable).

## Speed and reverse

- Speed is an exact ratio between −16× and 16× (UI in percent).
- **Reverse** plays by seeking backwards frame by frame. It is smooth with
  intra-only codecs (image sequences, FFV1, ProRes, HAP-class codecs) and
  slow with long-GOP codecs (H.264/HEVC/MPEG-4), where each reverse step can
  decode a whole GOP. Use intra-only media for reverse playback.
- **Audio** plays only at 1× speed; at any other speed it is muted (D-015).

## Decoding never blocks the show

Video decodes on a background thread into a bounded queue (6 frames by
default); the render loop never waits. If the queue does not hold the frame
for the current time (after a jump, or a decoder that cannot keep up), the
previous frame stays on screen and the miss is counted. Audio reads ahead
~0.5 s; missing samples play as silence and are counted as underruns.

## Failure behaviour

Missing files, unreadable or corrupt media and decoder errors are reported
in the media panel and retried every 2 s; they never stop rendering or crash
the application. Truncated files play up to the damage.
