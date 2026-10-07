/*{
  "DESCRIPTION": "OpenMapper conformance: every scalar input type encoded into the output",
  "INPUTS": [
    { "NAME": "level", "TYPE": "float", "DEFAULT": 0.5 },
    { "NAME": "mode", "TYPE": "long", "VALUES": [0, 1, 2], "LABELS": ["a", "b", "c"], "DEFAULT": 1 },
    { "NAME": "flag", "TYPE": "bool", "DEFAULT": true },
    { "NAME": "pos", "TYPE": "point2D", "DEFAULT": [0.25, 0.75] },
    { "NAME": "trigger", "TYPE": "event" }
  ]
}*/
void main() {
    float f = flag ? 1.0 : 0.0;
    float t = trigger ? 1.0 : 0.0;
    gl_FragColor = vec4(level, float(mode) / 4.0, pos.x + 0.5 * f, pos.y - 0.5 * t);
}
