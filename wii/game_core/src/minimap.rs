//! MW2's HUD minimap: the map's minimap texture turned with the view (your heading is always up), your arrow in the
//! middle, enemies that fire (a ping that fades over IW4's `compassSoundPingFadeTime`) and, under a UAV, every enemy
//! as the radar sweep passes them (`compassRadarUpdateTime` sweeps, `compassRadarPingFadeTime` fades). The frame and
//! constants are IW4L's `hud_iw4` compass; the layout (where, how big) is MW2's `minimap_fullscreen` menu.

use alloc::vec::Vec;

use hud_iw4::{
    COMPASS_FRIENDLY_HEIGHT_DEFAULT, COMPASS_FRIENDLY_WIDTH_DEFAULT, COMPASS_PLAYER_HEIGHT_DEFAULT,
    COMPASS_PLAYER_WIDTH_DEFAULT, COMPASS_RADAR_PING_FADE_TIME_DEFAULT,
    COMPASS_RADAR_UPDATE_TIME_DEFAULT, COMPASS_SOUND_PING_FADE_TIME_DEFAULT, CompassMapBounds,
    compass_clamp_offset, compass_map_bounds_from_corners,
};
use wpk::records::Minimap;
use wpk::ui::Quad;

/// A corner of a textured quad with its own texture coordinates (the turned map); the shim's `mwf_ui_vert`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vert {
    pub x: f32,
    pub y: f32,
    pub s: f32,
    pub t: f32,
    pub rgba: u32,
}

/// The minimap for one frame: where it is on screen, and the view it shows.
pub struct Frame {
    bounds: CompassMapBounds,
    east: [f32; 2],
    south: [f32; 2],
    rect: [f32; 4],
    centre: [f32; 2],
    /// Screen pixels per world unit (the rect's height covers the map's `max_range`).
    scale: f32,
    player: [f32; 2],
    forward: [f32; 2],
    right: [f32; 2],
}

fn dot(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[0] + a[1] * b[1]
}

impl Frame {
    /// The minimap of `m` in screen `rect` (x, y, w, h) for a player at `player` looking along `yaw_deg`.
    pub fn new(m: &Minimap, rect: [f32; 4], player: [f32; 3], yaw_deg: f32) -> Option<Frame> {
        let bounds = compass_map_bounds_from_corners(m.northwest, m.southeast, m.north_yaw)?;
        let n = bounds.north;
        let r = yaw_deg.to_radians();
        let forward = [libm::cosf(r), libm::sinf(r)];
        Some(Frame {
            bounds,
            east: [n[1], -n[0]],
            south: [-n[0], -n[1]],
            rect,
            centre: [rect[0] + rect[2] * 0.5, rect[1] + rect[3] * 0.5],
            scale: rect[3] / m.max_range.max(1.0),
            player: [player[0], player[1]],
            forward,
            right: [forward[1], -forward[0]],
        })
    }

    /// Texture coordinates of world point `p` on the minimap image.
    fn uv(&self, p: [f32; 2]) -> [f32; 2] {
        let d = [
            p[0] - self.bounds.upper_left[0],
            p[1] - self.bounds.upper_left[1],
        ];
        [
            dot(d, self.east) / self.bounds.world_size[0],
            dot(d, self.south) / self.bounds.world_size[1],
        ]
    }

    /// The map image filling the rect, turned so the view points up.
    pub fn map_quad(&self, rgba: u32) -> [Vert; 4] {
        let [x, y, w, h] = self.rect;
        let corner = |sx: f32, sy: f32| {
            let dx = (sx - self.centre[0]) / self.scale;
            let dy = (sy - self.centre[1]) / self.scale;
            let p = [
                self.player[0] + self.right[0] * dx - self.forward[0] * dy,
                self.player[1] + self.right[1] * dx - self.forward[1] * dy,
            ];
            let [s, t] = self.uv(p);
            Vert {
                x: sx,
                y: sy,
                s,
                t,
                rgba,
            }
        };
        [
            corner(x, y),
            corner(x + w, y),
            corner(x + w, y + h),
            corner(x, y + h),
        ]
    }

