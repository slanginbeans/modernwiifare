//! First-person model posing and CPU skinning (`wpk::view::ViewModel`).
//!
//! Pose (IW4 rules, as `anim_iw4::compose`): a bone's local rotation is the clip's (else its bind rotation), its
//! local translation is bind + the clip's delta; world = parent world × local. Skinning: posed world × inverse bind,
//! weighted over up to four bones. The model is shown with `tag_view` on the camera: [`eye_from_model`] maps model
//! space to GX eye space independently of where the player is.

use alloc::vec::Vec;

use wpk::view::{Clip, ViewModel, quat_from_i16};

use crate::gfx::Mtx34;

type Quat = [f32; 4];

pub const IDENTITY: Mtx34 = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
];

fn qnorm(q: Quat) -> Quat {
    let l = libm::sqrtf(q.iter().map(|v| v * v).sum::<f32>()).max(1e-12);
    q.map(|v| v / l)
}

/// Normalized linear interpolation (shortest arc).
fn nlerp(a: Quat, b: Quat, t: f32) -> Quat {
    let d: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let s = if d < 0.0 { -1.0 } else { 1.0 };
    qnorm(core::array::from_fn(|i| a[i] + (b[i] * s - a[i]) * t))
}

/// Rigid transform from rotation (x, y, z, w) and translation.
pub fn mtx(q: Quat, t: [f32; 3]) -> Mtx34 {
    let [x, y, z, w] = qnorm(q);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
            t[0],
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
            t[1],
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
            t[2],
        ],
    ]
}

pub fn mul(a: &Mtx34, b: &Mtx34) -> Mtx34 {
    core::array::from_fn(|r| {
        core::array::from_fn(|c| {
            let t = if c == 3 { a[r][3] } else { 0.0 };
            a[r][0] * b[0][c] + a[r][1] * b[1][c] + a[r][2] * b[2][c] + t
        })
    })
}

/// Inverse of a rigid transform.
pub fn inverse_rigid(m: &Mtx34) -> Mtx34 {
    core::array::from_fn(|r| {
        let row = [m[0][r], m[1][r], m[2][r]];
        [
            row[0],
            row[1],
            row[2],
            -(row[0] * m[0][3] + row[1] * m[1][3] + row[2] * m[2][3]),
        ]
    })
}

pub fn apply(m: &Mtx34, p: [f32; 3]) -> [f32; 3] {
    core::array::from_fn(|r| m[r][0] * p[0] + m[r][1] * p[1] + m[r][2] * p[2] + m[r][3])
}

/// Sample a clip at `time_s`: (frame a, frame b, blend).
fn frames(c: &Clip, time_s: f32) -> (usize, usize, f32) {
    let n = usize::from(c.frames.max(1));
    if n == 1 {
        return (0, 0, 0.0);
    }
    let last = (n - 1) as f32;
    let mut f = time_s * c.framerate;
    if c.looping {
        f %= last;
        if f < 0.0 {
            f += last;
        }
    } else {
        f = f.clamp(0.0, last);
    }
    let a = libm::floorf(f) as usize;
    let b = (a + 1).min(n - 1);
    (a, b, f - a as f32)
}

/// How far through `c` the pose at `time_s` is, 0..1 (wrapping for a looping clip), as IW4 notetrack times are.
pub fn fraction(c: &Clip, time_s: f32) -> f32 {
    let (a, _, t) = frames(c, time_s);
    let last = f32::from(c.frames.saturating_sub(1));
    if last <= 0.0 {
        1.0
    } else {
        (a as f32 + t) / last
    }
}

/// Duration of a clip in seconds.
pub fn duration(c: &Clip) -> f32 {
    f32::from(c.frames.saturating_sub(1)) / c.framerate.max(1e-3)
}

/// World (model space) transforms of every bone with `clip` at `time_s` (bind pose when None).
pub fn pose(vm: &ViewModel, clip: Option<(&Clip, f32)>, out: &mut Vec<Mtx34>) {
    let n = vm.bones.len();
    let mut rot: Vec<Quat> = vm.bones.iter().map(|b| b.local_rot).collect();
    let mut delta: Vec<[f32; 3]> = alloc::vec![[0.0; 3]; n];
    if let Some((c, t)) = clip {
        let (a, b, w) = frames(c, t);
        for tr in &c.tracks {
            let bone = usize::from(tr.bone);
            let at = |i: usize, len: usize| if len == 1 { 0 } else { i.min(len - 1) };
            let (qa, qb) = (
                quat_from_i16(tr.rot[at(a, tr.rot.len())]),
                quat_from_i16(tr.rot[at(b, tr.rot.len())]),
            );
            rot[bone] = nlerp(qa, qb, w);
            if !tr.trans.is_empty() {
                let (ta, tb) = (
                    tr.trans[at(a, tr.trans.len())],
                    tr.trans[at(b, tr.trans.len())],
                );
                delta[bone] = core::array::from_fn(|k| ta[k] + (tb[k] - ta[k]) * w);
            }
        }
    }
    out.clear();
    for (i, bone) in vm.bones.iter().enumerate() {
        let t = core::array::from_fn(|k| bone.local_trans[k] + delta[i][k]);
        let local = mtx(rot[i], t);
        let world = if bone.parent >= 0 {
            mul(&out[bone.parent as usize], &local)
        } else {
            local
        };
        out.push(world);
    }
}

