use crate::ui::d2d::{D2dEngine, Rect};
use std::f32::consts::PI;

#[inline]
fn project_3d(x: f32, y: f32, z: f32, yaw: f32, pitch: f32, cx: f32, cy: f32, scale: f32) -> (f32, f32, f32) {
    let cyaw = yaw.cos();
    let syaw = yaw.sin();
    let cpitch = pitch.cos();
    let spitch = pitch.sin();

    let x1 = x * cyaw + z * syaw;
    let z1 = -x * syaw + z * cyaw;
    let y2 = y * cpitch - z1 * spitch;
    let z2 = y * spitch + z1 * cpitch;

    let persp = 3.2 / (3.2 - z2).max(0.8);
    (cx + x1 * scale * persp, cy + y2 * scale * persp, z2)
}

#[inline]
fn splat_dot(buf: &mut [u8], w: usize, h: usize, px: f32, py: f32, rad: f32, r: u8, g: u8, b: u8, a: f32) {
    let a = a.clamp(0.0, 1.0);
    if a <= 0.004 {
        return;
    }
    let min_x = (px - rad - 0.5).floor().max(0.0) as isize;
    let max_x = (px + rad + 0.5).ceil().min((w - 1) as f32) as isize;
    let min_y = (py - rad - 0.5).floor().max(0.0) as isize;
    let max_y = (py + rad + 0.5).ceil().min((h - 1) as f32) as isize;

    for yy in min_y..=max_y {
        let dy = yy as f32 + 0.5 - py;
        for xx in min_x..=max_x {
            let dx = xx as f32 + 0.5 - px;
            let d = (dx * dx + dy * dy).sqrt();
            let cov = (rad + 0.5 - d).clamp(0.0, 1.0);
            if cov <= 0.0 {
                continue;
            }
            let src_a = (a * cov).clamp(0.0, 1.0);
            let idx = (yy as usize * w + xx as usize) * 4;
            // Premultiplied Over blend onto BGRA buffer
            let dst_b = buf[idx] as f32 / 255.0;
            let dst_g = buf[idx + 1] as f32 / 255.0;
            let dst_r = buf[idx + 2] as f32 / 255.0;
            let dst_a = buf[idx + 3] as f32 / 255.0;

            let sb = (b as f32 / 255.0) * src_a;
            let sg = (g as f32 / 255.0) * src_a;
            let sr = (r as f32 / 255.0) * src_a;
            let inv = 1.0 - src_a;

            let out_b = (sb + dst_b * inv).clamp(0.0, 1.0);
            let out_g = (sg + dst_g * inv).clamp(0.0, 1.0);
            let out_r = (sr + dst_r * inv).clamp(0.0, 1.0);
            let out_a = (src_a + dst_a * inv).clamp(0.0, 1.0);

            buf[idx] = (out_b * 255.0).round() as u8;
            buf[idx + 1] = (out_g * 255.0).round() as u8;
            buf[idx + 2] = (out_r * 255.0).round() as u8;
            buf[idx + 3] = (out_a * 255.0).round() as u8;
        }
    }
}

pub struct ParticleBuffers {
    co_buf: Vec<u8>,
    ha_buf: Vec<u8>,
    focus_buf: Vec<u8>,
}

impl ParticleBuffers {
    pub fn new() -> Self {
        Self {
            co_buf: vec![0u8; 64 * 64 * 4],
            ha_buf: vec![0u8; 64 * 64 * 4],
            focus_buf: vec![0u8; 96 * 96 * 4],
        }
    }

