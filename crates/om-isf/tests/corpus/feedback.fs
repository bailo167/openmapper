/*{
  "DESCRIPTION": "OpenMapper conformance: persistent buffer accumulates across frames",
  "INPUTS": [],
  "PASSES": [
    { "TARGET": "acc", "PERSISTENT": true, "FLOAT": true },
    { }
  ]
}*/
void main() {
    if (PASSINDEX == 0) {
        vec4 prev = IMG_THIS_NORM_PIXEL(acc);
        gl_FragColor = vec4(prev.r + 0.1, 0.0, 0.0, 1.0);
    } else {
        gl_FragColor = IMG_THIS_NORM_PIXEL(acc);
    }
}