/// Skin matrices (posed world × inverse bind) from posed world transforms.
pub fn skin_matrices(vm: &ViewModel, world: &[Mtx34], out: &mut Vec<Mtx34>) {
    out.clear();
    out.extend(vm.bones.iter().zip(world).map(|(b, w)| mul(w, &b.inv_bind)));
}

/// Skinned model-space positions of every vertex.
pub fn skin(vm: &ViewModel, skin: &[Mtx34], out: &mut Vec<[f32; 3]>) {
    out.clear();
    for v in &vm.verts {
        if v.weights[0] == 255 {
            out.push(apply(&skin[usize::from(v.bones[0])], v.pos));
            continue;
        }
        let mut p = [0.0f32; 3];
        for k in 0..4 {
            if v.weights[k] == 0 {
                continue;
            }
            let w = f32::from(v.weights[k]) / 255.0;
            let q = apply(&skin[usize::from(v.bones[k])], v.pos);
            for j in 0..3 {
                p[j] += q[j] * w;
            }
        }
        out.push(p);
    }
}

/// Model space → GX eye space with `tag_view` (posed world `view`) on the camera. IW4 axes (x forward, y left,
/// z up) become eye axes (x right, y up, looking down −z).
pub fn eye_from_model(view: &Mtx34) -> Mtx34 {
    const IW_TO_EYE: Mtx34 = [
        [0.0, -1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [-1.0, 0.0, 0.0, 0.0],
    ];
    mul(&IW_TO_EYE, &inverse_rigid(view))
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use wpk::view::{Bone, SkinVertex, Track, quat_to_i16};

    fn two_bones() -> ViewModel {
        let id = [0.0, 0.0, 0.0, 1.0];
        let bind1 = mtx(id, [10.0, 0.0, 0.0]);
        ViewModel {
            bones: vec![
                Bone {
                    parent: -1,
                    local_rot: id,
                    local_trans: [0.0; 3],
                    inv_bind: IDENTITY,
                },
                Bone {
                    parent: 0,
                    local_rot: id,
                    local_trans: [10.0, 0.0, 0.0],
                    inv_bind: inverse_rigid(&bind1),
                },
            ],
            view_bone: 0,
            flash_bone: u16::MAX,
            verts: vec![SkinVertex {
                pos: [12.0, 0.0, 0.0],
                bones: [1, 0, 0, 0],
                weights: [255, 0, 0, 0],
                ..SkinVertex::default()
            }],
            indices: vec![],
            surfaces: vec![],
            clips: vec![],
        }
    }

    #[test]
    fn bind_pose_skins_to_itself_and_rotation_moves_children() {
        let mut vm = two_bones();
        let (mut world, mut sk, mut pos) = (Vec::new(), Vec::new(), Vec::new());
        pose(&vm, None, &mut world);
        skin_matrices(&vm, &world, &mut sk);
        skin(&vm, &sk, &mut pos);
        assert!((pos[0][0] - 12.0).abs() < 1e-4 && pos[0][1].abs() < 1e-4);
        // Rotate the root 90 degrees about Z over a 2-frame clip: the vertex swings from +x to +y.
        let h = core::f32::consts::FRAC_1_SQRT_2;
        vm.clips.push(Clip {
            slot: 1,
            looping: false,
            framerate: 10.0,
            frames: 2,
            tracks: vec![Track {
                bone: 0,
                rot: vec![
                    quat_to_i16([0.0, 0.0, 0.0, 1.0]),
                    quat_to_i16([0.0, 0.0, h, h]),
                ],
                trans: vec![],
            }],
        });
        let c = vm.clips[0].clone();
        pose(&vm, Some((&c, 0.1)), &mut world);
        skin_matrices(&vm, &world, &mut sk);
        skin(&vm, &sk, &mut pos);
        assert!(
            pos[0][0].abs() < 1e-2 && (pos[0][1] - 12.0).abs() < 1e-2,
            "{:?}",
            pos[0]
        );
        // Half way: 45 degrees.
        pose(&vm, Some((&c, 0.05)), &mut world);
        skin_matrices(&vm, &world, &mut sk);
        skin(&vm, &sk, &mut pos);
        assert!((pos[0][0] - pos[0][1]).abs() < 1e-2, "{:?}", pos[0]);
    }

    #[test]
    fn tag_view_on_the_camera_looks_down_minus_z() {
        // A point 10 units in front of tag_view (IW x forward) is 10 units down -z in eye space; left (+y) is -x.
        let view = mtx([0.0, 0.0, 0.0, 1.0], [5.0, 0.0, 0.0]);
        let m = eye_from_model(&view);
        let p = apply(&m, [15.0, 2.0, 3.0]);
        assert!(
            (p[0] + 2.0).abs() < 1e-5 && (p[1] - 3.0).abs() < 1e-5 && (p[2] + 10.0).abs() < 1e-5,
            "{p:?}"
        );
    }
}
