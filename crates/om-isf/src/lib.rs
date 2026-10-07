// SPDX-License-Identifier: Apache-2.0
//! ISF (Interactive Shader Format) support, implemented independently from
//! the public ISF specification.
//!
//! An ISF file is GLSL fragment code preceded by a `/* … */` JSON header that
//! declares inputs and render passes. This crate:
//! 1. parses the header ([`IsfDoc`]),
//! 2. translates the ISF GLSL dialect into GLSL 4.50 for naga's GLSL
//!    frontend (explicit uniform block, separate textures/samplers, GL
//!    bottom-left coordinate conventions preserved),
//! 3. parses and validates the result with naga, reporting errors against
//!    the user's original line numbers.
//!
//! OpenMapper-specific extensions live under the JSON key `"OPENMAPPER"`;
//! none are defined yet.

mod expr;
mod translate;

pub use expr::eval_dimension;
pub use translate::{Compiled, UniformField, UniformKind, compile};

use serde::Deserialize;

/// Largest accepted shader source (bytes); bounds parse/compile time.
pub const MAX_SOURCE_BYTES: usize = 256 * 1024;

/// ISF loading/compilation failure.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum IsfError {
    #[error("missing ISF JSON header (a /* ... */ comment at the top)")]
    MissingHeader,
    #[error("invalid ISF header JSON: {0}")]
    Header(String),
    #[error("unsupported ISF input type {kind:?} for input {name:?}")]
    UnsupportedInput { name: String, kind: String },
    #[error("invalid input name {0:?}")]
    BadName(String),
    #[error("shader source is larger than {MAX_SOURCE_BYTES} bytes")]
    TooLarge,
    #[error("GLSL error at line {line}: {message}")]
    Glsl { line: usize, message: String },
    #[error("shader validation failed: {0}")]
    Validation(String),
    #[error("pass size expression {expr:?}: {message}")]
    Expression { expr: String, message: String },
}

/// Kind of an ISF input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputKind {
    Event,
    Bool,
    Long,
    Float,
    Point2D,
    Color,
    Image,
}

/// One declared input.
#[derive(Debug, Clone, PartialEq)]
pub struct Input {
    pub name: String,
    pub kind: InputKind,
    pub label: Option<String>,
    /// Default value as given (number, bool, or array).
    pub default: Option<serde_json::Value>,
    pub min: Option<serde_json::Value>,
    pub max: Option<serde_json::Value>,
    /// `long` inputs: allowed values and their labels.
    pub values: Vec<i64>,
    pub labels: Vec<String>,
}

/// One render pass.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Pass {
    /// Name other passes can sample this pass's output as.
    pub target: Option<String>,
    /// Keep the target's contents between frames.
    pub persistent: bool,
    /// Size expressions (e.g. `"$WIDTH/2"`); `None` means full size.
    pub width: Option<String>,
    pub height: Option<String>,
}

/// A parsed ISF document.
#[derive(Debug, Clone, PartialEq)]
pub struct IsfDoc {
    pub description: Option<String>,
    pub categories: Vec<String>,
    pub inputs: Vec<Input>,
    /// At least one pass.
    pub passes: Vec<Pass>,
    /// GLSL after the header.
    pub glsl: String,
    /// 1-based line in the file where `glsl` starts.
    pub glsl_first_line: usize,
}

impl IsfDoc {
    /// True if the shader processes an image (has an `inputImage` input):
    /// usable as an effect. Otherwise it is a generator.
    #[must_use]
    pub fn is_filter(&self) -> bool {
        self.inputs
            .iter()
            .any(|i| i.kind == InputKind::Image && i.name == "inputImage")
    }

    /// Image inputs followed by pass targets, in binding order.
    #[must_use]
    pub fn image_names(&self) -> Vec<String> {
        let mut out: Vec<String> = self
            .inputs
            .iter()
            .filter(|i| i.kind == InputKind::Image)
            .map(|i| i.name.clone())
            .collect();
        for p in &self.passes {
            if let Some(t) = &p.target
                && !out.contains(t)
            {
                out.push(t.clone());
            }
        }
        out
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "UPPERCASE")]
struct RawHeader {
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    categories: Vec<String>,
    #[serde(default)]
    inputs: Vec<RawInput>,
    #[serde(default)]
    passes: Vec<RawPass>,
}

