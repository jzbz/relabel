//! An 8-bit coverage buffer, and the quarter-turn index math it shares with
//! [`ModuleGrid`](crate::code2d::ModuleGrid).
//!
//! Its own module because the canvas blends it and the text path fills it, and
//! the canvas must not depend on text. Turning it never recomputes a value:
//! bytes only move, so a quarter-turned label is exactly the upright one on
//! its side.

use crate::document::Rot;

/// 0 is untouched, 255 is full ink. Row-major, no padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Coverage {
    pub w: u32,
    pub h: u32,
    pub data: Vec<u8>,
}

impl Coverage {
    pub fn new(w: u32, h: u32) -> Coverage {
        Coverage {
            w,
            h,
            data: vec![0; w as usize * h as usize],
        }
    }

    /// The value at a dot, or 0 outside the buffer.
    #[inline]
    pub fn at(&self, x: u32, y: u32) -> u8 {
        if x >= self.w || y >= self.h {
            return 0;
        }
        self.data
            .get(y as usize * self.w as usize + x as usize)
            .copied()
            .unwrap_or(0)
    }

    /// Turned clockwise by the rotation, quantised to a quarter turn: the same
    /// convention as `ModuleGrid::turned` and `BitPlane::rotate_cw`.
    pub fn turned(&self, rot: Rot) -> Coverage {
        let quarters = match rot.quantised() {
            Rot::D90 => 1,
            Rot::D180 => 2,
            Rot::D270 => 3,
            _ => 0,
        };
        let mut c = self.clone();
        for _ in 0..quarters {
            c = c.quarter_cw();
        }
        c
    }

    /// `dst(x, y) = src(x = y, y = h - 1 - x)`.
    fn quarter_cw(&self) -> Coverage {
        let (w, h) = (self.h, self.w);
        let mut data = Vec::with_capacity(self.data.len());
        for y in 0..h {
            for x in 0..w {
                data.push(self.at(y, self.h - 1 - x));
            }
        }
        Coverage { w, h, data }
    }

    /// A `w` x `h` rectangle whose top-left is at `(x0, y0)` in this buffer,
    /// zero wherever it falls outside.
    pub fn crop(&self, x0: i64, y0: i64, w: u32, h: u32) -> Coverage {
        let mut out = Coverage::new(w, h);
        for y in 0..h {
            for x in 0..w {
                let (sx, sy) = (x0 + i64::from(x), y0 + i64::from(y));
                if sx >= 0 && sy >= 0 && sx < i64::from(self.w) && sy < i64::from(self.h) {
                    out.data[y as usize * w as usize + x as usize] = self.at(sx as u32, sy as u32);
                }
            }
        }
        out
    }

    /// Dots with any coverage at all.
    pub fn inked(&self) -> usize {
        self.data.iter().filter(|&&c| c > 0).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::code2d::ModuleGrid;

    /// Turned exactly as a module grid turns, cell for cell.
    #[test]
    fn coverage_turns_like_a_module_grid() {
        let (w, h) = (7u32, 5u32);
        let mut s = 0x2545_f491u32;
        let dark: Vec<bool> = (0..w * h)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 17;
                s ^= s << 5;
                s & 1 == 1
            })
            .collect();
        let grid = ModuleGrid::new(w, h, 0, dark.clone());
        let cov = Coverage {
            w,
            h,
            data: dark.iter().map(|&d| if d { 255 } else { 0 }).collect(),
        };
        for rot in [Rot::D0, Rot::D90, Rot::D180, Rot::D270] {
            let (g, c) = (grid.turned(rot), cov.turned(rot));
            assert_eq!((g.w, g.h), (c.w, c.h));
            for y in 0..g.h {
                for x in 0..g.w {
                    assert_eq!(g.is_dark(x, y), c.at(x, y) == 255, "{rot:?} ({x}, {y})");
                }
            }
        }
    }

    #[test]
    fn a_crop_outside_the_buffer_is_empty() {
        let mut c = Coverage::new(3, 3);
        c.data[4] = 200;
        assert_eq!(c.crop(1, 1, 1, 1).data, vec![200]);
        assert_eq!(c.crop(-5, -5, 2, 2).data, vec![0; 4]);
        assert_eq!(c.crop(0, 0, 5, 5).inked(), 1);
    }
}
