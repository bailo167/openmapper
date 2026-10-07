// SPDX-License-Identifier: Apache-2.0
//! ISF GLSL → GLSL 4.50 for naga, plus uniform layout.

use std::fmt::Write as _;
use std::sync::OnceLock;

use regex::Regex;

use crate::{InputKind, IsfDoc, IsfError};

/// Type of a uniform block member.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UniformKind {
    Float,
    Int,
    Vec2,
    Vec4,
}

impl UniformKind {
    fn glsl(self) -> &'static str {
        match self {
            Self::Float => "float",
            Self::Int => "int",
            Self::Vec2 => "vec2",
            Self::Vec4 => "vec4",
        }
    }

    /// (size, std140 alignment) in bytes.
    fn layout(self) -> (u32, u32) {
        match self {
            Self::Float | Self::Int => (4, 4),
            Self::Vec2 => (8, 8),
            Self::Vec4 => (16, 16),
        }
    }
}

/// One member of the generated uniform block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UniformField {
    /// Built-in name (`TIME`, …) or the ISF input name.
    pub name: String,
    pub kind: UniformKind,
    /// Byte offset (std140).
    pub offset: u32,
}

/// A translated, validated ISF shader.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub doc: IsfDoc,
    pub module: naga::Module,
    pub info: naga::valid::ModuleInfo,
    /// The generated GLSL 4.50 (for diagnostics).
    pub glsl: String,
    pub uniforms: Vec<UniformField>,
    /// Uniform block size in bytes (multiple of 16).
    pub uniform_size: u32,
    /// Texture names in binding order (binding = index + 1; binding 0 is
    /// the shared sampler).
    pub images: Vec<String>,
}

impl Compiled {
    #[must_use]
    pub fn field(&self, name: &str) -> Option<&UniformField> {
        self.uniforms.iter().find(|f| f.name == name)
    }
}

const BUILTINS: [(&str, UniformKind); 6] = [
    ("TIME", UniformKind::Float),
    ("TIMEDELTA", UniformKind::Float),
    ("FRAMEINDEX", UniformKind::Int),
    ("PASSINDEX", UniformKind::Int),
    ("RENDERSIZE", UniformKind::Vec2),
    ("DATE", UniformKind::Vec4),
];

fn regexes() -> &'static [(Regex, &'static str); 7] {
    static R: OnceLock<[(Regex, &'static str); 7]> = OnceLock::new();
    R.get_or_init(|| {
        #[allow(clippy::unwrap_used)] // literal patterns, checked by tests
        let r = |p: &str| Regex::new(p).unwrap();
        [
            (r(r"(?m)^\s*#version[^\n]*$"), ""),
            (r(r"(?m)^\s*precision\s+\w+\s+\w+\s*;"), ""),
            (r(r"\bgl_FragColor\b"), "isf_FragColor"),
            (r(r"\bgl_FragCoord\b"), "isf_FragCoordGL"),
            (r(r"\bvv_FragNormCoord\b"), "isf_FragNormCoord"),
            (r(r"\btexture2D\s*\("), "texture("),
            (
                r(r"\bvoid\s+main\s*\(\s*(void)?\s*\)"),
                "void isf_user_main()",
            ),
        ]
    })
}

fn img_call() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    #[allow(clippy::unwrap_used)]
    R.get_or_init(|| {
        Regex::new(r"\b(IMG_NORM_PIXEL|IMG_PIXEL|IMG_THIS_NORM_PIXEL|IMG_THIS_PIXEL|IMG_SIZE)\s*\(\s*([A-Za-z_]\w*)\s*([,)])").unwrap()
    })
}

/// Rewrites the ISF dialect in user code.
fn rewrite_user(glsl: &str) -> String {
    let mut out = glsl.to_owned();
    for (re, rep) in regexes() {
        out = re.replace_all(&out, *rep).into_owned();
    }
    img_call()
        .replace_all(&out, |c: &regex::Captures<'_>| {
            let close = if &c[3] == ")" { ")" } else { "" };
            format!("{}_{}({close}", &c[1], &c[2])
        })
        .into_owned()
}