    /// `cO`: 64x64 3D toroidal dot-ring (11 bands x 44 segs = 484 dots), `project_3d(0, 0.3, 32, 32, 1)`.
    /// Clockwise from 12-o'clock with `fill` 0..=1 (`alpha *= 0.16` unelapsed). Uses `rgb(255,95,46)` when `warning` is true.
    pub fn draw_quota_ring(
        &mut self,
        d2d: &mut D2dEngine,
        slot_id: &str,
        rect: Rect,
        fill_0_to_1: f32,
        warning: bool,
        accent_rgb: [u8; 3],
        frame_tick: u64,
    ) {
        self.co_buf.fill(0);
        let fill = fill_0_to_1.clamp(0.0, 1.0);
        let bands = 11usize;
        let segs = 44usize;
        let major_r = 21.0f32;
        let minor_r = 4.2f32;

        let (cr, cg, cb) = if warning {
            (255u8, 95u8, 46u8)
        } else {
            (accent_rgb[0], accent_rgb[1], accent_rgb[2])
        };

        for s in 0..segs {
            // Clockwise starting at 12-o'clock: theta = -PI/2 .. 3*PI/2
            let frac = s as f32 / segs as f32;
            let theta = -PI * 0.5 + frac * 2.0 * PI;
            let active = frac <= fill;

            for b_idx in 0..bands {
                let phi = (b_idx as f32 / bands as f32) * 2.0 * PI;
                let r_ring = major_r + minor_r * phi.cos();
                let x = r_ring * theta.cos();
                let y = r_ring * theta.sin();
                let z = minor_r * phi.sin();

                // Normalize around center ~ radius 25
                let (px, py, z2) = project_3d(x / 25.2, y / 25.2, z / 25.2, 0.0, 0.30, 32.0, 32.0, 23.0);
                let depth_alpha = (0.42 + 0.58 * ((z2 + 0.25) / 0.5)).clamp(0.20, 1.0);
                let mut alpha = depth_alpha * 0.90;
                if !active {
                    alpha *= 0.16;
                }
                let dot_r = if active { 1.05 } else { 0.88 };
                splat_dot(&mut self.co_buf, 64, 64, px, py, dot_r, cr, cg, cb, alpha);
            }
        }

        d2d.update_dynamic_bgra(slot_id, 64, 64, frame_tick, &self.co_buf);
        d2d.draw_dynamic_slot(slot_id, rect, 1.0);
    }

    /// `hA`: 64x64 3D 9-ring rotating sphere lattice (`project_3d(0.55*t, 0.35, 32, 32, 28.16)`)
    /// with 3-lobe sinusoidal liquid waterline (`rgb(255,95,46)` when `>= 80%`).
    pub fn draw_liquid_sphere(
        &mut self,
        d2d: &mut D2dEngine,
        slot_id: &str,
        rect: Rect,
        pct_0_to_100: f32,
        normal_rgb: [u8; 3],
        t_sec: f32,
        frame_tick: u64,
    ) {
        self.ha_buf.fill(0);
        let pct = pct_0_to_100.clamp(0.0, 100.0);
        let fill_frac = pct / 100.0;
        let (cr, cg, cb) = if pct >= 80.0 {
            (255u8, 95u8, 46u8)
        } else {
            (normal_rgb[0], normal_rgb[1], normal_rgb[2])
        };

        let yaw = 0.55 * t_sec;
        let pitch = 0.35f32;
        let rings = 9usize;

        for r_i in 0..rings {
            // Latitude from south (+1) to north (-1) excluding poles
            let lat_t = (r_i as f32 + 1.0) / (rings as f32 + 1.0);
            let lat = (lat_t - 0.5) * PI; // -PI/2 .. +PI/2
            let y = lat.sin();
            let ring_r = lat.cos();
            let pts_on_ring = ((ring_r * 26.0).round() as usize).max(8);

            for j in 0..pts_on_ring {
                let lon = (j as f32 / pts_on_ring as f32) * 2.0 * PI;
                let x = ring_r * lon.cos();
                let z = ring_r * lon.sin();

                let (px, py, z2) = project_3d(x, y, z, yaw, pitch, 32.0, 32.0, 28.16);
                let depth_alpha = (0.32 + 0.68 * ((z2 + 1.0) * 0.5)).clamp(0.18, 1.0);

                // 3-lobe sinusoidal liquid wave threshold (y in screen coords: +1 bottom, -1 top)
                let wave = 0.09 * (lon * 3.0 + t_sec * 2.8).sin()
                    + 0.05 * (lat * 2.0 - t_sec * 1.9).cos();
                let water_y = 1.0 - 2.0 * fill_frac + wave;
                let submerged = y >= water_y;

                if submerged {
                    splat_dot(
                        &mut self.ha_buf,
                        64,
                        64,
                        px,
                        py,
                        1.25,
                        cr,
                        cg,
                        cb,
                        depth_alpha * 0.96,
                    );
                } else {
                    splat_dot(
                        &mut self.ha_buf,
                        64,
                        64,
                        px,
                        py,
                        0.95,
                        230,
                        234,
                        245,
                        depth_alpha * 0.22,
                    );
                }
            }
        }

        d2d.update_dynamic_bgra(slot_id, 64, 64, frame_tick, &self.ha_buf);
        d2d.draw_dynamic_slot(slot_id, rect, 1.0);
    }

