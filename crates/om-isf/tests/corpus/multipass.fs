/*{
  "DESCRIPTION": "OpenMapper conformance: two passes; pass 0 renders half-size, pass 1 reads it",
  "INPUTS": [],
  "PASSES": [
    { "TARGET": "half", "WIDTH": "$WIDTH/2", "HEIGHT": "$HEIGHT/2" },
    { }
  ]
}*/
void main() {
    if (PASSINDEX == 0) {
        gl_FragColor = vec4(0.5, isf_FragNormCoord.y, 0.0, 1.0);
    } else {
        vec4 h = IMG_THIS_NORM_PIXEL(half);
        gl_FragColor = vec4(h.r * 2.0, h.g, IMG_SIZE(half).x / 1000.0, 1.0);
    }
}
