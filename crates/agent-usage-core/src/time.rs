//! Small time-formatting helpers shared by every frontend.

/// Formats a duration as a compact human string: `3d 12h`, `2h 45m`, `45m` (min `1m`).
/// Negative durations render as `0m`.
pub fn format_duration(duration: chrono::Duration) -> String {
    let total_secs = duration.num_seconds();
    if total_secs < 0 {
        return String::from("0m");
    }

    let days = total_secs / 86400;
    let hours = (total_secs % 86400) / 3600;
    let minutes = (total_secs % 3600) / 60;

    match (days, hours, minutes) {
        (d, h, _) if d > 0 && h > 0 => format!("{}d {}h", d, h),
        (d, _, _) if d > 0 => format!("{}d", d),
        (_, h, m) if h > 0 && m > 0 => format!("{}h {}m", h, m),
        (_, h, _) if h > 0 => format!("{}h", h),
        (_, _, m) => format!("{}m", m.max(1)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(
            format_duration(chrono::Duration::seconds(3 * 86400 + 12 * 3600)),
            "3d 12h"
        );
        assert_eq!(format_duration(chrono::Duration::seconds(5 * 86400)), "5d");
        assert_eq!(
            format_duration(chrono::Duration::seconds(2 * 3600 + 45 * 60)),
            "2h 45m"
        );
        assert_eq!(format_duration(chrono::Duration::seconds(3 * 3600)), "3h");
        assert_eq!(format_duration(chrono::Duration::seconds(45 * 60)), "45m");
        assert_eq!(format_duration(chrono::Duration::seconds(0)), "1m");
        assert_eq!(format_duration(chrono::Duration::seconds(-100)), "0m");
    }
}
