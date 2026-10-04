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

/// How hard the machine is working right now.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MachineLoad {
    pub heat: Heat,
    /// The OS reports memory pressure (macOS: warning or critical).
    pub memory_tight: bool,
}

impl MachineLoad {
    /// Whether new heavy work (a new worker, a new build or test run) should wait.
    pub fn strained(&self) -> bool {
        self.heat >= Heat::Serious || self.memory_tight
    }

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

/// `kern.memorystatus_vm_pressure_level` (1 normal, 2 warning, 4 critical): whether memory is
/// tight.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub(crate) fn memory_tight_from_pressure_level(level: i32) -> bool {
    level >= 2
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

/// Whether `/proc/pressure/memory` shows memory tight: some task stalled on memory for a
/// quarter of the last 10 s, or every task for a tenth of it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn memory_tight_from_psi(text: &str) -> bool {
    psi_avg10(text, "some").is_some_and(|some| some >= 25.0)
        || psi_avg10(text, "full").is_some_and(|full| full >= 10.0)
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

/// Whether a Windows memory load (percent of physical memory in use) is tight.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn memory_tight_from_load(percent: u32) -> bool {
    percent >= 90
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strained_means_serious_heat_or_tight_memory() {
        let calm = MachineLoad::default();
        assert!(!calm.strained() && !calm.critical());
        let fair = MachineLoad {
            heat: Heat::Fair,
            memory_tight: false,
        };
        assert!(!fair.strained());
        let serious = MachineLoad {
            heat: Heat::Serious,
            memory_tight: false,
        };
        assert!(serious.strained() && !serious.critical());
        let tight = MachineLoad {
            heat: Heat::Nominal,
            memory_tight: true,
        };
        assert!(tight.strained());
        let critical = MachineLoad {
            heat: Heat::Critical,
            memory_tight: false,
        };
        assert!(critical.strained() && critical.critical());
    }

    #[test]
    fn os_levels_map_to_heat_and_memory() {
        assert_eq!(heat_from_thermal_state(0), Heat::Nominal);
        assert_eq!(heat_from_thermal_state(1), Heat::Fair);
        assert_eq!(heat_from_thermal_state(2), Heat::Serious);
        assert_eq!(heat_from_thermal_state(3), Heat::Critical);
        assert!(!memory_tight_from_pressure_level(1));
        assert!(memory_tight_from_pressure_level(2));
        assert!(memory_tight_from_pressure_level(4));
        assert!(!memory_tight_from_load(70));
        assert!(memory_tight_from_load(95));
    }

    #[test]
    fn pressure_stall_files_are_read() {
        let calm = "some avg10=1.50 avg60=0.80 avg300=0.10 total=1234\nfull avg10=0.00 avg60=0.00 avg300=0.00 total=0\n";
        assert!(!memory_tight_from_psi(calm));
        let some = "some avg10=31.00 avg60=12.00 avg300=3.00 total=99\nfull avg10=2.00 avg60=1.00 avg300=0.00 total=9\n";
        assert!(memory_tight_from_psi(some));
        let full = "some avg10=12.00 avg60=12.00 avg300=3.00 total=99\nfull avg10=11.00 avg60=1.00 avg300=0.00 total=9\n";
        assert!(memory_tight_from_psi(full));
        assert!(!memory_tight_from_psi(""));
    }

    #[test]
    fn thermal_zones_are_judged_by_their_own_trip_points() {
        let root = std::env::temp_dir().join(format!("brigadier-thermal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
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
        std::fs::remove_dir_all(&root).unwrap();
    }
}
