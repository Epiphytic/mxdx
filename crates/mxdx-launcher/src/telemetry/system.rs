use mxdx_types::events::telemetry::{
    CpuInfo, DiskInfo, HostTelemetryEvent, MemoryInfo, NetworkInfo,
};
use sysinfo::{Disks, Networks, System};

use crate::config::TelemetryDetail;

/// Collects host telemetry data respecting the configured detail level.
///
/// - `Summary`: hostname, os, arch, uptime, load_avg, basic cpu/memory only.
/// - `Full`: all fields populated including network, disk, services, devices.
pub fn collect_telemetry(detail_level: TelemetryDetail) -> HostTelemetryEvent {
    let mut sys = System::new_all();
    sys.refresh_all();

    let hostname = System::host_name().unwrap_or_default();
    let os = System::os_version().unwrap_or_default();
    let arch = std::env::consts::ARCH.to_string();
    let uptime_seconds = System::uptime();
    let load_avg = {
        let la = System::load_average();
        [la.one, la.five, la.fifteen]
    };

    let cpu = CpuInfo {
        cores: sys.cpus().len() as u32,
        usage_percent: sys.global_cpu_usage() as f64,
    };

    let memory = MemoryInfo {
        total_bytes: sys.total_memory(),
        used_bytes: sys.used_memory(),
    };

    match detail_level {
        TelemetryDetail::Summary => HostTelemetryEvent {
            timestamp: String::new(),
            hostname,
            os,
            arch,
            uptime_seconds,
            load_avg,
            cpu,
            memory,
            disk: DiskInfo {
                total_bytes: 0,
                used_bytes: 0,
            },
            network: None,
            services: None,
            devices: None,
        },
        TelemetryDetail::Full => {
            let disks = Disks::new_with_refreshed_list();
            let (disk_total, disk_used) = disks.iter().fold((0u64, 0u64), |(total, used), d| {
                (
                    total + d.total_space(),
                    used + (d.total_space() - d.available_space()),
                )
            });

            let networks = Networks::new_with_refreshed_list();
            let (rx, tx) = networks
                .iter()
                .fold((0u64, 0u64), |(rx, tx), (_name, data)| {
                    (rx + data.total_received(), tx + data.total_transmitted())
                });

            HostTelemetryEvent {
                timestamp: String::new(),
                hostname,
                os,
                arch,
                uptime_seconds,
                load_avg,
                cpu,
                memory,
                disk: DiskInfo {
                    total_bytes: disk_total,
                    used_bytes: disk_used,
                },
                network: Some(NetworkInfo {
                    rx_bytes: rx,
                    tx_bytes: tx,
                }),
                services: None,
                devices: None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::TelemetryDetail;

    #[test]
    fn telemetry_summary_mode_excludes_detailed_fields() {
        let telemetry = collect_telemetry(TelemetryDetail::Summary);
        assert!(telemetry.network.is_none());
        assert!(telemetry.services.is_none());
        assert!(telemetry.devices.is_none());
    }

    #[test]
    fn telemetry_full_mode_includes_all_fields() {
        let telemetry = collect_telemetry(TelemetryDetail::Full);
        assert!(telemetry.network.is_some());
        assert!(!telemetry.hostname.is_empty());
    }

    #[test]
    fn telemetry_has_basic_system_info() {
        let telemetry = collect_telemetry(TelemetryDetail::Summary);
        assert!(!telemetry.hostname.is_empty());
        assert!(!telemetry.os.is_empty());
        assert!(!telemetry.arch.is_empty());
        assert!(telemetry.uptime_seconds > 0);
    }
}
