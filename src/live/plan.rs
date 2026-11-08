//! What to generate next and where players may go. A chunk is safe to stand in when every
//! chunk the server reads around it (view distance + 3, plus one of slack) is ready.

/// A square of chunks, inclusive of `min` and `side` chunks wide.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Square {
    pub min_cx: i32,
    pub min_cz: i32,
    pub side: i32,
}

impl Square {
    pub fn contains(&self, cx: i32, cz: i32) -> bool {
        cx >= self.min_cx
            && cz >= self.min_cz
            && cx < self.min_cx + self.side
            && cz < self.min_cz + self.side
    }

    /// Centre and width in blocks, as `worldborder` takes them.
    pub fn border(&self) -> (f64, f64, f64) {
        let size = f64::from(self.side * 16);
        (
            f64::from(self.min_cx * 16) + size / 2.0,
            f64::from(self.min_cz * 16) + size / 2.0,
            size,
        )
    }

    /// Chunks between `(cx, cz)` and the nearest edge.
    pub fn inset(&self, cx: i32, cz: i32) -> i32 {
        let max = self.side - 1;
        (cx - self.min_cx)
            .min(self.min_cx + max - cx)
            .min(cz - self.min_cz)
            .min(self.min_cz + max - cz)
    }
}

/// Whether every chunk within `margin` of `(cx, cz)` is ready.
pub fn chunk_is_safe(ready: &dyn Fn(i32, i32) -> bool, cx: i32, cz: i32, margin: i32) -> bool {
    (cx - margin..=cx + margin).all(|x| (cz - margin..=cz + margin).all(|z| ready(x, z)))
}

/// The largest square of safe chunks, at most `2 * max_half + 1` wide, that holds every
/// player chunk. Among equals the one centred closest on the players wins.
pub fn safe_square(
    ready: &dyn Fn(i32, i32) -> bool,
    players: &[(i32, i32)],
    margin: i32,
    max_half: i32,
) -> Option<Square> {
    let (&(fx, fz), _) = players.split_first()?;
    let (mut min_x, mut max_x, mut min_z, mut max_z) = (fx, fx, fz, fz);
    for &(x, z) in players {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_z = min_z.min(z);
        max_z = max_z.max(z);
    }
    let spread = (max_x - min_x).max(max_z - min_z) + 1;
    let max_side = 2 * max_half + 1;
    if spread > max_side {
        return None;
    }

    // Inner area: where a square cell can be. Window: inner area plus the margin.
    let (ix0, iz0) = (max_x - max_side + 1, max_z - max_side + 1);
    let (ix1, iz1) = (min_x + max_side - 1, min_z + max_side - 1);
    let (wx0, wz0) = (ix0 - margin, iz0 - margin);
    let w = (ix1 - ix0 + 1 + 2 * margin) as usize;
    let h = (iz1 - iz0 + 1 + 2 * margin) as usize;

    let not_ready = prefix(w, h, |x, z| !ready(wx0 + x as i32, wz0 + z as i32) as u32);
    let (iw, ih) = ((ix1 - ix0 + 1) as usize, (iz1 - iz0 + 1) as usize);
    debug_assert!(ih > 0);
    let m = margin as usize;
    let side = 2 * m + 1;
    let unsafe_ = prefix(iw, ih, |x, z| {
        (sum(&not_ready, w, x, z, side, side) > 0) as u32
    });

    let centre = (
        players.iter().map(|p| f64::from(p.0)).sum::<f64>() / players.len() as f64,
        players.iter().map(|p| f64::from(p.1)).sum::<f64>() / players.len() as f64,
    );
    let best_at = |s: i32| -> Option<Square> {
        let mut best: Option<(f64, Square)> = None;
        for a in (max_x - s + 1)..=min_x {
            for b in (max_z - s + 1)..=min_z {
                let (x, z) = ((a - ix0) as usize, (b - iz0) as usize);
                if sum(&unsafe_, iw, x, z, s as usize, s as usize) != 0 {
                    continue;
                }
                let half = f64::from(s - 1) / 2.0;
                let d = (f64::from(a) + half - centre.0).powi(2)
                    + (f64::from(b) + half - centre.1).powi(2);
                if best.is_none_or(|(bd, _)| d < bd) {
                    best = Some((
                        d,
                        Square {
                            min_cx: a,
                            min_cz: b,
                            side: s,
                        },
                    ));
                }
            }
        }
        best.map(|(_, sq)| sq)
    };

    // A square that fits keeps fitting as it shrinks around the players.
    let (mut lo, mut hi) = (spread, max_side);
    let mut found = best_at(lo)?;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        match best_at(mid) {
            Some(sq) => {
                found = sq;
                lo = mid;
            }
            None => hi = mid - 1,
        }
    }
    Some(found)
}

fn prefix(w: usize, h: usize, f: impl Fn(usize, usize) -> u32) -> Vec<u32> {
    let stride = w + 1;
    let mut p = vec![0u32; stride * (h + 1)];
    for z in 0..h {
        let mut row = 0;
        for x in 0..w {
            row += f(x, z);
            p[(z + 1) * stride + x + 1] = p[z * stride + x + 1] + row;
        }
    }
    p
}

fn sum(p: &[u32], w: usize, x: usize, z: usize, dx: usize, dz: usize) -> u32 {
    let s = w + 1;
    p[(z + dz) * s + x + dx] + p[z * s + x] - p[z * s + x + dx] - p[(z + dz) * s + x]
}