    /// Tauri Foto 2 flat face-on undulating radial dot-burst ring centered behind Companion.
    /// Freezes smoothly when `running` is false (paused state).
    pub fn draw_focus_ring(
        &mut self,
        d2d: &mut D2dEngine,
        slot_id: &str,
        rect: Rect,
        progress_0_to_1: f32,
        accent_rgb: [u8; 3],
        t_sec: f32,
        running: bool,
        frame_tick: u64,
    ) {
        let _ = project_3d;
        self.focus_buf.fill(0);
        let prog = progress_0_to_1.clamp(0.0, 1.0);
        let t = if running { t_sec } else { 0.0 };

        let spokes = 44usize;
        let lanes = 4usize;
        let (ar, ag, ab) = (accent_rgb[0], accent_rgb[1], accent_rgb[2]);
        let mix_w = |c: u8, w: f32| -> u8 {
            ((c as f32) * (1.0 - w) + 255.0 * w).clamp(0.0, 255.0).round() as u8
        };

        for lane_idx in 0..lanes {
            let lane_f = lane_idx as f32 / (lanes - 1) as f32;
            let base_rad = 32.0 + lane_f * 7.2;
            for s in 0..spokes {
                let ang = (s as f32 / spokes as f32) * 2.0 * PI;
                let wave = 1.0
                    + 0.065 * (3.0 * ang - 2.3 * t + 0.22 * lane_idx as f32).sin()
                    + 0.03 * (5.0 * ang + 1.5 * t - 0.14 * lane_idx as f32).sin();
                let rad = (base_rad * wave).clamp(29.5, 44.5);
                let px = 48.0 + rad * ang.cos();
                let py = 48.0 + rad * ang.sin();
                let clock_frac = ((px - 48.0).atan2(48.0 - py) / (2.0 * PI) + 1.0) % 1.0;
                let elapsed = clock_frac <= prog;

                let envelope = 1.0 - 0.42 * lane_f;
                let (w_mix, dot_r, alpha) = if elapsed {
                    (
                        0.28 * (1.0 - lane_f * 0.45),
                        0.80 + 0.36 * envelope,
                        (0.48 + 0.46 * envelope).clamp(0.20, 0.94),
                    )
                } else {
                    (
                        0.38,
                        0.62 + 0.18 * envelope,
                        (0.11 + 0.10 * envelope).clamp(0.06, 0.22),
                    )
                };

                splat_dot(
                    &mut self.focus_buf,
                    96,
                    96,
                    px,
                    py,
                    dot_r,
                    mix_w(ar, w_mix),
                    mix_w(ag, w_mix),
                    mix_w(ab, w_mix),
                    alpha,
                );
            }
        }

        d2d.update_dynamic_bgra(slot_id, 96, 96, frame_tick, &self.focus_buf);
        d2d.draw_dynamic_slot(slot_id, rect, 1.0);
    }
}
