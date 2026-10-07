/*{
  "DESCRIPTION": "OpenMapper conformance: image filter passthrough with gain",
  "CATEGORIES": ["Filter"],
  "INPUTS": [
    { "NAME": "inputImage", "TYPE": "image" },
    { "NAME": "gain", "TYPE": "float", "DEFAULT": 1.0, "MIN": 0.0, "MAX": 2.0 },
    { "NAME": "swap", "TYPE": "bool", "DEFAULT": false }
  ]
}*/
void main() {
    vec4 c = IMG_THIS_PIXEL(inputImage);
    if (swap) {
        c = c.bgra;
    }
    gl_FragColor = vec4(c.rgb * gain, c.a);
}
