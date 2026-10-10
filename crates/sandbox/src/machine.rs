//! How hard the machine is working, as the OS itself tells it: its heat state and memory
//! pressure. Brigadier holds new heavy work while the machine struggles (PLAN.md §10.7); it
//! never reads a temperature and compares it against a number of its own.
//!
//! - macOS: `ProcessInfo.thermalState` and the kernel's memory-pressure level (the level the
//!   memory-pressure dispatch source reports, read directly so it needs no event loop).
//! - Linux: thermal zones against their own trip points, and pressure stall information
//!   (`/proc/pressure/memory`).
//! - Windows: the memory load. Windows has no reliable unprivileged heat signal, so its heat
//!   reads as nominal; memory pressure and the build lease still hold work back.
//!
//! A signal the OS doesn't give reads as calm.

use std::path::Path;

/// The machine's heat, in the OS's own four steps (`ProcessInfo.ThermalState` on macOS).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum Heat {
    #[default]
    Nominal,
    Fair,
    /// Hot enough that the OS slows things down: no new heavy work starts.
    Serious,
    /// As hot as it gets: held for long, Brigadier's own builds are paused.
    Critical,
}

/// Memory pressure in the OS's levels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemoryPressure {
    #[default]
    Normal,
    Warning,
    Critical,
}

/// How hard the machine is working right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MachineLoad {
    pub heat: Heat,
    pub memory: MemoryPressure,
}

impl MachineLoad {
    /// Workers can start under a memory warning; only critical pressure holds them.
    pub fn workers_held(&self) -> bool {
        self.heat >= Heat::Serious || self.memory == MemoryPressure::Critical
    }

    /// Builds and tests are memory-heavy, so even a warning holds new runs.
    pub fn builds_held(&self) -> bool {
        self.heat >= Heat::Serious || self.memory >= MemoryPressure::Warning
    }

    /// Only sustained critical heat escalates to pausing already running builds.
    pub fn critical(&self) -> bool {
        self.heat == Heat::Critical
    }
}

/// Reads the machine's load.
pub trait Machine: Send + Sync {
    /// The load now. Cheap enough to call every few seconds.
    fn load(&self) -> MachineLoad;
}

/// `ProcessInfo.thermalState`'s raw value as a [`Heat`].
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn heat_from_thermal_state(state: isize) -> Heat {
    match state {
        ..=0 => Heat::Nominal,
        1 => Heat::Fair,
        2 => Heat::Serious,
        _ => Heat::Critical,
    }
}

/// `kern.memorystatus_vm_pressure_level` (1 normal, 2 warning, 4 critical).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn memory_from_pressure_level(level: i32) -> MemoryPressure {
    match level {
        4.. => MemoryPressure::Critical,
        2.. => MemoryPressure::Warning,
        _ => MemoryPressure::Normal,
    }
}

/// A share of stalled time (`some`, `full`; `avg10`, in percent) from a pressure stall file.
fn psi_avg10(text: &str, line: &str) -> Option<f64> {
    text.lines()
        .find(|row| row.split_whitespace().next() == Some(line))?
        .split_whitespace()
        .find_map(|field| field.strip_prefix("avg10="))?
        .parse()
        .ok()
}

/// Memory PSI over ten seconds: warning at `some >= 25%` or `full >= 10%`.
/// Critical at `full >= 25%`: all non-idle tasks stalled together for a quarter of the
/// window, indicating sustained thrashing rather than just some busy tasks reclaiming.
/// See <https://docs.kernel.org/accounting/psi.html> for the OS signal's meaning.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn memory_from_psi(text: &str) -> MemoryPressure {
    let full = psi_avg10(text, "full").unwrap_or_default();
    if full >= 25.0 {
        MemoryPressure::Critical
    } else if full >= 10.0 || psi_avg10(text, "some").is_some_and(|some| some >= 25.0) {
        MemoryPressure::Warning
    } else {
        MemoryPressure::Normal
    }
}

/// The heat of the hottest thermal zone under `thermal` (`/sys/class/thermal`), each against
/// its own trip points: at or past a `passive` trip is serious; at or past a `hot` trip, or
/// within 5 °C of the `critical` one, is critical. Zones without trip points say nothing.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn heat_from_thermal_zones(thermal: &Path) -> Heat {
    let read =
        |path: &Path| -> Option<i64> { std::fs::read_to_string(path).ok()?.trim().parse().ok() };
    let Ok(entries) = std::fs::read_dir(thermal) else {
        return Heat::Nominal;
    };
    let mut heat = Heat::Nominal;
    for entry in entries.flatten() {
        let zone = entry.path();
        if !entry
            .file_name()
            .to_string_lossy()
            .starts_with("thermal_zone")
        {
            continue;
        }
        let Some(temp) = read(&zone.join("temp")) else {
            continue;
        };
        for trip in 0.. {
            let Ok(kind) = std::fs::read_to_string(zone.join(format!("trip_point_{trip}_type")))
            else {
                break;
            };
            let Some(at) = read(&zone.join(format!("trip_point_{trip}_temp"))) else {
                continue;
            };
            if at <= 0 {
                continue;
            }
            let reached = match kind.trim() {
                "critical" if temp >= at - 5_000 => Heat::Critical,
                "hot" if temp >= at => Heat::Critical,
                "passive" if temp >= at => Heat::Serious,
                _ => Heat::Nominal,
            };
            heat = heat.max(reached);
        }
    }
    heat
}

