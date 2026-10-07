/*{
  "DESCRIPTION": "OpenMapper conformance: audio waveform (red) and spectrum (green) inputs",
  "CATEGORIES": ["Generator"],
  "INPUTS": [
    { "NAME": "wave", "TYPE": "audio" },
    { "NAME": "spectrum", "TYPE": "audioFFT" }
  ]
}*/
void main() {
    vec2 uv = isf_FragNormCoord;
    float w = IMG_NORM_PIXEL(wave, vec2(uv.x, 0.25)).r;
    float f = IMG_NORM_PIXEL(spectrum, vec2(uv.x, 0.25)).r;
    gl_FragColor = vec4(w, f, IMG_SIZE(spectrum).x / 1024.0, 1.0);
}
