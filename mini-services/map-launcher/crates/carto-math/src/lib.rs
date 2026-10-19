//! Geospatial + raster math for the map-launcher (pure `std`).
//!
//! Web-Mercator projection, tile math, Cohen–Sutherland line clipping,
//! Sutherland–Hodgman polygon clipping and Douglas–Peucker simplification.

pub const TILE_SIZE: f64 = 256.0;
pub const MAX_LAT: f64 = 85.0511287798066;

pub fn deg_to_rad(d: f64) -> f64 {
    d * std::f64::consts::PI / 180.0
}

/// Degrees lon/lat -> normalized Web-Mercator coords in [0,1]^2.
pub fn lonlat_to_merc(lon: f64, lat: f64) -> (f64, f64) {
    let x = (lon + 180.0) / 360.0;
    let lat = lat.clamp(-MAX_LAT, MAX_LAT);
    let phi = deg_to_rad(lat);
    let y = 0.5 - ((std::f64::consts::FRAC_PI_4 + phi / 2.0).tan().ln()) / (2.0 * std::f64::consts::PI);
    (x, y)
}

/// Normalized Web-Mercator -> degrees lon/lat.
pub fn merc_to_lonlat(x: f64, y: f64) -> (f64, f64) {
    let lon = x * 360.0 - 180.0;
    let n = std::f64::consts::PI * (1.0 - 2.0 * y);
    let lat = n.sinh().atan().to_degrees();
    (lon, lat)
}

/// Tile (z, x, y) -> lon/lat bbox [west, south, east, north].
pub fn tile_bbox(z: u8, x: u32, y: u32) -> [f64; 4] {
    let n = f64::from(1u32 << z);
    let west = x as f64 / n * 360.0 - 180.0;
    let east = (x as f64 + 1.0) / n * 360.0 - 180.0;
    let north = merc_to_lonlat(0.0, y as f64 / n).1;
    let south = merc_to_lonlat(0.0, (y as f64 + 1.0) / n).1;
    [west, south, east, north]
}

/// World pixel size at zoom z (one 256px tile).
pub fn world_px(z: u8) -> f64 {
    TILE_SIZE * f64::from(1u32 << z)
}

/// Project lon/lat degrees to global pixel coords at zoom z.
pub fn lonlat_to_px(lon: f64, lat: f64, z: u8) -> (f64, f64) {
    let (mx, my) = lonlat_to_merc(lon, lat);
    let s = world_px(z);
    (mx * s, my * s)
}

/// Lon/lat bbox -> inclusive tile index ranges (x0..=x1, y0..=y1) at zoom z.
pub fn bbox_to_tile_range(z: u8, bbox: &[f64; 4]) -> (u32, u32, u32, u32) {
    let n = 1u32 << z;
    let max = n - 1;
    let (mx0, my0) = lonlat_to_merc(bbox[0], bbox[3]);
    let (mx1, my1) = lonlat_to_merc(bbox[2], bbox[1]);
    let x0 = ((mx0 * n as f64).floor() as i64).clamp(0, i64::from(max)) as u32;
    let x1 = ((mx1 * n as f64).floor() as i64).clamp(0, i64::from(max)) as u32;
    let y0 = ((my0 * n as f64).floor() as i64).clamp(0, i64::from(max)) as u32;
    let y1 = ((my1 * n as f64).floor() as i64).clamp(0, i64::from(max)) as u32;
    (x0, x1, y0.min(y1), y1.max(y0))
}

