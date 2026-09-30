use crate::Coordinate;
use std::collections::hash_map::DefaultHasher;
use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};

/// Represents a distinct phase of expansion in the digital universe.
pub type RingLevel = u32;

/// The central pillar of the universe.
pub struct Monolith {
    /// The current outermost edge of our expansion.
    pub current_ring: RingLevel,

    /// The chronological ledger of all structural coordinates, grouped by the ring they were forged in.
    /// A BTreeMap ensures chronological, deterministic ordering. If we need to rewind, we drop outer rings.
    pub expansion_ledger: BTreeMap<RingLevel, Vec<Coordinate>>,
}

impl Monolith {
    /// Genesis. The spark that creates the center of the universe.
    pub fn genesis() -> Self {
        let mut ledger = BTreeMap::new();
        // Ring 0 is the Monolith itself.
        ledger.insert(0, vec![0]);

        Self {
            current_ring: 0,
            expansion_ledger: ledger,
        }
    }

    /// Expand the universe by forging a new ring.
    pub fn expand(&mut self) -> RingLevel {
        self.current_ring += 1;
        self.expansion_ledger.insert(self.current_ring, Vec::new());
        self.current_ring
    }

    /// The safety mechanism. If a ring collapses or becomes corrupted,
    /// we can shear off the outer layers and return the universe to a known stable ring.
    pub fn collapse_to_ring(&mut self, stable_ring: RingLevel) {
        if stable_ring >= self.current_ring {
            return; // Already stable.
        }

        // Shear off the outer rings
        self.expansion_ledger.retain(|&ring, _| ring <= stable_ring);
        self.current_ring = stable_ring;
    }

    /// Deterministic hash of a ring's `Vec<Coordinate>`
    pub fn ring_hash(&self, ring: RingLevel) -> u64 {
        let mut hasher = DefaultHasher::new();
        if let Some(coords) = self.expansion_ledger.get(&ring) {
            coords.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Fold the per-ring hashes into one root
    pub fn merkle_root(&self) -> u64 {
        let mut hasher = DefaultHasher::new();
        for ring in self.expansion_ledger.keys() {
            let h = self.ring_hash(*ring);
            ring.hash(&mut hasher);
            h.hash(&mut hasher);
        }
        hasher.finish()
    }

    /// Return the rings whose hashes differ
    pub fn diverging_rings(&self, their_ring_hashes: &BTreeMap<RingLevel, u64>) -> Vec<RingLevel> {
        let mut diverging = Vec::new();
        let max_our_ring = self.current_ring;
        let max_their_ring = their_ring_hashes.keys().copied().max().unwrap_or(0);
        let max_ring = std::cmp::max(max_our_ring, max_their_ring);

        for ring in 0..=max_ring {
            let our_hash = self.ring_hash(ring);
            // Default hasher for an empty/missing ring
            let empty_hash = {
                let h = DefaultHasher::new();
                h.finish()
            };
            let their_hash = their_ring_hashes.get(&ring).copied().unwrap_or(empty_hash);

            if our_hash != their_hash {
                diverging.push(ring);
            }
        }
        diverging
    }

    /// Add a coordinate to the current outermost ring.
    pub fn add_coordinate(&mut self, coord: Coordinate) {
        let cur = self.current_ring;
        if let Some(coords) = self.expansion_ledger.get_mut(&cur) {
            coords.push(coord);
        }
    }
}
