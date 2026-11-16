//! World draw submission on a real map pack, measured on the host (ignored: needs the user's converted pack).
//!
//! `MWF_BENCH_PACK=<path to a map .wpk> cargo test -p game_core --release --test world_bench -- --ignored --nocapture`
//!
//! From every spawn, looking eight ways, it draws the world through a recording renderer and prints per view the
//! calls the renderer saw (matrix loads, array binds, full state changes, tint changes, display lists) and the host
//! time, plus a hash of what was drawn. The hash covers, per display list in order, the GX state it ran under (draw
//! state, tint, vertex layout, the array base and the position matrix): changes that only drop redundant calls keep
//! it, anything that changes the picture or the order does not. `MWF_BENCH_EXPECT=<hash>` checks it.

use std::sync::OnceLock;
use std::time::Instant;

use dpvs_iw4::WalkScratch;
use game_core::camera::Camera;
use game_core::gfx::{DrawState, Gfx, Mtx34, TexDesc};
use game_core::world::{VisData, World};

#[derive(Default)]
struct Counts {
    mtx: u64,
    arrays: u64,
    state: u64,
    tint: u64,
    desc: u64,
    dl: u64,
}

/// A renderer that keeps the GX state the shim would hold and hashes it at each display list.
#[derive(Default)]
struct Rec {
    n: Counts,
    mtx: Mtx34,
    arrays: usize,
    stride: u8,
    state: Option<DrawState>,
    desc: u8,
    hash: u64,
    textures: i32,
    /// The pack in memory: pointers are hashed as offsets from it.
    base: usize,
    /// Hash the display lists (off for the timed repeats).
    hashing: bool,
}

fn mix(h: &mut u64, v: u64) {
    // FNV-1a over the bytes of v.
    for b in v.to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x100_0000_01b3);
    }
}

impl Gfx for Rec {
    fn tex_create(&mut self, _: &TexDesc) -> i32 {
        self.textures += 1;
        self.textures - 1
    }
    fn frame_begin(&mut self, view: &Mtx34, _: f32, _: f32, _: f32, _: u32) {
        self.mtx = *view;
        self.state = None;
        self.arrays = 0;
    }
    fn world_arrays(&mut self, v: *const u8, stride: u8) {
        self.n.arrays += 1;
        self.arrays = v as usize;
        self.stride = stride;
    }
    fn world_state(&mut self, s: &DrawState) {
        self.n.state += 1;
        self.state = Some(*s);
    }
    fn world_tint(&mut self, s: &DrawState) {
        self.n.tint += 1;
        let l = self.state.expect("tint before a state");
        assert_eq!(
            DrawState { tint: s.tint, ..l },
            *s,
            "tint-only change changed more"
        );
        self.state = Some(*s);
    }
    fn pos_matrix(&mut self, m: &Mtx34) {
        self.n.mtx += 1;
        self.mtx = *m;
    }
    fn world_desc(&mut self, flags: u8) {
        if flags != self.desc {
            self.n.desc += 1;
        }
        self.desc = flags;
    }
    fn call_dl(&mut self, dl: &[u8]) {
        self.n.dl += 1;
        let s = self.state.expect("display list before a state");
        if !self.hashing {
            return;
        }
        let mut h = self.hash;
        mix(&mut h, (dl.as_ptr() as usize - self.base) as u64);
        mix(&mut h, dl.len() as u64);
        mix(&mut h, (self.arrays - self.base) as u64);
        mix(&mut h, u64::from(self.stride));
        mix(&mut h, u64::from(self.desc));
        mix(&mut h, s.tex as u64);
        mix(&mut h, s.lightmap as u64);
        mix(
            &mut h,
            u64::from(s.blend) | u64::from(s.alpha_ref) << 8 | u64::from(s.cull) << 16,
        );
        mix(&mut h, u64::from(s.tint));
        // The matrix to 1/64 unit: a different but equivalent float sum must not change the hash.
        for v in self.mtx.iter().flatten() {
            mix(&mut h, (v * 64.0).round() as i64 as u64);
        }
        self.hash = h;
    }
    fn sky_face(&mut self, _: &Mtx34, _: &[[f32; 5]; 4], _: i32) {}
    fn hud_rects(&mut self, _: &[game_core::hud::Rect], _: u32) {}
    fn frame_end(&mut self) {}
}

fn host_us() -> u32 {
    static T0: OnceLock<Instant> = OnceLock::new();
    T0.get_or_init(Instant::now).elapsed().as_micros() as u32
}