/// Haversine great-circle distance in meters.
pub fn haversine_m(lon1: f64, lat1: f64, lon2: f64, lat2: f64) -> f64 {
    const R: f64 = 6_371_008.8;
    let (phi1, phi2) = (deg_to_rad(lat1), deg_to_rad(lat2));
    let dphi = phi2 - phi1;
    let dl = deg_to_rad(lon2 - lon1);
    let a = (dphi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * R * a.sqrt()
}

/* --------------------------------------------------------------------- */
/* Cohen–Sutherland line clipping                                          */
/* --------------------------------------------------------------------- */

const INSIDE: u8 = 0;
const LEFT: u8 = 1;
const RIGHT: u8 = 2;
const BOTTOM: u8 = 4;
const TOP: u8 = 8;

/// Axis-aligned clip rect: [xmin, ymin, xmax, ymax].
#[derive(Clone, Copy, Debug)]
pub struct ClipRect {
    pub xmin: f64,
    pub ymin: f64,
    pub xmax: f64,
    pub ymax: f64,
}

impl ClipRect {
    pub fn new(xmin: f64, ymin: f64, xmax: f64, ymax: f64) -> Self {
        Self { xmin, ymin, xmax, ymax }
    }

    fn code(&self, x: f64, y: f64) -> u8 {
        let mut c = INSIDE;
        if x < self.xmin {
            c |= LEFT;
        } else if x > self.xmax {
            c |= RIGHT;
        }
        if y < self.ymin {
            c |= BOTTOM;
        } else if y > self.ymax {
            c |= TOP;
        }
        c
    }

    /// Clip segment (x1,y1)-(x2,y2); returns clipped endpoints or None.
    pub fn clip_line(&self, x1: f64, y1: f64, x2: f64, y2: f64) -> Option<(f64, f64, f64, f64)> {
        let (mut x1, mut y1, mut x2, mut y2) = (x1, y1, x2, y2);
        let mut c1 = self.code(x1, y1);
        let mut c2 = self.code(x2, y2);
        for _ in 0..8 {
            if c1 | c2 == 0 {
                return Some((x1, y1, x2, y2));
            }
            if c1 & c2 != 0 {
                return None;
            }
            let c_out = if c1 != 0 { c1 } else { c2 };
            let (x, y) = {
                let dy = y2 - y1;
                let dx = x2 - x1;
                if c_out & TOP != 0 {
                    (x1 + dx * (self.ymax - y1) / dy, self.ymax)
                } else if c_out & BOTTOM != 0 {
                    (x1 + dx * (self.ymin - y1) / dy, self.ymin)
                } else if c_out & RIGHT != 0 {
                    (self.xmax, y1 + dy * (self.xmax - x1) / dx)
                } else {
                    (self.xmin, y1 + dy * (self.xmin - x1) / dx)
                }
            };
            if c_out == c1 {
                x1 = x;
                y1 = y;
                c1 = self.code(x1, y1);
            } else {
                x2 = x;
                y2 = y;
                c2 = self.code(x2, y2);
            }
        }
        None
    }

    /// Sutherland–Hodgman polygon clip (classic 4-edge passes).
    /// `pts` is a flat [x0,y0,x1,y1,...] ring; returns a flat clipped ring.
    pub fn clip_polygon(&self, pts: &[f64]) -> Vec<f64> {
        let n = pts.len() / 2;
        if n < 3 {
            return Vec::new();
        }
        let mut poly: Vec<(f64, f64)> = pts.chunks(2).map(|c| (c[0], c[1])).collect();

        fn clip_pass(
            poly: &[(f64, f64)],
            inside: &dyn Fn(f64, f64) -> bool,
            isect: &dyn Fn(f64, f64, f64, f64) -> (f64, f64),
        ) -> Vec<(f64, f64)> {
            let mut out: Vec<(f64, f64)> = Vec::with_capacity(poly.len() + 2);
            let n = poly.len();
            for i in 0..n {
                let (cx, cy) = poly[i];
                let (px, py) = poly[(i + n - 1) % n];
                let cur_in = inside(cx, cy);
                let prev_in = inside(px, py);
                if cur_in {
                    if !prev_in {
                        out.push(isect(px, py, cx, cy));
                    }
                    out.push((cx, cy));
                } else if prev_in {
                    out.push(isect(px, py, cx, cy));
                }
            }
            out
        }

        let (xmin, ymin, xmax, ymax) = (self.xmin, self.ymin, self.xmax, self.ymax);
        // left: x >= xmin
        poly = clip_pass(&poly, &|x, _| x >= xmin, &|x1, y1, x2, y2| {
            (xmin, y1 + (y2 - y1) * (xmin - x1) / (x2 - x1))
        });
        // right: x <= xmax
        poly = clip_pass(&poly, &|x, _| x <= xmax, &|x1, y1, x2, y2| {
            (xmax, y1 + (y2 - y1) * (xmax - x1) / (x2 - x1))
        });
        // bottom: y >= ymin
        poly = clip_pass(&poly, &|_, y| y >= ymin, &|x1, y1, x2, y2| {
            (x1 + (x2 - x1) * (ymin - y1) / (y2 - y1), ymin)
        });
        // top: y <= ymax
        poly = clip_pass(&poly, &|_, y| y <= ymax, &|x1, y1, x2, y2| {
            (x1 + (x2 - x1) * (ymax - y1) / (y2 - y1), ymax)
        });
        poly.into_iter().flat_map(|(x, y)| [x, y]).collect()
    }
}

/// Douglas–Peucker polyline simplification on flat [x,y,...] coords.
pub fn simplify(points: &[f64], epsilon: f64) -> Vec<f64> {
    let n = points.len() / 2;
    if n < 3 || epsilon <= 0.0 {
        return points.to_vec();
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        if b <= a + 1 {
            continue;
        }
        let (ax, ay) = (points[a * 2], points[a * 2 + 1]);
        let (bx, by) = (points[b * 2], points[b * 2 + 1]);
        let (dx, dy) = (bx - ax, by - ay);
        let len2 = dx * dx + dy * dy;
        let (mut dmax, mut imax) = (0.0f64, a);
        for i in a + 1..b {
            let (px, py) = (points[i * 2], points[i * 2 + 1]);
            let d = if len2 > 0.0 {
                ((dx * (py - ay) - dy * (px - ax)) / len2.sqrt()).abs()
            } else {
                (px - ax).hypot(py - ay)
            };
            if d > dmax {
                dmax = d;
                imax = i;
            }
        }
        if dmax > epsilon {
            keep[imax] = true;
            stack.push((a, imax));
            stack.push((imax, b));
        }
    }
    let mut out = Vec::new();
    for i in 0..n {
        if keep[i] {
            out.push(points[i * 2]);
            out.push(points[i * 2 + 1]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn merc_roundtrip() {
        for &(lon, lat) in &[(35.74, -6.16), (0.0, 0.0), (-120.5, 44.2)] {
            let (x, y) = lonlat_to_merc(lon, lat);
            let (lo, la) = merc_to_lonlat(x, y);
            assert!(close(lon, lo) && close(lat, la), "{lon},{lat} -> {lo},{la}");
        }
    }

    #[test]
    fn merc_bounds() {
        let (x, y) = lonlat_to_merc(0.0, 0.0);
        assert!(close(x, 0.5) && close(y, 0.5));
        let (x, y) = lonlat_to_merc(-180.0, MAX_LAT);
        assert!(close(x, 0.0) && close(y, 0.0));
    }

    #[test]
    fn z0_tile_is_world() {
        let b = tile_bbox(0, 0, 0);
        assert!(close(b[0], -180.0) && close(b[2], 180.0));
        assert!(b[3] > 85.0 && b[1] < -85.0);
    }

    #[test]
    fn dodoma_tile_z11() {
        // Dodoma ~ (35.74, -6.16): z11 -> x = (35.74+180)/360*2048 ≈ 1228, y ≈ 1203
        let (x, y) = {
            let (mx, my) = lonlat_to_merc(35.74, -6.16);
            (
                (mx * (1u32 << 11) as f64).floor() as u32,
                (my * (1u32 << 11) as f64).floor() as u32,
            )
        };
        let b = tile_bbox(11, x, y);
        assert!(b[0] <= 35.74 && b[2] >= 35.74 && b[1] <= -6.16 && b[3] >= -6.16);
    }

    #[test]
    fn clip_line_cases() {
        let r = ClipRect::new(0.0, 0.0, 10.0, 10.0);
        assert!(r.clip_line(-5.0, 5.0, 15.0, 5.0).is_some());
        assert!(r.clip_line(-5.0, -5.0, -1.0, -1.0).is_none());
        let (x1, y1, x2, y2) = r.clip_line(-5.0, 5.0, 15.0, 5.0).unwrap();
        assert!(close(x1, 0.0) && close(x2, 10.0) && close(y1, 5.0) && close(y2, 5.0));
        assert!(r.clip_line(1.0, 1.0, 9.0, 9.0).is_some());
    }

    #[test]
    fn clip_polygon_square() {
        let r = ClipRect::new(2.0, 2.0, 8.0, 8.0);
        let sq = [0.0, 0.0, 10.0, 0.0, 10.0, 10.0, 0.0, 10.0];
        let out = r.clip_polygon(&sq);
        assert!(out.len() >= 8);
        for v in out.chunks(2) {
            assert!(v[0] >= 2.0 - 1e-9 && v[0] <= 8.0 + 1e-9);
            assert!(v[1] >= 2.0 - 1e-9 && v[1] <= 8.0 + 1e-9);
        }
    }

    #[test]
    fn simplify_keeps_shape() {
        let pts = [0.0, 0.0, 1.0, 0.1, 2.0, 0.0];
        let out = simplify(&pts, 0.05);
        assert_eq!(out.len(), 6);
        let out = simplify(&pts, 1.0);
        assert_eq!(out.len(), 4);
    }

    #[test]
    fn haversine_dodoma_city() {
        let d = haversine_m(35.7395, -6.1662, 35.7495, -6.1762);
        assert!(d > 1200.0 && d < 1700.0, "d={d}");
    }
}
