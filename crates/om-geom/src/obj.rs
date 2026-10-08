// SPDX-License-Identifier: Apache-2.0
//! Wavefront OBJ geometry (positions, texture coordinates, faces).
//!
//! Only what 3-D mapping needs: `v`, `vt` and `f` (triangles, quads and
//! larger convex polygons, fanned into triangles; negative indices
//! allowed). Normals, materials, groups and curves are ignored. Malformed
//! input is an error with a line number, never a panic, and sizes are
//! bounded.

/// Most vertices accepted after triangulation (three per triangle).
pub const MAX_VERTICES: usize = 3_000_000;
/// Largest OBJ file read.
pub const MAX_FILE_BYTES: u64 = 512 << 20;

/// A triangle soup: three entries per triangle.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    /// OBJ texture coordinates (origin bottom-left); `[0, 0]` where a face
    /// has none.
    pub uvs: Vec<[f32; 2]>,
}

impl Mesh {
    #[must_use]
    pub fn triangles(&self) -> usize {
        self.positions.len() / 3
    }

    /// Axis-aligned bounds `(min, max)`, or `None` if empty.
    #[must_use]
    pub fn bounds(&self) -> Option<([f32; 3], [f32; 3])> {
        let first = *self.positions.first()?;
        Some(self.positions.iter().fold((first, first), |(lo, hi), p| {
            (
                std::array::from_fn(|k| lo[k].min(p[k])),
                std::array::from_fn(|k| hi[k].max(p[k])),
            )
        }))
    }
}

/// Why an OBJ file could not be read.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("OBJ line {line}: {message}")]
pub struct ObjError {
    pub line: usize,
    pub message: String,
}

fn err(line: usize, message: impl Into<String>) -> ObjError {
    ObjError {
        line,
        message: message.into(),
    }
}

fn floats<const N: usize>(parts: &[&str], line: usize, min: usize) -> Result<[f32; N], ObjError> {
    if parts.len() < min {
        return Err(err(line, format!("expected at least {min} numbers")));
    }
    let mut out = [0f32; N];
    for (k, v) in out.iter_mut().enumerate() {
        let Some(text) = parts.get(k) else { break };
        let x: f32 = text
            .parse()
            .map_err(|_| err(line, format!("{text:?} is not a number")))?;
        if !x.is_finite() {
            return Err(err(line, "non-finite number"));
        }
        *v = x;
    }
    Ok(out)
}

/// Resolves a 1-based (or negative, relative) OBJ index.
fn index(text: &str, count: usize, line: usize) -> Result<usize, ObjError> {
    let i: i64 = text
        .parse()
        .map_err(|_| err(line, format!("{text:?} is not an index")))?;
    let count_i = i64::try_from(count).unwrap_or(i64::MAX);
    let resolved = if i > 0 { i - 1 } else { count_i + i };
    if i == 0 || resolved < 0 || resolved >= count_i {
        return Err(err(
            line,
            format!("index {i} is out of range (1..={count})"),
        ));
    }
    usize::try_from(resolved).map_err(|_| err(line, "index out of range"))
}

/// Parses OBJ text.
pub fn parse(text: &str) -> Result<Mesh, ObjError> {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut uvs: Vec<[f32; 2]> = Vec::new();
    let mut mesh = Mesh::default();
    for (n, raw) in text.lines().enumerate() {
        let line = n + 1;
        let content = raw.split('#').next().unwrap_or("");
        let mut parts = content.split_whitespace();
        let Some(tag) = parts.next() else { continue };
        let rest: Vec<&str> = parts.collect();
        match tag {
            "v" => positions.push(floats::<3>(&rest, line, 3)?),
            "vt" => uvs.push(floats::<2>(&rest, line, 1)?),
            "f" => {
                if rest.len() < 3 {
                    return Err(err(line, "a face needs at least 3 vertices"));
                }
                // Checked before fanning out: one huge face must not allocate
                // far past the cap.
                if mesh.positions.len() + 3 * (rest.len() - 2) > MAX_VERTICES {
                    return Err(err(line, "model is too large"));
                }
                let mut corners = Vec::with_capacity(rest.len());
                for v in &rest {
                    let mut fields = v.split('/');
                    let p = index(fields.next().unwrap_or(""), positions.len(), line)?;
                    let t = match fields.next() {
                        Some(t) if !t.is_empty() => Some(index(t, uvs.len(), line)?),
                        _ => None,
                    };
                    corners.push((p, t));
                }
                for k in 1..corners.len() - 1 {
                    for &(p, t) in &[corners[0], corners[k], corners[k + 1]] {
                        mesh.positions.push(positions[p]);
                        mesh.uvs.push(t.map_or([0.0, 0.0], |t| uvs[t]));
                    }
                }
                if mesh.positions.len() > MAX_VERTICES {
                    return Err(err(line, "model is too large"));
                }
            }
            _ => {}
        }
    }
    if mesh.positions.is_empty() {
        return Err(err(0, "the file contains no faces"));
    }
    Ok(mesh)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn one_huge_face_is_refused_before_fanning_out() {
        let corners = MAX_VERTICES / 3 + 3;
        let text = format!("v 0 0 0\nf{}\n", " 1".repeat(corners));
        assert!(parse(&text).is_err());
    }

    #[test]
    fn parses_quads_negative_indices_and_comments() {
        let text = "# a unit quad\n\
                    v 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\n\
                    vt 0 0\nvt 1 0\nvt 1 1\nvt 0 1\n\
                    vn 0 0 1\n\
                    usemtl x\n\
                    f -4/-4/1 -3/-3/1 -2/-2/1 -1/-1/1 # trailing comment\n";
        let m = parse(text).unwrap();
        assert_eq!(m.triangles(), 2);
        assert_eq!(m.positions[0], [0.0, 0.0, 0.0]);
        assert_eq!(m.positions[4], [1.0, 1.0, 0.0]);
        assert_eq!(m.uvs[2], [1.0, 1.0]);
        assert_eq!(m.bounds(), Some(([0.0, 0.0, 0.0], [1.0, 1.0, 0.0])));
    }

    #[test]
    fn faces_without_uvs_get_zero() {
        let m = parse("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\nf 1//1 2//1 3//1\n").unwrap();
        assert_eq!(m.triangles(), 2);
        assert!(m.uvs.iter().all(|t| *t == [0.0, 0.0]));
    }

    #[test]
    fn malformed_files_are_errors_with_line_numbers() {
        for (text, line) in [
            ("v 0 0\n", 1),
            ("v 0 0 x\n", 1),
            ("v 0 0 0\nv 1 0 0\nf 1 2\n", 3),
            ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 4\n", 4),
            ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 0\n", 4),
            ("v 0 0 0\nv 1 0 0\nv 0 1 0\nf 1/5 2 3\n", 4),
            ("v 0 0 nan\n", 1),
            ("v 0 0 0\n", 0),
        ] {
            assert_eq!(parse(text).unwrap_err().line, line, "{text:?}");
        }
    }
}
