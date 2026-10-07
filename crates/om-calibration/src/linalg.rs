// SPDX-License-Identifier: Apache-2.0
//! Small dense linear algebra in `f64`: symmetric eigen-decomposition
//! (cyclic Jacobi) for null-space problems, and Cholesky solves for
//! normal equations. Sizes here are tiny (≤ 12), so clarity wins over speed.
// Index loops mirror the matrix notation they implement.
#![allow(clippy::needless_range_loop)]

/// A row-major `n × n` matrix.
pub type Mat = Vec<Vec<f64>>;

/// `Aᵀ A` for a row-major `rows × n` matrix given as rows.
#[must_use]
pub fn gram(rows: &[Vec<f64>], n: usize) -> Mat {
    let mut m = vec![vec![0.0; n]; n];
    for r in rows {
        for i in 0..n {
            if r[i] == 0.0 {
                continue;
            }
            for j in i..n {
                m[i][j] += r[i] * r[j];
            }
        }
    }
    for i in 0..n {
        for j in 0..i {
            m[i][j] = m[j][i];
        }
    }
    m
}

/// Eigenvalues and eigenvectors (as columns of the returned matrix) of a
/// symmetric matrix, by cyclic Jacobi rotations.
#[must_use]
pub fn symmetric_eigen(a: &Mat) -> (Vec<f64>, Mat) {
    let n = a.len();
    let mut a = a.clone();
    let mut v: Mat = (0..n)
        .map(|i| (0..n).map(|j| if i == j { 1.0 } else { 0.0 }).collect())
        .collect();
    for _sweep in 0..100 {
        let off: f64 = (0..n)
            .flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[i][j] * a[i][j])
            .sum();
        let scale: f64 = (0..n).map(|i| a[i][i] * a[i][i]).sum::<f64>().max(1e-300);
        if off <= 1e-30 * scale {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..n {
                    let (akp, akq) = (a[k][p], a[k][q]);
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let (apk, aqk) = (a[p][k], a[q][k]);
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let (vkp, vkq) = (v[k][p], v[k][q]);
                    v[k][p] = c * vkp - s * vkq;
                    v[k][q] = s * vkp + c * vkq;
                }
            }
        }
    }
    ((0..n).map(|i| a[i][i]).collect(), v)
}

/// The unit vector `x` minimising `|A x|` (the eigenvector of `AᵀA` with
/// the smallest eigenvalue).
#[must_use]
pub fn null_vector(rows: &[Vec<f64>], n: usize) -> Vec<f64> {
    let (values, vectors) = symmetric_eigen(&gram(rows, n));
    let k = values
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.total_cmp(b.1))
        .map_or(0, |(i, _)| i);
    (0..n).map(|i| vectors[i][k]).collect()
}

/// Solves `A x = b` for symmetric positive-definite `A` (Cholesky).
/// `None` if `A` is not positive definite.
#[must_use]
pub fn cholesky_solve(a: &Mat, b: &[f64]) -> Option<Vec<f64>> {
    let n = a.len();
    let mut l = vec![vec![0.0; n]; n];
    for i in 0..n {
        for j in 0..=i {
            let sum: f64 = (0..j).map(|k| l[i][k] * l[j][k]).sum();
            if i == j {
                let d = a[i][i] - sum;
                if d <= 0.0 || !d.is_finite() {
                    return None;
                }
                l[i][j] = d.sqrt();
            } else {
                l[i][j] = (a[i][j] - sum) / l[j][j];
            }
        }
    }
    let mut y = vec![0.0; n];
    for i in 0..n {
        let sum: f64 = (0..i).map(|k| l[i][k] * y[k]).sum();
        y[i] = (b[i] - sum) / l[i][i];
    }
    let mut x = vec![0.0; n];
    for i in (0..n).rev() {
        let sum: f64 = (i + 1..n).map(|k| l[k][i] * x[k]).sum();
        x[i] = (y[i] - sum) / l[i][i];
    }
    Some(x)
}

/// 3×3 helpers.
pub type M3 = [[f64; 3]; 3];

#[must_use]
pub fn mul3(a: &M3, b: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    m
}

#[must_use]
pub fn transpose3(a: &M3) -> M3 {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = a[j][i];
        }
    }
    m
}

#[must_use]
pub fn det3(a: &M3) -> f64 {
    a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0])
}

#[must_use]
pub fn mul3v(a: &M3, v: [f64; 3]) -> [f64; 3] {
    [
        a[0][0] * v[0] + a[0][1] * v[1] + a[0][2] * v[2],
        a[1][0] * v[0] + a[1][1] * v[1] + a[1][2] * v[2],
        a[2][0] * v[0] + a[2][1] * v[1] + a[2][2] * v[2],
    ]
}

