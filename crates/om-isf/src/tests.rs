// SPDX-License-Identifier: Apache-2.0

use super::*;

fn corpus(name: &str) -> String {
    std::fs::read_to_string(format!(
        "{}/tests/corpus/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn every_corpus_shader_parses_and_compiles() {
    let dir = format!("{}/tests/corpus", env!("CARGO_MANIFEST_DIR"));
    let mut n = 0;
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let src = std::fs::read_to_string(&path).unwrap();
        let doc = parse(&src).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        compile(&doc).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        n += 1;
    }
    assert!(n >= 7);
}

#[test]
fn header_is_parsed() {
    let doc = parse(&corpus("inputs.fs")).unwrap();
    assert_eq!(doc.inputs.len(), 5);
    let mode = &doc.inputs[1];
    assert_eq!(mode.kind, InputKind::Long);
    assert_eq!(mode.values, vec![0, 1, 2]);
    assert_eq!(mode.labels, vec!["a", "b", "c"]);
    assert!(!doc.is_filter());
    assert!(parse(&corpus("passthrough.fs")).unwrap().is_filter());
    let mp = parse(&corpus("multipass.fs")).unwrap();
    assert_eq!(mp.passes.len(), 2);
    assert_eq!(mp.passes[0].target.as_deref(), Some("half"));
    assert_eq!(mp.passes[0].width.as_deref(), Some("$WIDTH/2"));
    assert!(parse(&corpus("feedback.fs")).unwrap().passes[0].persistent);
    assert_eq!(mp.image_names(), vec!["half"]);
}

#[test]
fn uniform_layout_is_std140() {
    let c = compile(&parse(&corpus("inputs.fs")).unwrap()).unwrap();
    let off = |n: &str| c.field(n).unwrap().offset;
    assert_eq!(off("TIME"), 0);
    assert_eq!(off("RENDERSIZE"), 16);
    assert_eq!(off("DATE"), 32);
    assert_eq!(off("level"), 48);
    assert_eq!(off("mode"), 52);
    assert_eq!(off("flag"), 56);
    assert_eq!(off("pos"), 64, "vec2 aligns to 8");
    assert_eq!(off("trigger"), 72);
    assert_eq!(c.uniform_size, 80);
}

#[test]
fn malformed_shaders_report_user_line_numbers() {
    let src = "/*{ \"INPUTS\": [] }*/\nvoid main() {\n    gl_FragColor = vec4(1.0);\n    this is not glsl;\n}\n";
    let doc = parse(src).unwrap();
    match compile(&doc) {
        Err(IsfError::Glsl { line, .. }) => assert_eq!(line, 4, "error on the user's line 4"),
        other => panic!("expected a GLSL error, got {other:?}"),
    }
    assert!(matches!(
        parse("void main() {}"),
        Err(IsfError::MissingHeader)
    ));
    assert!(matches!(
        parse("/* not json */ void main(){}"),
        Err(IsfError::Header(_))
    ));
    assert!(matches!(
        parse(r#"/*{"INPUTS":[{"NAME":"a","TYPE":"cube"}]}*/"#),
        Err(IsfError::UnsupportedInput { .. })
    ));
    assert!(matches!(
        parse(r#"/*{"INPUTS":[{"NAME":"gl_x","TYPE":"float"}]}*/"#),
        Err(IsfError::BadName(_))
    ));
    let huge = format!("/*{{}}*/{}", " ".repeat(MAX_SOURCE_BYTES));
    assert!(matches!(parse(&huge), Err(IsfError::TooLarge)));
}

#[test]
fn dialect_rewrites() {
    let c = compile(&parse(&corpus("multipass.fs")).unwrap()).unwrap();
    assert!(c.glsl.contains("IMG_THIS_NORM_PIXEL_half()"));
    assert!(c.glsl.contains("IMG_SIZE_half()"));
    assert!(c.glsl.contains("void isf_user_main()"));
    assert!(!c.glsl.contains("gl_FragColor"));
}
