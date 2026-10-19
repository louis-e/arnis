//! Stream-to-disk scheduling for the post-merge placements (3D models, planes, landmarks).
//!
//! Each placement runs as soon as every region it may write to has merged all its tiles, so
//! those regions are flushed during the tile loop instead of staying resident to the end.
//! Placements sharing a region still run in their original order; the ones released together
//! share none, so they commute and the result matches placing everything after the merge.

use std::collections::{HashMap, VecDeque};

type Region = (i32, i32);

pub struct DeferredPlacements {
    /// Per placement, the in-world regions it may write to.
    regions: Vec<Vec<Region>>,
    /// Per placement, how many of its regions still wait on tiles.
    waiting: Vec<usize>,
    /// Per region, the placements that may write to it, in placement order.
    queues: HashMap<Region, VecDeque<usize>>,
    placed: Vec<bool>,
}

impl DeferredPlacements {
    /// `placements` in placement order. Regions outside `world` are dropped: the editor
    /// discards writes outside the world, so nothing there needs to wait or be ordered.
    pub fn new(placements: Vec<Vec<Region>>, world: impl Fn(Region) -> bool) -> Self {
        let mut queues: HashMap<Region, VecDeque<usize>> = HashMap::new();
        let regions: Vec<Vec<Region>> = placements
            .into_iter()
            .enumerate()
            .map(|(unit, mut regions)| {
                regions.retain(|&r| world(r));
                regions.sort_unstable();
                regions.dedup();
                for &r in &regions {
                    queues.entry(r).or_default().push_back(unit);
                }
                regions
            })
            .collect();
        let waiting = regions.iter().map(Vec::len).collect();
        let placed = vec![false; regions.len()];
        Self {
            regions,
            waiting,
            queues,
            placed,
        }
    }

    /// Whether a placement still has to write to `region` before it may be flushed.
    pub fn holds(&self, region: Region) -> bool {
        self.queues.get(&region).is_some_and(|q| !q.is_empty())
    }

    /// Regions any placement may write to.
    pub fn regions(&self) -> impl Iterator<Item = &Region> {
        self.queues.keys()
    }

    pub fn regions_of(&self, unit: usize) -> &[Region] {
        &self.regions[unit]
    }

    /// Records that `region` merged its last tile. Returns the placements now free to run, as
    /// waves to run in order; a wave's placements share no region.
    pub fn complete(&mut self, region: Region) -> Vec<Vec<usize>> {
        let Some(queue) = self.queues.get(&region) else {
            return Vec::new();
        };
        for &unit in queue {
            self.waiting[unit] -= 1;
        }
        let head = queue.front().copied();
        self.release(head.into_iter().collect())
    }

    /// Everything not yet placed, in placement order. Once every tile has merged that is only
    /// placements with no region inside the world.
    pub fn drain(&mut self) -> Vec<usize> {
        self.queues.clear();
        (0..self.placed.len())
            .filter(|&unit| !std::mem::replace(&mut self.placed[unit], true))
            .collect()
    }

    fn runnable(&self, unit: usize) -> bool {
        self.waiting[unit] == 0
            && self.regions[unit]
                .iter()
                .all(|r| self.queues[r].front() == Some(&unit))
    }

    fn release(&mut self, mut candidates: Vec<usize>) -> Vec<Vec<usize>> {
        let mut waves = Vec::new();
        loop {
            candidates.sort_unstable();
            candidates.dedup();
            let wave: Vec<usize> = candidates
                .iter()
                .copied()
                .filter(|&u| self.runnable(u))
                .collect();
            if wave.is_empty() {
                return waves;
            }
            candidates.clear();
            for &unit in &wave {
                self.placed[unit] = true;
                for r in &self.regions[unit] {
                    let queue = self.queues.get_mut(r).unwrap();
                    queue.pop_front();
                    if let Some(&next) = queue.front() {
                        candidates.push(next);
                    }
                }
            }
            waves.push(wave);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sched(units: &[&[Region]]) -> DeferredPlacements {
        DeferredPlacements::new(units.iter().map(|u| u.to_vec()).collect(), |r| {
            r.0 >= 0 && r.1 >= 0
        })
    }

    #[test]
    fn a_placement_waits_for_all_its_regions() {
        let mut s = sched(&[&[(0, 0), (1, 0)]]);
        assert!(s.complete((0, 0)).is_empty());
        assert!(s.holds((0, 0)));
        assert_eq!(s.complete((1, 0)), vec![vec![0]]);
        assert!(!s.holds((0, 0)) && !s.holds((1, 0)));
    }

    #[test]
    fn overlapping_placements_keep_their_order() {
        // Unit 1 is ready first but shares (1, 0) with unit 0, which must go before it.
        let mut s = sched(&[&[(0, 0), (1, 0)], &[(1, 0), (2, 0)]]);
        assert!(s.complete((1, 0)).is_empty());
        assert!(s.complete((2, 0)).is_empty());
        assert_eq!(s.complete((0, 0)), vec![vec![0], vec![1]]);
    }

    #[test]
    fn disjoint_placements_share_a_wave() {
        let mut s = sched(&[&[(0, 0)], &[(1, 0)], &[(0, 0), (1, 0)]]);
        assert_eq!(s.complete((0, 0)), vec![vec![0]]);
        assert_eq!(s.complete((1, 0)), vec![vec![1], vec![2]]);
    }

    #[test]
    fn regions_outside_the_world_are_not_waited_on() {
        let mut s = sched(&[&[(-1, 0), (0, 0)], &[(-1, 0)]]);
        assert!(!s.holds((-1, 0)));
        assert_eq!(s.complete((0, 0)), vec![vec![0]]);
        assert_eq!(s.drain(), vec![1]);
    }

    #[test]
    fn drain_forces_what_never_completed() {
        let mut s = sched(&[&[(0, 0)], &[(0, 0), (5, 5)], &[(6, 6)]]);
        assert_eq!(s.complete((0, 0)), vec![vec![0]]);
        assert_eq!(s.drain(), vec![1, 2]);
        assert!(!s.holds((0, 0)) && !s.holds((5, 5)));
        assert!(s.drain().is_empty());
    }
}
