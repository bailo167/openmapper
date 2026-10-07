# Render golden tolerance justifications

Required by docs/PLAN.md: any threshold wider than the default tiers needs a
committed justification.

Default GPU-golden tier: mean ≤ 0.25 codes, max ≤ 1 code. Every render test
also requires p99.9 ≤ 2 codes.

## Hard-edged media: no max-error bound (mean and p99.9 still apply)

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

So on hard edges the max error measures the driver's filter precision, not
whether OpenMapper maps pixels correctly. A real mapping error (wrong
homography, wrong UV, wrong transfer function) moves the mean and p99.9 far
beyond these tiers, and those bounds are kept for every test.

Evidence: `perspective_quad_matches_reference` also renders the same mapping
with a smooth gradient (no discontinuities), which must meet the default
max ≤ 1 code tier.