#[derive(Deserialize)]
#[serde(rename_all = "UPPERCASE")]
struct RawInput {
    name: String,
    #[serde(rename = "TYPE")]
    kind: String,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    default: Option<serde_json::Value>,
    #[serde(default)]
    min: Option<serde_json::Value>,
    #[serde(default)]
    max: Option<serde_json::Value>,
    #[serde(default)]
    values: Vec<serde_json::Value>,
    #[serde(default)]
    labels: Vec<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "UPPERCASE")]
struct RawPass {
    #[serde(default)]
    target: Option<String>,
    #[serde(default)]
    persistent: Option<serde_json::Value>,
    #[serde(default)]
    width: Option<serde_json::Value>,
    #[serde(default)]
    height: Option<serde_json::Value>,
}

fn truthy(v: &serde_json::Value) -> bool {
    match v {
        serde_json::Value::Bool(b) => *b,
        serde_json::Value::Number(n) => n.as_f64().is_some_and(|x| x != 0.0),
        serde_json::Value::String(s) => matches!(s.as_str(), "true" | "TRUE" | "1"),
        _ => false,
    }
}

fn dim(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn valid_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next()
        .is_some_and(|f| f.is_ascii_alphabetic() || f == '_')
        && c.all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !s.starts_with("isf_")
        && !s.starts_with("gl_")
}

/// Parses an ISF fragment shader file.
pub fn parse(source: &str) -> Result<IsfDoc, IsfError> {
    if source.len() > MAX_SOURCE_BYTES {
        return Err(IsfError::TooLarge);
    }
    let start = source.find("/*").ok_or(IsfError::MissingHeader)?;
    if !source[..start].trim().is_empty() {
        return Err(IsfError::MissingHeader);
    }
    let end = source[start..]
        .find("*/")
        .map(|e| start + e)
        .ok_or(IsfError::MissingHeader)?;
    let header: RawHeader = serde_json::from_str(&source[start + 2..end])
        .map_err(|e| IsfError::Header(e.to_string()))?;
    let mut inputs = Vec::new();
    for raw in header.inputs {
        if !valid_ident(&raw.name) {
            return Err(IsfError::BadName(raw.name));
        }
        let kind = match raw.kind.as_str() {
            "event" => InputKind::Event,
            "bool" => InputKind::Bool,
            "long" => InputKind::Long,
            "float" => InputKind::Float,
            "point2D" => InputKind::Point2D,
            "color" => InputKind::Color,
            "image" => InputKind::Image,
            other => {
                return Err(IsfError::UnsupportedInput {
                    name: raw.name,
                    kind: other.to_owned(),
                });
            }
        };
        inputs.push(Input {
            name: raw.name,
            kind,
            label: raw.label,
            default: raw.default,
            min: raw.min,
            max: raw.max,
            values: raw
                .values
                .iter()
                .filter_map(serde_json::Value::as_i64)
                .collect(),
            labels: raw.labels,
        });
    }
    let mut passes: Vec<Pass> = header
        .passes
        .into_iter()
        .map(|p| Pass {
            target: p.target.filter(|t| !t.is_empty()),
            persistent: p.persistent.as_ref().is_some_and(truthy),
            width: p.width.as_ref().map(dim),
            height: p.height.as_ref().map(dim),
        })
        .collect();
    for p in &passes {
        if let Some(t) = &p.target
            && !valid_ident(t)
        {
            return Err(IsfError::BadName(t.clone()));
        }
    }
    if passes.is_empty() {
        passes.push(Pass::default());
    }
    let glsl = source[end + 2..].to_owned();
    let glsl_first_line = source[..end + 2].matches('\n').count() + 1;
    Ok(IsfDoc {
        description: header.description,
        categories: header.categories,
        inputs,
        passes,
        glsl,
        glsl_first_line,
    })
}

#[cfg(test)]
mod tests;
