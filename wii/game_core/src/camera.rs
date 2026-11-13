//! IW4 camera to GX.
//!
//! IW4 is Z-up: yaw 0 looks along +X, +Y is to the left, positive pitch looks down (as in `docs/wii-gx.md`). GX eye
//! space is OpenGL style: +X right, +Y up, looking down −Z. The view matrix's rows are therefore the camera's right,
//! up and back (−forward) vectors, with the eye translation folded in.

use libm::{cosf, sinf, tanf};

use crate::gfx::Mtx34;

pub type Vec3 = [f32; 3];

pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
pub fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: Vec3,
    /// Degrees.
    pub yaw: f32,
    /// Degrees, positive looks down.
    pub pitch: f32,
    /// Vertical field of view in degrees.
    pub fovy: f32,
    /// Width / height of the screen.
    pub aspect: f32,
}

/// MW2's default horizontal FOV, 65°, at 4:3 is about 50.5° vertical.
pub const DEFAULT_FOVY: f32 = 50.5;
pub const ZNEAR: f32 = 4.0;
pub const ZFAR: f32 = 30000.0;

impl Camera {
    pub fn new(eye: Vec3, yaw: f32, pitch: f32) -> Camera {
        Camera {
            eye,
            yaw,
            pitch,
            fovy: DEFAULT_FOVY,
            aspect: 4.0 / 3.0,
        }
    }

    pub fn forward(&self) -> Vec3 {
        let (y, p) = (self.yaw.to_radians(), self.pitch.to_radians());
        [cosf(y) * cosf(p), sinf(y) * cosf(p), -sinf(p)]
    }
    pub fn right(&self) -> Vec3 {
        let y = self.yaw.to_radians();
        [sinf(y), -cosf(y), 0.0]
    }
    pub fn up(&self) -> Vec3 {
        cross(self.right(), self.forward())
    }

    /// World → GX eye space.
    pub fn view(&self) -> Mtx34 {
        let mut m = self.view_rotation();
        for row in &mut m {
            row[3] = -dot([row[0], row[1], row[2]], self.eye);
        }
        m
    }

    /// The view without translation (for the skybox, drawn around the eye).
    pub fn view_rotation(&self) -> Mtx34 {
        let (r, u, f) = (self.right(), self.up(), self.forward());
        [
            [r[0], r[1], r[2], 0.0],
            [u[0], u[1], u[2], 0.0],
            [-f[0], -f[1], -f[2], 0.0],
        ]
    }

    /// Conservative visibility of an axis-aligned box: false only when it is entirely behind the near plane or
    /// entirely outside one side plane of the view frustum.
    pub fn sees_box(&self, min: Vec3, max: Vec3) -> bool {
        let (r, u, f) = (self.right(), self.up(), self.forward());
        let ty = tanf(self.fovy.to_radians() / 2.0);
        let tx = ty * self.aspect;
        let mut all = [true; 5];
        for i in 0..8 {
            let c = [
                if i & 1 == 0 { min[0] } else { max[0] },
                if i & 2 == 0 { min[1] } else { max[1] },
                if i & 4 == 0 { min[2] } else { max[2] },
            ];
            let d = sub(c, self.eye);
            let (x, y, z) = (dot(d, r), dot(d, u), dot(d, f));
            let out = [z < ZNEAR, x > z * tx, x < -z * tx, y > z * ty, y < -z * ty];
            for (a, o) in all.iter_mut().zip(out) {
                *a &= o;
            }
        }
        !all.iter().any(|&a| a)
    }

    /// Distance along the view direction (for back-to-front sorting).
    pub fn depth_of(&self, p: Vec3) -> f32 {
        dot(sub(p, self.eye), self.forward())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(m: &Mtx34, p: Vec3) -> Vec3 {
        let r = |i: usize| m[i][0] * p[0] + m[i][1] * p[1] + m[i][2] * p[2] + m[i][3];
        [r(0), r(1), r(2)]
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3)
    }

    #[test]
    fn view_maps_iw4_axes_to_gx_eye_space() {
        let cam = Camera::new([100.0, 200.0, 50.0], 0.0, 0.0);
        let v = cam.view();
        // 10 units ahead (+X) -> straight down -Z.
        assert!(close(apply(&v, [110.0, 200.0, 50.0]), [0.0, 0.0, -10.0]));
        // +Y is left -> -X in eye space; +Z up -> +Y.
        assert!(close(apply(&v, [100.0, 210.0, 50.0]), [-10.0, 0.0, 0.0]));
        assert!(close(apply(&v, [100.0, 200.0, 60.0]), [0.0, 10.0, 0.0]));
        // Yaw 90 looks along +Y.
        let cam = Camera::new([0.0; 3], 90.0, 0.0);
        assert!(close(apply(&cam.view(), [0.0, 5.0, 0.0]), [0.0, 0.0, -5.0]));
        // Positive pitch looks down.
        let cam = Camera::new([0.0; 3], 0.0, 90.0);
        assert!(close(
            apply(&cam.view(), [0.0, 0.0, -5.0]),
            [0.0, 0.0, -5.0]
        ));
    }

    #[test]
    fn frustum_box_test() {
        let cam = Camera::new([0.0; 3], 0.0, 0.0);
        let unit = |c: Vec3| {
            (
                [c[0] - 1.0, c[1] - 1.0, c[2] - 1.0],
                [c[0] + 1.0, c[1] + 1.0, c[2] + 1.0],
            )
        };
        let (a, b) = unit([100.0, 0.0, 0.0]);
        assert!(cam.sees_box(a, b), "ahead");
        let (a, b) = unit([-100.0, 0.0, 0.0]);
        assert!(!cam.sees_box(a, b), "behind");
        let (a, b) = unit([100.0, 500.0, 0.0]);
        assert!(!cam.sees_box(a, b), "far left");
        let (a, b) = unit([100.0, 0.0, 500.0]);
        assert!(!cam.sees_box(a, b), "far above");
        // A huge box around the eye is always seen.
        assert!(cam.sees_box([-1e4; 3], [1e4; 3]));
        assert!(cam.depth_of([10.0, 3.0, 0.0]) > cam.depth_of([5.0, -3.0, 0.0]));
    }
}
