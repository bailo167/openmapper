# Effects

Each surface has an ordered chain of effects applied to its media *before*
mapping. Disabled effects are skipped; identity settings (blur < 0.5 px,
pixelate 1) cost nothing.

Processing happens at the media's resolution in linear, premultiplied
half-float textures. Colour effects (Color, Invert) un-premultiply and work on
sRGB-encoded values — matching how most image tools behave — then convert
back. Blur works directly on linear premultiplied values (physically correct
mixing, no dark fringes).

| Effect | Parameters | Notes |
|---|---|---|
| Color | brightness −1..1, contrast 0..4, saturation 0..4, hue −180..180°, gamma 0.1..10 | Order: brightness → contrast → saturation → hue → gamma. Hue uses the W3C Filter Effects hue-rotate matrix; luma weights are Rec. 709. |
| Invert | — | Inverts sRGB-encoded colour, keeps alpha. |
| Blur | radius 0..60 px | Separable Gaussian, radius = 3σ, edges clamp. |
| Pixelate | block 1..256 px | Each block takes its centre pixel. |

Every effect has a CPU reference implementation (`om_render::effects`) and a
GPU-vs-CPU golden test.
