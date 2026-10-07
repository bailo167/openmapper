/*{
  "DESCRIPTION": "OpenMapper conformance: TIME, FRAMEINDEX and RENDERSIZE",
  "INPUTS": []
}*/
void main() {
    gl_FragColor = vec4(fract(TIME), float(FRAMEINDEX) / 100.0, RENDERSIZE.x / 1000.0, 1.0);
}
