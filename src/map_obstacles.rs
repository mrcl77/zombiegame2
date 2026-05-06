//! Spatial-grid–accelerated obstacle collision used by everything in the
//! sim that needs to know "is this point inside a wall / prop?" — bullets,
//! zombies, throwables, the player.
//!
//! Lives in its own module (vs. inline in `map.rs`) because it's
//! standalone, hot-path code that's easier to reason about and test in
//! isolation than buried at line ~120 of a 4500-line file.  Re-exported
//! from `map.rs` so existing call sites (`use crate::map::MapObstacles`)
//! keep working unchanged.

use bevy::prelude::*;

use crate::map::{MAP_HEIGHT, MAP_WIDTH};

/// Extra Y range *below* the surface map covered by the spatial grid — the
/// metro / underground level lives here.  Bumping this is the only knob
/// needed to host obstacles outside the original map bounds; player and
/// camera clamps reference the same constant via `world_min_y()`.
pub const UNDERGROUND_EXTENT_Y: f32 = 3000.0;

/// Lowest Y the obstacle grid (and player clamp) extends to.  Above this
/// value all obstacle positions are accepted into the grid.
#[inline]
pub fn world_min_y() -> f32 {
    -MAP_HEIGHT * 0.5 - UNDERGROUND_EXTENT_Y
}

/// Total Y extent covered by the grid — surface map + underground.
#[inline]
fn grid_height_px() -> f32 {
    MAP_HEIGHT + UNDERGROUND_EXTENT_Y
}

#[derive(Clone, Copy)]
pub enum ObstacleShape {
    Circle(f32),
    Rect(Vec2),
}

#[derive(Clone, Copy)]
pub struct Obstacle {
    pub pos: Vec2,
    pub shape: ObstacleShape,
}

/// Side length of one spatial-grid cell, in world px.  Picked to comfortably
/// hold the largest "small" obstacle (props ≤32 px radius) inside one cell
/// while still covering big building rects in only ~12 cells.
pub const OBSTACLE_GRID_CELL: f32 = 128.0;

#[derive(Default)]
struct ObstacleGrid {
    cells: Vec<Vec<u32>>,
    cols: usize,
    rows: usize,
}

#[inline]
fn obstacle_aabb(o: &Obstacle) -> (Vec2, Vec2) {
    match o.shape {
        ObstacleShape::Circle(r) => (
            Vec2::new(o.pos.x - r, o.pos.y - r),
            Vec2::new(o.pos.x + r, o.pos.y + r),
        ),
        ObstacleShape::Rect(half) => (o.pos - half, o.pos + half),
    }
}

impl ObstacleGrid {
    #[inline]
    fn world_to_cell(p: Vec2) -> (i32, i32) {
        // Y is offset by the full grid height (surface + underground) so
        // obstacles below the original map bounds still land at non-negative
        // cell indices.  X uses the unchanged map-width offset.
        let cx = ((p.x + MAP_WIDTH * 0.5) / OBSTACLE_GRID_CELL).floor() as i32;
        let cy = ((p.y - world_min_y()) / OBSTACLE_GRID_CELL).floor() as i32;
        (cx, cy)
    }

    fn rebuild(&mut self, list: &[Obstacle]) {
        self.cols = ((MAP_WIDTH / OBSTACLE_GRID_CELL).ceil() as usize) + 1;
        self.rows = ((grid_height_px() / OBSTACLE_GRID_CELL).ceil() as usize) + 1;
        let total = self.cols * self.rows;
        self.cells.clear();
        self.cells.resize_with(total, Vec::new);
        for (i, o) in list.iter().enumerate() {
            // Skip zero-radius "removed" obstacles to keep cells lean.
            if matches!(o.shape, ObstacleShape::Circle(r) if r <= 0.0) {
                continue;
            }
            let (min, max) = obstacle_aabb(o);
            let (c0, r0) = Self::world_to_cell(min);
            let (c1, r1) = Self::world_to_cell(max);
            let cs = c0.max(0);
            let ce = c1.min(self.cols as i32 - 1);
            let rs = r0.max(0);
            let re = r1.min(self.rows as i32 - 1);
            for r in rs..=re {
                for c in cs..=ce {
                    self.cells[r as usize * self.cols + c as usize].push(i as u32);
                }
            }
        }
    }
}

#[derive(Resource, Default)]
pub struct MapObstacles {
    pub list: Vec<Obstacle>,
    grid: ObstacleGrid,
}

