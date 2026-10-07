# Render golden tolerance justifications

Required by docs/PLAN.md: any threshold wider than the default tiers needs a
committed justification.

## GPU vs CPU reference, interior max error on hard-edged media: 4 codes

Default tier: max 1 code. Mean (≤ 0.25 codes) and p99.9 (≤ 2 codes) tiers are
unchanged for every test.

GPU texture units interpolate bilinear samples with reduced-precision weights
(8 fractional bits is common; WebGPU does not mandate more). The CPU reference
uses full `f32` weights. Where a white grid line (linear 1.0) neighbours a dark
texel (linear ≈ 0.01), a weight error of 1/512 changes the sample by ≈ 0.002
linear; sRGB encoding has slope ≈ 6.5 at linear 0.01, giving ≈ 3.3 codes.

Evidence the wider bound is not hiding a mapping error:
`perspective_quad_matches_reference` renders the same perspective mapping with
a smooth gradient (no discontinuities) and must meet the default 1-code tier.
