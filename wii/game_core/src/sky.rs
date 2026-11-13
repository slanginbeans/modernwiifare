//! Skybox quads. The pack holds the six faces of IW4's sky cube in D3D order (+X, −X, +Y, −Y, +Z, −Z), and IW4 samples
//! that cube with the world-space view ray, so each face is drawn as a quad on a unit cube around the eye with the
//! texcoords D3D's cube lookup gives its corners.

/// Corners `[x, y, z, s, t]` of face `f` on a cube of half-size `r` around the eye.
pub fn face_quad(f: usize, r: f32) -> [[f32; 5]; 4] {
    // For each face: the major axis value and how (s, t) follow from (x, y, z) on that face (D3D cube rules):
    // +X: s = -z, t = -y   -X: s = z, t = -y   +Y: s = x, t = z   -Y: s = x, t = -z   +Z: s = x, t = -y   -Z: s = -x, t = -y
    let corners = [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)];
    let mut out = [[0.0f32; 5]; 4];
    for (i, &(a, b)) in corners.iter().enumerate() {
        let p: [f32; 3] = match f {
            0 => [1.0, a, b],
            1 => [-1.0, a, b],
            2 => [a, 1.0, b],
            3 => [a, -1.0, b],
            4 => [a, b, 1.0],
            _ => [a, b, -1.0],
        };
        let (sc, tc) = match f {
            0 => (-p[2], -p[1]),
            1 => (p[2], -p[1]),
            2 => (p[0], p[2]),
            3 => (p[0], -p[2]),
            4 => (p[0], -p[1]),
            _ => (-p[0], -p[1]),
        };
        out[i] = [
            p[0] * r,
            p[1] * r,
            p[2] * r,
            (sc + 1.0) * 0.5,
            (tc + 1.0) * 0.5,
        ];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The same D3D lookup the converter's reference renderer uses, by direction.
    fn lookup(d: [f32; 3]) -> (usize, f32, f32) {
        let [x, y, z] = d;
        let (ax, ay, az) = (x.abs(), y.abs(), z.abs());
        let (face, sc, tc, ma) = if ax >= ay && ax >= az {
            if x > 0.0 {
                (0, -z, -y, ax)
            } else {
                (1, z, -y, ax)
            }
        } else if ay >= az {
            if y > 0.0 {
                (2, x, z, ay)
            } else {
                (3, x, -z, ay)
            }
        } else if z > 0.0 {
            (4, x, -y, az)
        } else {
            (5, -x, -y, az)
        };
        (face, (sc / ma + 1.0) * 0.5, (tc / ma + 1.0) * 0.5)
    }

    #[test]
    fn quads_match_the_cube_lookup() {
        for f in 0..6 {
            let q = face_quad(f, 1.0);
            // Points slightly inside each corner, towards the face centre, look up the same face and texcoords.
            let centre = [
                (q[0][0] + q[2][0]) / 2.0,
                (q[0][1] + q[2][1]) / 2.0,
                (q[0][2] + q[2][2]) / 2.0,
            ];
            for c in q {
                let p = [
                    c[0] * 0.9 + centre[0] * 0.1,
                    c[1] * 0.9 + centre[1] * 0.1,
                    c[2] * 0.9 + centre[2] * 0.1,
                ];
                let (face, s, t) = lookup(p);
                assert_eq!(face, f);
                let es = c[3] * 0.9 + 0.5 * 0.1;
                let et = c[4] * 0.9 + 0.5 * 0.1;
                assert!(
                    (s - es).abs() < 1e-4 && (t - et).abs() < 1e-4,
                    "face {f}: {s},{t} vs {es},{et}"
                );
            }
        }
    }
}