/// Translates and validates `doc`.
pub fn compile(doc: &IsfDoc) -> Result<Compiled, IsfError> {
    // Uniform block layout (std140).
    let mut uniforms = Vec::new();
    let mut offset = 0u32;
    let mut push = |name: &str, kind: UniformKind| {
        let (size, align) = kind.layout();
        offset = offset.div_ceil(align) * align;
        uniforms.push(UniformField {
            name: name.to_owned(),
            kind,
            offset,
        });
        offset += size;
    };
    for (name, kind) in BUILTINS {
        push(name, kind);
    }
    for i in &doc.inputs {
        let kind = match i.kind {
            InputKind::Float => UniformKind::Float,
            InputKind::Long | InputKind::Bool | InputKind::Event => UniformKind::Int,
            InputKind::Point2D => UniformKind::Vec2,
            InputKind::Color => UniformKind::Vec4,
            InputKind::Image | InputKind::Audio | InputKind::AudioFft => continue,
        };
        push(&i.name, kind);
    }
    let uniform_size = offset.div_ceil(16) * 16;
    let images = doc.image_names();

    let mut g = String::new();
    let _ = writeln!(g, "#version 450");
    let _ = writeln!(g, "layout(location = 0) out vec4 isf_FragColor;");
    let _ = writeln!(g, "layout(set = 0, binding = 0) uniform IsfUniforms {{");
    for f in &uniforms {
        let input = doc.inputs.iter().find(|i| i.name == f.name);
        let member = match input.map(|i| i.kind) {
            Some(InputKind::Bool | InputKind::Event) => format!("isf_b_{}", f.name),
            _ => f.name.clone(),
        };
        let _ = writeln!(g, "    {} {member};", f.kind.glsl());
    }
    let _ = writeln!(g, "}};");
    let _ = writeln!(
        g,
        "layout(set = 1, binding = 0) uniform sampler isf_sampler;"
    );
    for (k, name) in images.iter().enumerate() {
        let _ = writeln!(
            g,
            "layout(set = 1, binding = {}) uniform texture2D isf_tex_{name};",
            k + 1
        );
    }
    let _ = writeln!(g, "vec2 isf_FragNormCoord;");
    let _ = writeln!(g, "vec4 isf_FragCoordGL;");
    for i in &doc.inputs {
        if matches!(i.kind, InputKind::Bool | InputKind::Event) {
            let _ = writeln!(g, "bool {};", i.name);
        }
    }
    // ISF image access uses GL conventions (origin bottom-left); textures
    // here are top-left, so flip t.
    for name in &images {
        let _ = writeln!(
            g,
            "vec4 IMG_NORM_PIXEL_{name}(vec2 c) {{ return texture(sampler2D(isf_tex_{name}, isf_sampler), vec2(c.x, 1.0 - c.y)); }}"
        );
        let _ = writeln!(
            g,
            "vec2 IMG_SIZE_{name}() {{ return vec2(textureSize(sampler2D(isf_tex_{name}, isf_sampler), 0)); }}"
        );
        let _ = writeln!(
            g,
            "vec4 IMG_PIXEL_{name}(vec2 c) {{ return IMG_NORM_PIXEL_{name}(c / IMG_SIZE_{name}()); }}"
        );
        let _ = writeln!(
            g,
            "vec4 IMG_THIS_NORM_PIXEL_{name}() {{ return IMG_NORM_PIXEL_{name}(isf_FragNormCoord); }}"
        );
        let _ = writeln!(
            g,
            "vec4 IMG_THIS_PIXEL_{name}() {{ return IMG_NORM_PIXEL_{name}(isf_FragNormCoord); }}"
        );
    }
    let preamble_lines = g.matches('\n').count();
    g.push_str(&rewrite_user(&doc.glsl));
    let _ = writeln!(g);
    let _ = writeln!(g, "void main() {{");
    let _ = writeln!(
        g,
        "    isf_FragCoordGL = vec4(gl_FragCoord.x, RENDERSIZE.y - gl_FragCoord.y, gl_FragCoord.z, gl_FragCoord.w);"
    );
    let _ = writeln!(
        g,
        "    isf_FragNormCoord = isf_FragCoordGL.xy / RENDERSIZE;"
    );
    for i in &doc.inputs {
        if matches!(i.kind, InputKind::Bool | InputKind::Event) {
            let _ = writeln!(g, "    {0} = isf_b_{0} != 0;", i.name);
        }
    }
    let _ = writeln!(g, "    isf_FragColor = vec4(0.0);");
    let _ = writeln!(g, "    isf_user_main();");
    let _ = writeln!(g, "}}");

    let user_line = |generated_line: usize| {
        generated_line
            .saturating_sub(preamble_lines)
            .saturating_add(doc.glsl_first_line.saturating_sub(1))
    };
    let mut frontend = naga::front::glsl::Frontend::default();
    let options = naga::front::glsl::Options::from(naga::ShaderStage::Fragment);
    let module = frontend.parse(&options, &g).map_err(|errs| {
        let first = errs.errors.first();
        let line = first
            .map(|e| e.meta.location(&g).line_number as usize)
            .unwrap_or(0);
        IsfError::Glsl {
            line: user_line(line),
            message: first.map_or_else(|| errs.to_string(), |e| e.kind.to_string()),
        }
    })?;
    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::default(),
    )
    .validate(&module)
    .map_err(|e| IsfError::Validation(e.as_inner().to_string()))?;
    Ok(Compiled {
        doc: doc.clone(),
        module,
        info,
        glsl: g,
        uniforms,
        uniform_size,
        images,
    })
}
