//! Free-fly camera on the GameCube pad (until collision and IW4L movement are in): main stick moves, C-stick looks,
//! L/R analog triggers go down/up, B held moves fast.

use crate::camera::Camera;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PadInput {
    /// Sticks in −128..127 (as libogc reports them), up = positive.
    pub sx: i8,
    pub sy: i8,
    pub cx: i8,
    pub cy: i8,
    /// Analog triggers 0..255.
    pub l: u8,
    pub r: u8,
    pub fast: bool,
}

/// Units per second (IW4 units are inches; 190 is MW2's base run speed).
pub const SPEED: f32 = 320.0;
pub const FAST: f32 = 4.0;
/// Degrees per second at full C-stick deflection.
pub const YAW_RATE: f32 = 150.0;
pub const PITCH_RATE: f32 = 110.0;
/// Sticks rest a few counts off centre.
pub const DEADZONE: i32 = 12;

fn axis(v: i8) -> f32 {
    let v = i32::from(v);
    if v.abs() <= DEADZONE {
        0.0
    } else {
        let s = (v.abs() - DEADZONE) as f32 / (127 - DEADZONE) as f32;
        s.min(1.0) * v.signum() as f32
    }
}

pub fn update(cam: &mut Camera, input: &PadInput, dt: f32) {
    cam.yaw -= axis(input.cx) * YAW_RATE * dt;
    cam.pitch = (cam.pitch - axis(input.cy) * PITCH_RATE * dt).clamp(-85.0, 85.0);
    if cam.yaw < 0.0 {
        cam.yaw += 360.0;
    } else if cam.yaw >= 360.0 {
        cam.yaw -= 360.0;
    }
    let speed = SPEED * if input.fast { FAST } else { 1.0 } * dt;
    let (f, r) = (cam.forward(), cam.right());
    let (mf, mr) = (axis(input.sy), axis(input.sx));
    let vertical = (f32::from(input.r) - f32::from(input.l)) / 255.0;
    for k in 0..3 {
        cam.eye[k] += (f[k] * mf + r[k] * mr) * speed;
    }
    cam.eye[2] += vertical * speed;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sticks_move_and_look() {
        let mut cam = Camera::new([0.0; 3], 0.0, 0.0);
        update(
            &mut cam,
            &PadInput {
                sy: 127,
                ..Default::default()
            },
            1.0,
        );
        assert!((cam.eye[0] - SPEED).abs() < 1e-3 && cam.eye[1].abs() < 1e-3);
        // Inside the dead zone nothing happens.
        let before = cam;
        update(
            &mut cam,
            &PadInput {
                sx: 5,
                cx: -5,
                ..Default::default()
            },
            1.0,
        );
        assert_eq!(cam, before);
        // C-stick right turns right (yaw decreases), pitch is clamped.
        update(
            &mut cam,
            &PadInput {
                cx: 127,
                cy: -127,
                ..Default::default()
            },
            1.0,
        );
        assert!((cam.yaw - (360.0 - YAW_RATE)).abs() < 1e-3);
        assert!((cam.pitch - 85.0).abs() < 1e-3);
        // Triggers fly up and down.
        let z = cam.eye[2];
        update(
            &mut cam,
            &PadInput {
                r: 255,
                ..Default::default()
            },
            0.5,
        );
        assert!((cam.eye[2] - z - SPEED * 0.5).abs() < 1e-3);
    }
}