/// A player as the scheduler sees them, in blocks and blocks per second.
#[derive(Clone, Copy, Debug)]
pub struct Mover {
    pub x: f64,
    pub z: f64,
    pub vx: f64,
    pub vz: f64,
}

/// Regions worth generating around the players, most urgent first.
///
/// `reach` blocks around each player and around where they will be in `lead` seconds, with
/// the regions ahead of a moving player moved up the list.
pub fn wanted_regions(movers: &[Mover], reach: f64, lead: f64) -> Vec<(i32, i32)> {
    let mut scored: std::collections::HashMap<(i32, i32), f64> = Default::default();
    for m in movers {
        let speed = m.vx.hypot(m.vz);
        let (dx, dz) = if speed > 0.5 {
            (m.vx / speed, m.vz / speed)
        } else {
            (0.0, 0.0)
        };
        let ahead_len = (speed * lead).min(2048.0);
        let steps = (ahead_len / (reach / 2.0)).ceil().max(0.0) as i32;
        for step in 0..=steps {
            let t = if steps == 0 {
                0.0
            } else {
                f64::from(step) / f64::from(steps)
            };
            let (px, pz) = (m.x + dx * ahead_len * t, m.z + dz * ahead_len * t);
            let (rx0, rx1) = (
                ((px - reach) / 512.0).floor() as i32,
                ((px + reach) / 512.0).floor() as i32,
            );
            let (rz0, rz1) = (
                ((pz - reach) / 512.0).floor() as i32,
                ((pz + reach) / 512.0).floor() as i32,
            );
            for rx in rx0..=rx1 {
                for rz in rz0..=rz1 {
                    let (cx, cz) = (f64::from(rx) * 512.0 + 256.0, f64::from(rz) * 512.0 + 256.0);
                    let (ox, oz) = (cx - m.x, cz - m.z);
                    let ahead = (ox * dx + oz * dz).max(0.0);
                    let urgency = (speed / 10.0).min(1.0);
                    let score = ox.hypot(oz) - 0.6 * ahead * urgency;
                    let e = scored.entry((rx, rz)).or_insert(f64::INFINITY);
                    *e = e.min(score);
                }
            }
        }
    }
    let mut out: Vec<((i32, i32), f64)> = scored.into_iter().collect();
    out.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.cmp(&b.0)));
    out.into_iter().map(|(r, _)| r).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_square_stays_inside_the_ready_area() {
        // Ready: chunks -20..=20 on both axes.
        let ready = |x: i32, z: i32| x.abs() <= 20 && z.abs() <= 20;
        let sq = safe_square(&ready, &[(0, 0)], 5, 64).unwrap();
        assert_eq!(sq.side, 31);
        assert_eq!((sq.min_cx, sq.min_cz), (-15, -15));
        for x in sq.min_cx..sq.min_cx + sq.side {
            for z in sq.min_cz..sq.min_cz + sq.side {
                assert!(chunk_is_safe(&ready, x, z, 5));
            }
        }
    }

    #[test]
    fn the_square_holds_the_player_off_centre() {
        let ready = |x: i32, z: i32| x.abs() <= 20 && z.abs() <= 20;
        let sq = safe_square(&ready, &[(14, -3)], 5, 64).unwrap();
        assert!(sq.contains(14, -3));
        assert_eq!(sq.side, 31);
    }

    #[test]
    fn no_square_when_the_player_is_not_safe() {
        let ready = |x: i32, z: i32| x.abs() <= 20 && z.abs() <= 20;
        assert!(safe_square(&ready, &[(18, 0)], 5, 64).is_none());
        assert!(safe_square(&ready, &[], 5, 64).is_none());
    }

    #[test]
    fn the_square_is_capped() {
        let ready = |_: i32, _: i32| true;
        let sq = safe_square(&ready, &[(3, 3), (5, 9)], 4, 10).unwrap();
        assert_eq!(sq.side, 21);
        assert!(sq.contains(3, 3) && sq.contains(5, 9));
    }

    #[test]
    fn players_too_far_apart_get_no_square() {
        let ready = |_: i32, _: i32| true;
        assert!(safe_square(&ready, &[(0, 0), (100, 0)], 4, 10).is_none());
    }

    #[test]
    fn the_border_is_in_blocks() {
        let sq = Square {
            min_cx: -2,
            min_cz: 0,
            side: 4,
        };
        assert_eq!(sq.border(), (0.0, 32.0, 64.0));
        assert_eq!(sq.inset(-2, 1), 0);
        assert_eq!(sq.inset(0, 1), 1);
    }

    #[test]
    fn regions_ahead_come_first() {
        let still = wanted_regions(
            &[Mover {
                x: 256.0,
                z: 256.0,
                vx: 0.0,
                vz: 0.0,
            }],
            600.0,
            10.0,
        );
        assert_eq!(still[0], (0, 0));
        let east = wanted_regions(
            &[Mover {
                x: 256.0,
                z: 256.0,
                vx: 30.0,
                vz: 0.0,
            }],
            600.0,
            10.0,
        );
        let pos = |r: (i32, i32)| east.iter().position(|x| *x == r).unwrap();
        assert!(pos((1, 0)) < pos((-1, 0)));
        assert!(east.contains(&(2, 0)));
    }
}
