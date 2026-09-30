//! The Data Cell: The Universal Shipping Container
//!
//! This defines the atomic building block of the Terraforma network.
//! Whether it holds a medical record, a genomic sequence, or a market signal,
//! it is packed into a DataCell. The grid only sees the exterior of the cell.

use crate::Coordinate;

/// A strict 32-byte array representing a 'Tooth' for the Kinematic Drive.
/// This is the exterior label of the shipping container, optimized for SIMD search.
pub type Tooth = [u8; 32];

/// The structural classification of the Cell's payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CellKind {
    Document = 0,
    Executable = 1,
    Signal = 2,
    Proprietary = 3,
    Diamond = 4,
    Memory = 5,
    Observation = 6,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Replication {
    Replicate,
    Local,
}

impl CellKind {
    pub fn replication(&self) -> Replication {
        match self {
            CellKind::Memory | CellKind::Diamond => Replication::Replicate,
            _ => Replication::Local,
        }
    }

    pub fn decay_rate(&self) -> f32 {
        match self {
            CellKind::Memory => 0.995,
            CellKind::Executable => 0.98,
            CellKind::Diamond => 0.98,
            CellKind::Observation => 0.97,
            CellKind::Document => 0.92,
            CellKind::Proprietary => 0.92,
            CellKind::Signal => 0.75,
        }
    }
}

/// The universal shipping container.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct DataCell {
    /// The unique location of this cell on the Terraforma grid.
    pub coordinate: Coordinate,

    /// What kind of data is inside.
    pub kind: CellKind,

    /// The 'Living Metal' weight of this cell.
    /// Increases automatically when the cell is successfully searched or used.
    pub resonance_weight: f32,

    /// The exterior labels. A fixed-size array of teeth for microsecond SIMD matching.
    /// We cap it at 16 to keep the struct size deterministic and cache-friendly.
    pub teeth: [Tooth; 16],

    /// The actual payload. In a real memory-mapped environment,
    /// this would be a pointer (offset and length) to the raw bytes on disk.
    pub payload_offset: u64,
    pub payload_length: u32,
}

impl DataCell {
    /// Forge a new, empty container ready to be packed.
    pub fn new(coordinate: Coordinate, kind: CellKind) -> Self {
        Self {
            coordinate,
            kind,
            resonance_weight: 0.5, // Base gravity
            teeth: [[0; 32]; 16],
            payload_offset: 0,
            payload_length: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_precious_replicates() {
        assert_eq!(CellKind::Memory.replication(), Replication::Replicate);
        assert_eq!(CellKind::Diamond.replication(), Replication::Replicate);

        assert_eq!(CellKind::Signal.replication(), Replication::Local);
        assert_eq!(CellKind::Document.replication(), Replication::Local);
        assert_eq!(CellKind::Executable.replication(), Replication::Local);
        assert_eq!(CellKind::Proprietary.replication(), Replication::Local);
        assert_eq!(CellKind::Observation.replication(), Replication::Local);
    }
}
