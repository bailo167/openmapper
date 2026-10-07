# ISF shader support

OpenMapper implements the public ISF (Interactive Shader Format) fragment
shader specification independently (`crates/om-isf`, `om_render::isf`). No
third-party shader code or shader libraries are included; the conformance
corpus (`crates/om-isf/tests/corpus`) consists of original minimal shaders.

## Using shaders

- **Generator media**: add a `.fs` file as media (path field). It renders every
  frame at canvas size; `TIME` follows the media item's own time (restart,
  speed).
- **Effect**: with a `.fs` path in the media path field, choose
  *Effects → + Add → Shader*. Filters read `inputImage` (the surface's media
  after earlier effects); `TIME` is the show time.
- **Live editing**: saved changes to a shader file are recompiled within
  ~0.3 s; compile errors appear next to the media/effect with the line number
  in your file, and the previous output is dropped (never a crash).
- Inputs get automatic controls: `float` (MIN/MAX slider), `bool`/`event`
  (checkbox), `long` (VALUES/LABELS menu or slider), `point2D`, `color`.

## Supported

| Feature | Status |
|---|---|
| JSON header: DESCRIPTION, CATEGORIES, INPUTS, PASSES | yes |
| Input types float, bool, event, long, point2D, color, image (`inputImage`) | yes |
| Built-ins TIME, TIMEDELTA, FRAMEINDEX, PASSINDEX, RENDERSIZE, DATE (zeros) | yes |
| `isf_FragNormCoord`, `vv_FragNormCoord`, `gl_FragCoord` (GL bottom-left) | yes |
| IMG_NORM_PIXEL, IMG_PIXEL, IMG_THIS_PIXEL, IMG_THIS_NORM_PIXEL, IMG_SIZE | yes |
| Multipass with TARGET, WIDTH/HEIGHT expressions (`$WIDTH/2`, functions) | yes |
| PERSISTENT buffers (feedback) | yes |
| Additional image inputs other than `inputImage` | bound transparent (not yet assignable) |
| audio / audioFFT inputs, IMPORTED images, custom vertex shaders (`.vs`) | not yet |
| DATE values | zeros |

## Colour

ISF shaders see sRGB-encoded, straight-alpha colour, as in GL-based tools;
OpenMapper converts from/to its linear premultiplied pipeline around them.

## Safety

Sources over 256 KB are rejected. Shaders are parsed and validated (naga)
before reaching the GPU; GPU-side errors are captured and reported. A shader
that loops for a very long time can still stall the GPU (no runtime watchdog
exists in the graphics APIs used) — test heavy shaders before a show.

## Extensions

Reserved JSON key `"OPENMAPPER"` for OpenMapper-specific extensions (none yet).