    /// Where world point `p` is on screen, pulled to the rect's edge when outside it.
    pub fn to_screen(&self, p: [f32; 3]) -> [f32; 2] {
        let d = [p[0] - self.player[0], p[1] - self.player[1]];
        let off = [
            dot(d, self.right) * self.scale,
            -dot(d, self.forward) * self.scale,
        ];
        let off = compass_clamp_offset(off, [self.rect[2], self.rect[3]]);
        [self.centre[0] + off[0], self.centre[1] + off[1]]
    }

    /// An icon of `size` centred on screen point `at`.
    pub fn icon(at: [f32; 2], size: [f32; 2], texture: u16, rgba: u32) -> Quad {
        Quad {
            x: at[0] - size[0] * 0.5,
            y: at[1] - size[1] * 0.5,
            w: size[0],
            h: size[1],
            uv: [0.0, 0.0, 1.0, 1.0],
            rgba,
            texture,
        }
    }

    /// Your arrow in the middle (it points up: the map turns instead).
    pub fn player_icon(&self, texture: u16) -> Quad {
        Self::icon(
            self.centre,
            [COMPASS_PLAYER_WIDTH_DEFAULT, COMPASS_PLAYER_HEIGHT_DEFAULT],
            texture,
            0xffff_ffff,
        )
    }

    /// The size of an enemy's ping.
    pub const ENEMY_SIZE: [f32; 2] = [
        COMPASS_FRIENDLY_WIDTH_DEFAULT,
        COMPASS_FRIENDLY_HEIGHT_DEFAULT,
    ];
}

/// How long a firing ping shows, and how long a radar contact fades (ms).
pub const FIRING_PING_MS: u32 = (COMPASS_SOUND_PING_FADE_TIME_DEFAULT * 1000.0) as u32;
pub const RADAR_PING_MS: u32 = (COMPASS_RADAR_PING_FADE_TIME_DEFAULT * 1000.0) as u32;
/// One radar sweep across the map.
pub const RADAR_SWEEP_MS: u32 = (COMPASS_RADAR_UPDATE_TIME_DEFAULT * 1000.0) as u32;

/// A ping's opacity `since` ms after it was made, fading over `fade_ms` (None once gone).
pub fn ping_alpha(since: u32, fade_ms: u32) -> Option<f32> {
    (since < fade_ms).then(|| 1.0 - since as f32 / fade_ms as f32)
}

/// The UAV's radar: a sweep crosses the map west to east every [`RADAR_SWEEP_MS`] and marks each enemy it passes
/// where they stand at that moment.
#[derive(Clone, Debug, Default)]
pub struct Radar {
    /// Active until.
    pub until_ms: Option<u32>,
    start_ms: u32,
    /// Per enemy slot: where and when the sweep last marked them.
    pub contacts: Vec<Option<([f32; 3], u32)>>,
}

impl Radar {
    pub fn activate(&mut self, now: u32, duration_ms: u32) {
        if !self.active(now) {
            self.start_ms = now;
        }
        self.until_ms = Some(now.wrapping_add(duration_ms));
    }

    pub fn active(&self, now: u32) -> bool {
        self.until_ms
            .is_some_and(|u| (u.wrapping_sub(now) as i32) > 0)
    }

