/*{
  "DESCRIPTION": "OpenMapper conformance: solid colour input",
  "CATEGORIES": ["Generator"],
  "INPUTS": [
    { "NAME": "tint", "TYPE": "color", "DEFAULT": [0.25, 0.5, 0.75, 1.0] }
  ]
}*/
void main() {
    gl_FragColor = tint;
}
