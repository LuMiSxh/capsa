// SPDX-License-Identifier: MPL-2.0
//! Human-readable durations and sizes for the doctor details.

pub(super) fn human_secs(s: u64) -> String {
    match s {
        0..120 => format!("{s}s"),
        120..7200 => format!("{}m", s / 60),
        7200..172_800 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

pub(super) fn human_bytes(n: u64) -> String {
    match n {
        0..10_240 => format!("{n} bytes"),
        10_240..10_485_760 => format!("{} KiB", n / 1024),
        _ => format!("{} MiB", n / 1_048_576),
    }
}
