//! The Clockwork Governor: The Constitutional Checkpoint
//!
//! This module acts as the tollbooth for the Terraforma grid.
//! It mechanically enforces rate limits, dissonance thresholds, and the Strategic Pause,
//! ensuring no agent or Silt Weaver can overwhelm or corrupt the universe.

use crate::cell::DataCell;

/// The reasons why a Cell might be rejected by the Governor.
#[derive(Debug)]
pub enum Rejection {
    /// The cell contains no teeth (unsearchable mud).
    EmptyTeeth,
    /// The system is under too much pressure (rate limit exceeded).
    OverPressure,
    /// The cell's semantic weight is too low (dissonance).
    HighDissonance,
}

/// The physical representation of system pressure.
pub struct HydraulicThrottle {
    /// Current write requests per second.
    pub current_pressure: u32,
    /// The maximum safe threshold before the Strategic Pause is forced.
    pub max_safe_pressure: u32,
}

impl HydraulicThrottle {
    /// Simulates the mechanical valve. If pressure is too high,
    /// the thread is forced to yield (Strategic Pause).
    pub fn regulate(&mut self) -> Result<(), Rejection> {
        if self.current_pressure > self.max_safe_pressure {
            // In a real async environment, this would be tokio time sleep.
            // For the bedrock, it forces a mechanical rejection to protect the grid.
            return Err(Rejection::OverPressure);
        }
        self.current_pressure += 1;
        Ok(())
    }
}

/// The main tollbooth.
pub struct Governor {
    pub throttle: HydraulicThrottle,
}

impl Governor {
    pub fn new(max_pressure: u32) -> Self {
        Self {
            throttle: HydraulicThrottle {
                current_pressure: 0,
                max_safe_pressure: max_pressure,
            },
        }
    }

    /// The single entry point for a Cell to be admitted to the grid.
    pub fn inspect_and_admit(&mut self, cell: &DataCell) -> Result<(), Rejection> {
        // 1. Check Pressure (Hydraulics)
        self.throttle.regulate()?;

        // 2. Check Dissonance (Does it have teeth?)
        // If the first tooth is completely empty (all zeros), it is raw mud.
        if cell.teeth[0] == [0; 32] {
            return Err(Rejection::EmptyTeeth);
        }

        // If it passes the mechanics, it is approved for placement on the Monoliths grid.
        Ok(())
    }
}