/// Windows physical memory in use: warning at 90%, critical at 95%.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn memory_from_load(percent: u32) -> MemoryPressure {
    match percent {
        95.. => MemoryPressure::Critical,
        90.. => MemoryPressure::Warning,
        _ => MemoryPressure::Normal,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A folder in the temp directory, removed when dropped however the test ends.
    struct Temp(PathBuf);

    impl std::ops::Deref for Temp {
        type Target = Path;

        fn deref(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn workers_and_builds_use_different_memory_thresholds() {
        for (level, memory, workers, builds) in [
            (1, MemoryPressure::Normal, false, false),
            (2, MemoryPressure::Warning, false, true),
            (4, MemoryPressure::Critical, true, true),
        ] {
            let load = MachineLoad {
                heat: Heat::Nominal,
                memory: memory_from_pressure_level(level),
            };
            assert_eq!(load.memory, memory);
            assert_eq!(load.workers_held(), workers);
            assert_eq!(load.builds_held(), builds);
            assert!(
                !load.critical(),
                "memory never escalates to pausing running builds"
            );
        }
        for heat in [Heat::Nominal, Heat::Fair, Heat::Serious, Heat::Critical] {
            let load = MachineLoad {
                heat,
                ..Default::default()
            };
            assert_eq!(load.workers_held(), heat >= Heat::Serious);
            assert_eq!(load.builds_held(), heat >= Heat::Serious);
            assert_eq!(load.critical(), heat == Heat::Critical);
        }
    }

    #[test]
    fn os_levels_map_to_heat_and_memory() {
        assert_eq!(heat_from_thermal_state(0), Heat::Nominal);
        assert_eq!(heat_from_thermal_state(1), Heat::Fair);
        assert_eq!(heat_from_thermal_state(2), Heat::Serious);
        assert_eq!(heat_from_thermal_state(3), Heat::Critical);
        assert_eq!(memory_from_load(89), MemoryPressure::Normal);
        assert_eq!(memory_from_load(90), MemoryPressure::Warning);
        assert_eq!(memory_from_load(94), MemoryPressure::Warning);
        assert_eq!(memory_from_load(95), MemoryPressure::Critical);
    }

    #[test]
    fn pressure_stall_files_are_read() {
        for (text, expected) in [
            ("some avg10=24.99\nfull avg10=9.99", MemoryPressure::Normal),
            ("some avg10=25.00\nfull avg10=0.00", MemoryPressure::Warning),
            (
                "some avg10=12.00\nfull avg10=10.00",
                MemoryPressure::Warning,
            ),
            (
                "some avg10=40.00\nfull avg10=24.99",
                MemoryPressure::Warning,
            ),
            (
                "some avg10=40.00\nfull avg10=25.00",
                MemoryPressure::Critical,
            ),
            ("", MemoryPressure::Normal),
        ] {
            assert_eq!(memory_from_psi(text), expected, "{text}");
        }
    }

    #[test]
    fn thermal_zones_are_judged_by_their_own_trip_points() {
        let root = std::env::temp_dir().join(format!("brigadier-thermal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let root = Temp(root);
        let zone = |name: &str, temp: i64, trips: &[(&str, i64)]| {
            let dir = root.join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("temp"), format!("{temp}\n")).unwrap();
            for (index, (kind, at)) in trips.iter().enumerate() {
                std::fs::write(dir.join(format!("trip_point_{index}_type")), kind).unwrap();
                std::fs::write(dir.join(format!("trip_point_{index}_temp")), at.to_string())
                    .unwrap();
            }
        };
        assert_eq!(heat_from_thermal_zones(&root), Heat::Nominal);
        zone(
            "thermal_zone0",
            60_000,
            &[("passive", 85_000), ("critical", 105_000)],
        );
        zone("cooling_device0", 99_000, &[("passive", 1)]);
        assert_eq!(heat_from_thermal_zones(&root), Heat::Nominal);
        zone(
            "thermal_zone1",
            90_000,
            &[("passive", 85_000), ("critical", 105_000)],
        );
        assert_eq!(heat_from_thermal_zones(&root), Heat::Serious);
        zone(
            "thermal_zone2",
            101_000,
            &[("passive", 85_000), ("critical", 105_000)],
        );
        assert_eq!(heat_from_thermal_zones(&root), Heat::Critical);
    }
}
