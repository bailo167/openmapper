/*{
  "DESCRIPTION": "OpenMapper conformance: normalised coordinates (GL origin bottom-left)",
  "INPUTS": []
}*/
void main() {
    gl_FragColor = vec4(isf_FragNormCoord.x, isf_FragNormCoord.y, 0.0, 1.0);
}
