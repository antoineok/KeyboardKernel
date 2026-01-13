// SPDX-License-Identifier: MIT OR Apache-2.0
//
// Copyright (c) 2020-2023 Andre Richter <andre.o.richter@gmail.com>

//! Timer primitives.

use core::time::Duration;

//--------------------------------------------------------------------------------------------------
// Public Definitions
//--------------------------------------------------------------------------------------------------

/// Provides time management functions.
pub struct TimeManager;

//--------------------------------------------------------------------------------------------------
// Global instances
//--------------------------------------------------------------------------------------------------

static TIME_MANAGER: TimeManager = TimeManager::new();

//--------------------------------------------------------------------------------------------------
// Public Code
//--------------------------------------------------------------------------------------------------

/// Return a reference to the global TimeManager.
pub fn time_manager() -> &'static TimeManager {
    &TIME_MANAGER
}

impl TimeManager {
    /// Create an instance.
    pub const fn new() -> Self {
        Self
    }

    /// The timer's resolution.
    pub fn resolution(&self) -> Duration {
        crate::arch::time::resolution()
    }

    /// The uptime since power-on of the device.
    ///
    /// This includes time consumed by firmware and bootloaders.
    pub fn uptime(&self) -> Duration {
        crate::arch::time::uptime()
    }

    /// Spin for a given duration.
    pub fn spin_for(&self, duration: Duration) {
        crate::arch::time::spin_for(duration)
    }
}