/// Inverse of a 3×3 matrix, if not singular.
#[must_use]
pub fn inv3(a: &M3) -> Option<M3> {
    let d = det3(a);
    if d.abs() < 1e-300 || !d.is_finite() {
        return None;
    }
    let c =
        |r0: usize, c0: usize, r1: usize, c1: usize| a[r0][c0] * a[r1][c1] - a[r0][c1] * a[r1][c0];
    Some([
        [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
        [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
        [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
    ])
}

/// Rotation matrix from a Rodrigues vector (axis × angle).
#[must_use]
pub fn rodrigues(r: [f64; 3]) -> M3 {
    let theta = (r[0] * r[0] + r[1] * r[1] + r[2] * r[2]).sqrt();
    if theta < 1e-12 {
        // First-order: I + [r]×.
        return [[1.0, -r[2], r[1]], [r[2], 1.0, -r[0]], [-r[1], r[0], 1.0]];
    }
    let (x, y, z) = (r[0] / theta, r[1] / theta, r[2] / theta);
    let (s, c) = theta.sin_cos();
    let t = 1.0 - c;
    [
        [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
        [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
        [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
    ]
}

/// Rodrigues vector of a rotation matrix.
#[must_use]
pub fn rodrigues_of(m: &M3) -> [f64; 3] {
    let cos = ((m[0][0] + m[1][1] + m[2][2] - 1.0) / 2.0).clamp(-1.0, 1.0);
    let theta = cos.acos();
    if theta < 1e-9 {
        return [
            (m[2][1] - m[1][2]) / 2.0,
            (m[0][2] - m[2][0]) / 2.0,
            (m[1][0] - m[0][1]) / 2.0,
        ];
    }
    if (std::f64::consts::PI - theta) < 1e-6 {
        // Near 180°: axis from the diagonal.
        let x = ((m[0][0] + 1.0) / 2.0).max(0.0).sqrt();
        let y = ((m[1][1] + 1.0) / 2.0)
            .max(0.0)
            .sqrt()
            .copysign(m[0][1] + m[1][0]);
        let z = ((m[2][2] + 1.0) / 2.0)
            .max(0.0)
            .sqrt()
            .copysign(m[0][2] + m[2][0]);
        let n = (x * x + y * y + z * z).sqrt().max(1e-300);
        return [x / n * theta, y / n * theta, z / n * theta];
    }
    let k = theta / (2.0 * theta.sin());
    [
        (m[2][1] - m[1][2]) * k,
        (m[0][2] - m[2][0]) * k,
        (m[1][0] - m[0][1]) * k,
    ]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn eigen_decomposes_a_known_matrix() {
        let a = vec![
            vec![4.0, 1.0, 0.0],
            vec![1.0, 3.0, 1.0],
            vec![0.0, 1.0, 2.0],
        ];
        let (vals, vecs) = symmetric_eigen(&a);
        for k in 0..3 {
            let v: Vec<f64> = (0..3).map(|i| vecs[i][k]).collect();
            for i in 0..3 {
                let av: f64 = (0..3).map(|j| a[i][j] * v[j]).sum();
                assert!((av - vals[k] * v[i]).abs() < 1e-10);
            }
        }
        let trace: f64 = vals.iter().sum();
        assert!((trace - 9.0).abs() < 1e-10);
    }

    #[test]
    fn cholesky_solves_and_rejects_indefinite() {
        let a = vec![vec![4.0, 2.0], vec![2.0, 3.0]];
        let x = cholesky_solve(&a, &[2.0, 1.0]).unwrap();
        assert!((4.0 * x[0] + 2.0 * x[1] - 2.0).abs() < 1e-12);
        assert!((2.0 * x[0] + 3.0 * x[1] - 1.0).abs() < 1e-12);
        assert!(cholesky_solve(&vec![vec![1.0, 2.0], vec![2.0, 1.0]], &[0.0, 0.0]).is_none());
    }

    #[test]
    fn rodrigues_round_trips() {
        for r in [
            [0.1, -0.2, 0.3],
            [0.0, 0.0, 0.0],
            [1.5, 0.2, -0.7],
            [0.0, 3.1, 0.0],
        ] {
            let m = rodrigues(r);
            assert!((det3(&m) - 1.0).abs() < 1e-9);
            let back = rodrigues(rodrigues_of(&m));
            for i in 0..3 {
                for j in 0..3 {
                    assert!((back[i][j] - m[i][j]).abs() < 1e-6, "{r:?}");
                }
            }
        }
        let a = [[2.0, 0.0, 1.0], [1.0, 3.0, 0.0], [0.0, 1.0, 4.0]];
        let i = mul3(&a, &inv3(&a).unwrap());
        assert!((i[0][0] - 1.0).abs() < 1e-12 && i[0][1].abs() < 1e-12);
        assert!(inv3(&[[1.0, 2.0, 3.0], [2.0, 4.0, 6.0], [0.0, 0.0, 1.0]]).is_none());
    }
}