    /// Advance the sweep to `now` over `m`'s map: enemies (slot, position) the line passed since `then` are marked.
    pub fn sweep(&mut self, m: &Minimap, then: u32, now: u32, enemies: &[(usize, [f32; 3])]) {
        if !self.active(now) {
            return;
        }
        let Some(b) = compass_map_bounds_from_corners(m.northwest, m.southeast, m.north_yaw) else {
            return;
        };
        let east = [b.north[1], -b.north[0]];
        let phase = |t: u32| {
            (t.wrapping_sub(self.start_ms) % RADAR_SWEEP_MS) as f32 / RADAR_SWEEP_MS as f32
        };
        let (a, z) = (phase(then), phase(now));
        for &(slot, p) in enemies {
            let x = dot([p[0] - b.upper_left[0], p[1] - b.upper_left[1]], east) / b.world_size[0];
            let x = x.clamp(0.0, 1.0);
            let passed = if z >= a {
                x > a && x <= z
            } else {
                x > a || x <= z
            };
            if passed || now.wrapping_sub(then) >= RADAR_SWEEP_MS {
                if self.contacts.len() <= slot {
                    self.contacts.resize(slot + 1, None);
                }
                self.contacts[slot] = Some((p, now));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map() -> Minimap {
        // A 4000 x 4000 map, north along +x (IW4's default north yaw 0), range 2000.
        Minimap {
            texture: 0,
            northwest: [2000.0, 2000.0],
            southeast: [-2000.0, -2000.0],
            north_yaw: 0.0,
            max_range: 2000.0,
        }
    }

    #[test]
    fn the_map_turns_with_the_view() {
        let m = map();
        let rect = [6.0, 6.0, 100.0, 100.0];
        // At the centre looking north (+x): the top edge is 1000 units north, the image's top is north.
        let f = Frame::new(&m, rect, [0.0, 0.0, 0.0], 0.0).unwrap();
        let q = f.map_quad(0xffffffff);
        let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
        // Top left of the rect: 1000 north, 1000 west of the centre -> uv (0.25, 0.25).
        assert!(close(q[0].s, 0.25) && close(q[0].t, 0.25), "{:?}", q[0]);
        assert!(close(q[2].s, 0.75) && close(q[2].t, 0.75), "{:?}", q[2]);
        // Looking east (yaw -90): the top of the rect shows the east side of the map.
        let f = Frame::new(&m, rect, [0.0, 0.0, 0.0], -90.0).unwrap();
        let top_mid = {
            let q = f.map_quad(0);
            [(q[0].s + q[1].s) * 0.5, (q[0].t + q[1].t) * 0.5]
        };
        assert!(
            close(top_mid[0], 0.75) && close(top_mid[1], 0.5),
            "{top_mid:?}"
        );
        // Something 500 units ahead is 25 pixels above the centre; far away it sticks to the edge.
        let f = Frame::new(&m, rect, [0.0, 0.0, 0.0], 0.0).unwrap();
        let p = f.to_screen([500.0, 0.0, 0.0]);
        assert!(close(p[0], 56.0) && close(p[1], 31.0), "{p:?}");
        let far = f.to_screen([9000.0, 0.0, 0.0]);
        assert!(close(far[1], 6.0), "{far:?}");
    }

    #[test]
    fn the_radar_marks_enemies_as_its_sweep_passes() {
        let m = map();
        let mut r = Radar::default();
        assert!(!r.active(0));
        r.activate(1000, 30_000);
        let west = (0, [0.0, 1900.0, 0.0]);
        let east = (1, [0.0, -1900.0, 0.0]);
        // A quarter sweep: the line is at x = 0.25 of the map (west to east) and has passed the west enemy only.
        r.sweep(&m, 1000, 1000 + RADAR_SWEEP_MS / 4, &[west, east]);
        assert!(r.contacts[0].is_some());
        assert!(r.contacts.get(1).copied().flatten().is_none());
        r.sweep(
            &m,
            1000 + RADAR_SWEEP_MS / 4,
            1000 + RADAR_SWEEP_MS,
            &[west, east],
        );
        assert!(r.contacts[1].is_some());
        assert!(!r.active(31_001));
        assert_eq!(ping_alpha(0, FIRING_PING_MS), Some(1.0));
        assert_eq!(ping_alpha(FIRING_PING_MS, FIRING_PING_MS), None);
    }
}
