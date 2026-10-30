//! `--check data-quality`: fails when a source cannot be trusted or analysed.
//!
//! Only data quality is checked. Notices and info never produce a failure, because no rule is
//! calibrated well enough to gate a pipeline on it.

use crate::model::DataQuality;

/// Share of unreadable lines above which a source fails the check.
/// Source: assumption. One damaged line in a hundred is a log that is being written to or cut off;
/// more suggests a format the reader does not understand.
pub const MAX_SKIPPED_LINES_PCT: f64 = 1.0;
/// Share of compressed files that could not be decoded above which a source fails the check.
/// Source: assumption.
pub const MAX_UNREADABLE_COMPRESSED_PCT: f64 = 5.0;

fn percent(part: u64, whole: u64) -> f64 {
    if whole == 0 {
        0.0
    } else {
        part as f64 * 100.0 / whole as f64
    }
}

/// Reasons the source fails; empty when it passes. `requests_in_period` is what the selected
/// period holds, so an empty period fails like a source without usage.
pub fn data_quality(quality: &DataQuality, requests_in_period: u64) -> Vec<String> {
    let mut reasons = Vec::new();
    if requests_in_period == 0 {
        reasons.push("no requests with usage in the selected period".to_owned());
    }
    let read = &quality.read;
    let skipped = percent(read.skipped_lines, read.lines);
    if skipped > MAX_SKIPPED_LINES_PCT {
        reasons.push(format!(
            "{skipped:.1} % of lines unreadable ({} of {}), limit {MAX_SKIPPED_LINES_PCT} %",
            read.skipped_lines, read.lines
        ));
    }
    let compressed = read.compressed_files_read + read.compressed_files_unreadable;
    let unreadable = percent(read.compressed_files_unreadable, compressed);
    if unreadable > MAX_UNREADABLE_COMPRESSED_PCT {
        reasons.push(format!(
            "{unreadable:.1} % of compressed files unreadable ({} of {compressed}), limit {MAX_UNREADABLE_COMPRESSED_PCT} %",
            read.compressed_files_unreadable
        ));
    }
    reasons
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ReadStats;

    fn quality(read: ReadStats) -> DataQuality {
        DataQuality {
            observed_from: None,
            observed_to: None,
            requests: 1,
            analysable: true,
            read,
        }
    }

    #[test]
    fn clean_source_passes() {
        assert!(data_quality(&quality(ReadStats::default()), 5).is_empty());
    }

    #[test]
    fn empty_period_unreadable_lines_and_files_fail() {
        assert_eq!(data_quality(&quality(ReadStats::default()), 0).len(), 1);
        let lines = ReadStats {
            lines: 100,
            skipped_lines: 2,
            ..ReadStats::default()
        };
        assert_eq!(data_quality(&quality(lines), 5).len(), 1);
        let at_limit = ReadStats {
            lines: 100,
            skipped_lines: 1,
            ..ReadStats::default()
        };
        assert!(
            data_quality(&quality(at_limit), 5).is_empty(),
            "the limit itself passes"
        );
        let files = ReadStats {
            compressed_files_read: 9,
            compressed_files_unreadable: 1,
            ..ReadStats::default()
        };
        assert_eq!(data_quality(&quality(files), 5).len(), 1);
    }

    #[test]
    fn unknown_types_and_notes_do_not_fail() {
        let mut read = ReadStats::default();
        read.unknown_types.insert("future".into(), 50);
        read.files_without_usage = 3;
        assert!(data_quality(&quality(read), 5).is_empty());
    }
}
