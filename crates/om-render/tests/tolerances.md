# Render golden tolerance justifications

Required by docs/PLAN.md: any threshold wider than the default tiers needs a
committed justification.

Default GPU-golden tier (smooth media): mean ≤ 0.25 codes, p99.9 ≤ 2 codes,
max ≤ 1 code.

## Hard-edged media: mean ≤ 0.25, p99.9 ≤ 4, no max-error bound

GPU texture units interpolate bilinear samples with reduced-precision
sub-texel weights. Direct3D requires at least 8 bits; WebGPU mandates none;
software rasterizers differ again. The CPU reference uses full `f32` weights.

Where a white grid line (linear 1.0) neighbours a dark texel (linear ≈ 0.01),
a weight error of 2^-b changes the sample by ≈ 2^-b linear, and sRGB encoding
has slope ≈ 6.5 at linear 0.01:

| Adapter | Observed max (UV grid, perspective) | mean | p99.9 |
|---|---|---|---|
| Apple M4 (Metal) | 3 | 0.034 | 1 |
| Microsoft Basic Render Driver (WARP, D3D12) | 10 | 0.047 | 2 |
| WARP, two stacked UV grids at 70 % (blend test, 64×64) | 3 | 0.047 | 3 |

So on hard edges the max error measures the driver's filter precision, not
whether OpenMapper maps pixels correctly. On small test images the p99.9
covers only a dozen or so samples, all at hard edges, so it inherits the same
driver dependence (hence 4 rather than 2). A real mapping error (wrong
homography, wrong UV, wrong transfer function, wrong blend) moves the mean far
beyond 0.25 codes, and that bound is kept for every test.

Evidence: `perspective_quad_matches_reference` also renders the same mapping
with a smooth gradient (no discontinuities), which must meet the default
max ≤ 1 code tier.