#[test]
#[ignore]
fn world_bench() {
    let Ok(path) = std::env::var("MWF_BENCH_PACK") else {
        eprintln!("MWF_BENCH_PACK not set: nothing to measure");
        return;
    };
    let bytes = std::fs::read(&path).expect("read pack");
    let vis = VisData::from_pack(&bytes);
    let mut rec = Rec {
        base: bytes.as_ptr() as usize,
        hashing: true,
        ..Rec::default()
    };
    let mut world = World::load(&bytes, vis.as_ref(), &mut rec).expect("load");
    let mut scratch = Box::new(WalkScratch::new());
    let spawns: Vec<_> = world.spawns.clone();
    let mut total = Counts::default();
    let mut tris = 0u64;
    let mut worst = (0u64, 0usize);
    let mut heavy: Vec<(u64, usize)> = Vec::new();
    // Per view: instances drawn, instances culled, batches culled.
    let mut culling = [0u64; 3];
    let mut views = 0u64;
    let reps = 5;
    let mut best_ns = u128::MAX;
    world.clock = Some(host_us);
    let mut stage = [0u64; 4];
    for rep in 0..reps {
        let t0 = Instant::now();
        for (si, s) in spawns.iter().enumerate() {
            for k in 0..8 {
                let eye = [s.origin[0], s.origin[1], s.origin[2] + 60.0];
                let cam = Camera::new(eye, s.yaw + k as f32 * 45.0, 0.0);
                let before = rec.n.dl;
                let st = world.draw(&mut rec, &cam, &mut scratch);
                let c = world.cost;
                if rep > 0 {
                    for (a, b) in
                        stage
                            .iter_mut()
                            .zip([c.vis_us, c.cull_us, c.sort_us, c.submit_us])
                    {
                        *a += u64::from(b);
                    }
                }
                if rep == 0 {
                    culling[0] += u64::from(st.instances);
                    culling[1] += u64::from(st.instances_culled);
                    culling[2] += u64::from(st.culled);
                    views += 1;
                    tris += u64::from(st.triangles);
                    heavy.push((rec.n.dl - before, si * 8 + k));
                    if rec.n.dl - before > worst.0 {
                        worst = (rec.n.dl - before, si * 8 + k);
                    }
                }
            }
        }
        if rep > 0 {
            best_ns = best_ns.min(t0.elapsed().as_nanos());
        }
        rec.hashing = false;
        if rep == 0 {
            total = std::mem::take(&mut rec.n);
        }
    }
    let per = |v: u64| v as f64 / views as f64;
    println!(
        "{}: {views} views, per view: dl {:.0} mtx {:.0} arrays {:.0} state {:.0} tint {:.0} desc {:.0} tris {:.0}; \
         worst view {} draws (#{}); host {:.1} us/view (vis {:.1} cull {:.1} sort {:.1} submit {:.1}); hash {:016x}",
        world.name,
        per(total.dl),
        per(total.mtx),
        per(total.arrays),
        per(total.state),
        per(total.tint),
        per(total.desc),
        per(tris),
        worst.0,
        worst.1,
        best_ns as f64 / 1000.0 / views as f64,
        stage[0] as f64 / (views * (reps - 1)) as f64,
        stage[1] as f64 / (views * (reps - 1)) as f64,
        stage[2] as f64 / (views * (reps - 1)) as f64,
        stage[3] as f64 / (views * (reps - 1)) as f64,
        rec.hash,
    );
    println!(
        "  instances {} (drawn {:.0}, culled {:.0}), batches culled {:.0}",
        world.instance_count(),
        per(culling[0]),
        per(culling[1]),
        per(culling[2])
    );
    // The heaviest views as autoplay `warp` lines (eye 60 above the spawn; warp drops onto the floor).
    heavy.sort_by(|a, b| b.cmp(a));
    for &(n, v) in heavy.iter().take(4) {
        let s = &spawns[v / 8];
        let yaw = (s.yaw + (v % 8) as f32 * 45.0).rem_euclid(360.0);
        println!(
            "  {n} draws: warp {:.0} {:.0} {:.0} {:.0}",
            s.origin[0], s.origin[1], s.origin[2], yaw
        );
    }
    if let Ok(e) = std::env::var("MWF_BENCH_EXPECT") {
        assert_eq!(
            format!("{:016x}", rec.hash),
            e.trim(),
            "the picture changed"
        );
    }
}
