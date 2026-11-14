//! Player characters (the map pack's `CHAR`): binding MW2's player animations (`common.wpk` `PANM`, tracks by bone
//! name) to a character's skeleton, and choosing the animation a player plays from how it moves.
//!
//! The choice is a simplification of MW2's player anim tree (which blends legs and torso by move direction and
//! speed): one full-body clip by stance, sprint and the move direction relative to where the player faces.

use alloc::string::String;
use alloc::vec::Vec;

use wpk::view::{Clip, Track, ViewModel, player_slot, quat_to_i16};

/// The animation clips for a model whose bones are named `model_names`: each clip of `anims` (tracks indexing
/// `anim_names`) with its tracks moved to the model's bones of the same name. Tracks for bones the model lacks are
/// dropped; a track without rotation (the zero quaternion) takes the bone's bind rotation.
pub fn bind_clips(
    model: &ViewModel,
    model_names: &[String],
    anims: &ViewModel,
    anim_names: &[String],
) -> Vec<Clip> {
    let map: Vec<Option<usize>> = anim_names
        .iter()
        .map(|n| model_names.iter().position(|m| !m.is_empty() && m == n))
        .collect();
    anims
        .clips
        .iter()
        .map(|c| Clip {
            slot: c.slot,
            looping: c.looping,
            framerate: c.framerate,
            frames: c.frames,
            tracks: c
                .tracks
                .iter()
                .filter_map(|t| {
                    let bone = (*map.get(usize::from(t.bone))?)?;
                    let rot = if t.rot.len() == 1 && t.rot[0] == [0; 4] {
                        alloc::vec![quat_to_i16(model.bones[bone].local_rot)]
                    } else {
                        t.rot.clone()
                    };
                    Some(Track {
                        bone: bone as u16,
                        rot,
                        trans: t.trans.clone(),
                    })
                })
                .collect(),
        })
        .collect()
}

/// Below this horizontal speed a player stands (IW4 `PLAYER_MOVE_THRESHHOLD`-like: anims switch to the idle).
const MOVING_SPEED: f32 = 10.0;

/// The clip slot (`wpk::view::player_slot`) for a player: its stance, whether sprinting, its velocity and facing
/// yaw (degrees).
pub fn slot_for(prone: bool, crouched: bool, sprinting: bool, velocity: [f32; 3], yaw: f32) -> u8 {
    let speed = libm::sqrtf(velocity[0] * velocity[0] + velocity[1] * velocity[1]);
    let moving = speed > MOVING_SPEED;
    if prone {
        return if moving {
            player_slot::PRONE_CRAWL
        } else {
            player_slot::PRONE
        };
    }
    if crouched {
        return if moving {
            player_slot::CROUCH_RUN
        } else {
            player_slot::CROUCH
        };
    }
    if !moving {
        return player_slot::STAND;
    }
    if sprinting {
        return player_slot::SPRINT;
    }
    // The move direction relative to the facing: forward, back, left or right (the nearest quarter).
    let y = yaw.to_radians();
    let (fx, fy) = (libm::cosf(y), libm::sinf(y));
    let ahead = (velocity[0] * fx + velocity[1] * fy) / speed;
    let left = (-velocity[0] * fy + velocity[1] * fx) / speed;
    if libm::fabsf(ahead) >= libm::fabsf(left) {
        if ahead >= 0.0 {
            player_slot::RUN_FORWARD
        } else {
            player_slot::RUN_BACK
        }
    } else if left > 0.0 {
        player_slot::RUN_LEFT
    } else {
        player_slot::RUN_RIGHT
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wpk::view::Bone;

    #[test]
    fn clips_bind_by_bone_name() {
        let bone = |p: i16| Bone {
            parent: p,
            local_rot: [0.0, 0.0, 0.0, 1.0],
            ..Bone::default()
        };
        let model = ViewModel {
            bones: alloc::vec![bone(-1), bone(0)],
            ..ViewModel::default()
        };
        let names = alloc::vec![String::from("j_mainroot"), String::from("j_spine4")];
        let anims = ViewModel {
            bones: alloc::vec![bone(-1); 3],
            clips: alloc::vec![Clip {
                slot: player_slot::STAND,
                looping: true,
                framerate: 30.0,
                frames: 1,
                tracks: alloc::vec![
                    Track {
                        bone: 0,
                        rot: alloc::vec![[1, 2, 3, 4]],
                        trans: alloc::vec![],
                    },
                    Track {
                        bone: 1,
                        rot: alloc::vec![[0; 4]],
                        trans: alloc::vec![[1.0, 0.0, 0.0]],
                    },
                    Track {
                        bone: 2,
                        rot: alloc::vec![[5, 5, 5, 5]],
                        trans: alloc::vec![],
                    },
                ],
            }],
            ..ViewModel::default()
        };
        let anim_names = alloc::vec![
            String::from("j_spine4"),
            String::from("j_mainroot"),
            String::from("j_ankle_le"),
        ];
        let c = bind_clips(&model, &names, &anims, &anim_names);
        let t = &c[0].tracks;
        assert_eq!(t.len(), 2, "the bone the model lacks is dropped");
        assert_eq!((t[0].bone, t[0].rot[0]), (1, [1, 2, 3, 4]));
        assert_eq!(t[1].bone, 0);
        assert_eq!(
            t[1].rot[0],
            quat_to_i16([0.0, 0.0, 0.0, 1.0]),
            "bind rotation"
        );
    }

    #[test]
    fn the_clip_follows_stance_and_direction() {
        use player_slot::*;
        let still = [0.0; 3];
        assert_eq!(slot_for(false, false, false, still, 0.0), STAND);
        assert_eq!(
            slot_for(false, false, false, [190.0, 0.0, 0.0], 0.0),
            RUN_FORWARD
        );
        assert_eq!(
            slot_for(false, false, false, [-150.0, 0.0, 0.0], 0.0),
            RUN_BACK
        );
        assert_eq!(
            slot_for(false, false, false, [0.0, 150.0, 0.0], 0.0),
            RUN_LEFT
        );
        assert_eq!(
            slot_for(false, false, false, [0.0, -150.0, 0.0], 0.0),
            RUN_RIGHT
        );
        // Facing +y, moving +y is forward.
        assert_eq!(
            slot_for(false, false, false, [0.0, 150.0, 0.0], 90.0),
            RUN_FORWARD
        );
        assert_eq!(slot_for(false, false, true, [250.0, 0.0, 0.0], 0.0), SPRINT);
        assert_eq!(slot_for(false, true, false, still, 0.0), CROUCH);
        assert_eq!(
            slot_for(false, true, false, [80.0, 0.0, 0.0], 0.0),
            CROUCH_RUN
        );
        assert_eq!(slot_for(true, false, false, still, 0.0), PRONE);
        assert_eq!(
            slot_for(true, false, false, [30.0, 0.0, 0.0], 0.0),
            PRONE_CRAWL
        );
    }
}
