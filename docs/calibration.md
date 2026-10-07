# Output mapping, blending and 3-D calibration

Milestone 8. How an output shows the canvas: a 2-D region with corner pin
and soft edges, or a 3-D model seen through a calibrated projector.
Decision: DECISIONS.md D-025.

## 2-D output mapping (`Output.mapping`)

Each output shows a **region** of the canvas, a quad in canvas space. It
places that region on the output through a **corner pin** (`warp`, a quad
in output space `[0, 1]²`). Output pixels outside the corner pin are black.
The default (whole canvas, unwarped) is the identity.

For output position `o`: region coordinates are `s = W⁻¹ o` and the canvas
position is `c = R s`. `W` maps the unit square to the warp quad and `R`
maps it to the region quad; both are exact perspective maps (homographies).
Sampling is bilinear in linear light, as in the compositor.

### Soft edges

Ramps run inward from the region's left, right, top and bottom edges.
Widths are fractions of the region (0–0.5). Across a ramp the **light**
weight is a symmetric S-curve, `w(t) + w(1 − t) = 1`, with `curve` 1
linear and 2 smooth. Because a projector turns its signal into light
through its gamma, the multiplier is applied to the **signal** as
`w^(1/γ)` (`gamma`, default 2.2). Two projectors whose regions overlap,
each with a ramp spanning the overlap, therefore add up to the same light
as the rest of the image.

Typical two-projector blend, 20 % overlap:

| | region (x) | soft edge |
|---|---|---|
| left projector | 0.0 – 0.6 | right = 0.2 / 0.6 |
| right projector | 0.4 – 1.0 | left = 0.2 / 0.6 |

Verified: `overlapping_soft_edges_sum_to_full_light` renders exactly this
on the GPU and checks that the two outputs' light sums to 1 within 2 %
(8-bit signal quantisation) across the overlap.

## 3-D mapping (`Output.projection`)

An output can instead show a **model** (Wavefront OBJ, positions and UVs)
**as seen by a calibrated projector**, textured with the canvas through
the model's UVs. Surfaces drawn on the canvas therefore wrap onto the
object. OBJ UV origin is bottom-left; canvas origin is top-left.

### Projector model

A pinhole projector, the inverse of a camera. A world point `X` lands on
projector pixel `(u, v) = (fx·x/z + cx, fy·y/z + cy)` with
`(x, y, z) = R X + t`. Pixel `(0, 0)` is the top-left corner of the
image. There is no skew and no lens distortion; projector lenses are
close to distortion-free.

### Calibration

Measure **at least 6 point pairs**: a point on the model (model
coordinates) and the projector pixel that lights it. The model points
must **not all lie on one plane**: a single planar view cannot separate
focal length from distance. Spread them over the whole object and image.

The solver:
1. **Normalised DLT** for the 3×4 projection matrix (Hartley
   normalisation; null vector by symmetric eigen-decomposition).
2. **RQ decomposition** into `K [R | t]`, with the sign chosen so `R` is a
   proper rotation.
3. **Levenberg–Marquardt** refinement of all ten parameters (focal
   lengths, principal point, rotation, translation) on reprojection error.

Error bounds (tests in `crates/om-calibration/tests/synthetic.rs`, 20
random projectors each):

| Input | Result |
|---|---|
| Exact pixels | RMS < 1e-6 px; focal < 1e-6 relative; principal point < 1e-4 px; position < 1e-6 |
| σ = 0.5 px noise, 40 points | RMS 0.5–0.8 px (expected ≈ 0.66); focal < 1.5 %; principal point < 15 px; held-out points < 1.5 px |

End to end, calibrating from 1/8-pixel measurements of a cube and
rendering through the fitted projector matches rendering through the
true projector to a mean difference under 1 code (GPU test and CLI
test).

### Repeatability

The measured points and the fitted projector are both saved in the
project. Calibration is deterministic, so calibrating again from the
saved points reproduces the saved projector exactly. A JSON round trip
reproduces every projected pixel bit for bit.

## Using it

- **UI**: open an output's *Mapping* section for region, corner pin and
  soft edges. Use *3D projection* for the model path, the point table,
  *Calibrate* (reports the RMS error) and *Remove 3D*.
- **CLI**:
  ```
  openmapper-cli render-output show.omproj --output "Left" -o left.png --size 1920x1080
  openmapper-cli calibrate show.omproj --output "Stage" --size 1920x1080
  ```

## Not yet

- **Camera-assisted calibration** (structured light): needs a camera for
  verification; see DECISIONS.md D-025.
- Picking calibration pixels by clicking in the output window (points are
  typed or imported for now).
- Lens distortion, and soft edges for 3-D outputs.
