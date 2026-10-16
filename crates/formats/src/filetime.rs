/// Seconds between 1601-01-01 (FILETIME epoch) and 1970-01-01.
const EPOCH_DIFF: i64 = 11_644_473_600;

/// Windows FILETIME (100 ns ticks since 1601) to Unix seconds. Zero and values outside
/// 1980 to 2200 become 0 ("unknown"): a damaged record should not show a date in year 30828.
pub fn to_unix(ft: u64) -> i64 {
    if ft == 0 {
        return 0;
    }
    let secs = (ft / 10_000_000) as i64 - EPOCH_DIFF;
    if (315_532_800..=7_258_118_400).contains(&secs) {
        secs
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn converts_and_rejects_nonsense() {
        // 2024-01-01T00:00:00Z
        assert_eq!(super::to_unix(133_485_408_000_000_000), 1_704_067_200);
        assert_eq!(super::to_unix(0), 0);
        assert_eq!(super::to_unix(1), 0);
        assert_eq!(super::to_unix(u64::MAX), 0);
    }
}