#[inline]
fn resolve_one(o: &Obstacle, pos: &mut Vec2, own_radius: f32) {
    match o.shape {
        ObstacleShape::Circle(r) => {
            if r <= 0.0 {
                return;
            }
            let delta = *pos - o.pos;
            let min_dist = r + own_radius;
            let dist_sq = delta.length_squared();
            if dist_sq < min_dist * min_dist {
                if dist_sq > 0.0001 {
                    let dist = dist_sq.sqrt();
                    *pos += delta / dist * (min_dist - dist);
                } else {
                    *pos += Vec2::new(min_dist, 0.0);
                }
            }
        }
        ObstacleShape::Rect(half) => {
            let delta = *pos - o.pos;
            let clamped = Vec2::new(
                delta.x.clamp(-half.x, half.x),
                delta.y.clamp(-half.y, half.y),
            );
            let closest = o.pos + clamped;
            let diff = *pos - closest;
            let dist_sq = diff.length_squared();
            if dist_sq < own_radius * own_radius {
                if dist_sq > 0.0001 {
                    let dist = dist_sq.sqrt();
                    *pos = closest + diff / dist * own_radius;
                } else {
                    let dx_left = delta.x + half.x;
                    let dx_right = half.x - delta.x;
                    let dy_bot = delta.y + half.y;
                    let dy_top = half.y - delta.y;
                    let min_x = dx_left.min(dx_right);
                    let min_y = dy_bot.min(dy_top);
                    if min_x < min_y {
                        if dx_left < dx_right {
                            pos.x = o.pos.x - half.x - own_radius;
                        } else {
                            pos.x = o.pos.x + half.x + own_radius;
                        }
                    } else if dy_bot < dy_top {
                        pos.y = o.pos.y - half.y - own_radius;
                    } else {
                        pos.y = o.pos.y + half.y + own_radius;
                    }
                }
            }
        }
    }
}

#[inline]
fn hits_one(o: &Obstacle, pos: Vec2, radius: f32) -> bool {
    match o.shape {
        ObstacleShape::Circle(r) => {
            if r <= 0.0 {
                return false;
            }
            let min_d = r + radius;
            pos.distance_squared(o.pos) < min_d * min_d
        }
        ObstacleShape::Rect(half) => {
            let delta = pos - o.pos;
            let clamped = Vec2::new(
                delta.x.clamp(-half.x, half.x),
                delta.y.clamp(-half.y, half.y),
            );
            let closest = o.pos + clamped;
            pos.distance_squared(closest) < radius * radius
        }
    }
}

impl MapObstacles {
    /// Re-bin every obstacle into the spatial grid.  Cheap (~O(N × cells_per_obstacle)
    /// — typical run is well under a millisecond even for ~1000 entries).
    /// Call after any mutation that adds/removes entries, or whose shape
    /// AABB changes.  Pure shape→Circle(0) transitions don't need a rebuild
    /// (the grid query short-circuits via `hits_one`/`resolve_one` instead).
    pub fn rebuild_grid(&mut self) {
        self.grid.rebuild(&self.list);
    }

    pub fn resolve(&self, pos: &mut Vec2, own_radius: f32) {
        // Fallback: empty grid (during initial load before rebuild) — scan all.
        if self.grid.cells.is_empty() {
            for o in &self.list {
                resolve_one(o, pos, own_radius);
            }
            return;
        }
        // Resolve is idempotent: scanning the same obstacle twice is harmless
        // (the second pass sees the post-resolve position and is a no-op),
        // so we don't need to deduplicate across overlapping cells.
        let lo = Vec2::new(pos.x - own_radius, pos.y - own_radius);
        let hi = Vec2::new(pos.x + own_radius, pos.y + own_radius);
        let (c0, r0) = ObstacleGrid::world_to_cell(lo);
        let (c1, r1) = ObstacleGrid::world_to_cell(hi);
        let cs = c0.max(0) as usize;
        let ce = (c1.min(self.grid.cols as i32 - 1)).max(0) as usize;
        let rs = r0.max(0) as usize;
        let re = (r1.min(self.grid.rows as i32 - 1)).max(0) as usize;
        for r in rs..=re {
            let row_off = r * self.grid.cols;
            for c in cs..=ce {
                let cell = &self.grid.cells[row_off + c];
                for &idx in cell {
                    // SAFETY: indices are populated from list iteration so
                    // they're always in-bounds.  We only mutate `shape`
                    // post-build, never resize the list during lookup.
                    let o = unsafe { self.list.get_unchecked(idx as usize) };
                    resolve_one(o, pos, own_radius);
                }
            }
        }
    }

    pub fn remove_at(&mut self, pos: Vec2) {
        self.list.retain(|o| o.pos.distance_squared(pos) > 4.0);
        self.rebuild_grid();
    }

    /// Cheap intersection test: true if a circle of `radius` centred at `pos`
    /// overlaps any obstacle in the list.  Used by bullets, zombies and
    /// throwables.
    pub fn hits(&self, pos: Vec2, radius: f32) -> bool {
        if self.grid.cells.is_empty() {
            for o in &self.list {
                if hits_one(o, pos, radius) {
                    return true;
                }
            }
            return false;
        }
        let lo = Vec2::new(pos.x - radius, pos.y - radius);
        let hi = Vec2::new(pos.x + radius, pos.y + radius);
        let (c0, r0) = ObstacleGrid::world_to_cell(lo);
        let (c1, r1) = ObstacleGrid::world_to_cell(hi);
        let cs = c0.max(0) as usize;
        let ce = (c1.min(self.grid.cols as i32 - 1)).max(0) as usize;
        let rs = r0.max(0) as usize;
        let re = (r1.min(self.grid.rows as i32 - 1)).max(0) as usize;
        for r in rs..=re {
            let row_off = r * self.grid.cols;
            for c in cs..=ce {
                let cell = &self.grid.cells[row_off + c];
                for &idx in cell {
                    let o = unsafe { self.list.get_unchecked(idx as usize) };
                    if hits_one(o, pos, radius) {
                        return true;
                    }
                }
            }
        }
        false
    }
}
