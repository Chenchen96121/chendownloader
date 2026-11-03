//! A partial fMP4 probe is not a completed media segment.
use super::hls_local::HlsByteRange;
use crate::domain::media::AnalysisErrorCode;

pub(super) fn request_range(
    original: Option<HlsByteRange>,
    maximum: u64,
) -> Result<HlsByteRange, AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    if maximum == 0
        || maximum > 512 * 1_048_576
        || original.is_some_and(|r| {
            r.length == 0 || r.length > 512 * 1_048_576 || r.offset.checked_add(r.length).is_none()
        })
    {
        return Err(invalid);
    }
    Ok(HlsByteRange {
        offset: original.map_or(0, |r| r.offset),
        length: original
            .map_or(maximum, |r| r.length)
            .min(maximum)
            .min(1_048_576),
    })
}
pub(super) fn read_limit(
    original: Option<HlsByteRange>,
    maximum: u64,
    status: u16,
    content_range: Option<&str>,
    length: Option<u64>,
) -> Result<(u64, Option<u64>), AnalysisErrorCode> {
    let invalid = AnalysisErrorCode::InvalidMetadata;
    let requested = request_range(original, maximum)?;
    if status == 200 && original.is_none() && content_range.is_none() {
        if length.is_some_and(|n| n == 0 || n > maximum) {
            return Err(invalid);
        }
        return Ok((maximum, length));
    }
    if status != 206 {
        return Err(invalid);
    }
    let number = |raw: &str| -> Result<u64, AnalysisErrorCode> {
        if raw.is_empty() || raw.len() > 20 || !raw.bytes().all(|b| b.is_ascii_digit()) {
            return Err(invalid);
        }
        raw.parse().map_err(|_| invalid)
    };
    let (bounds, total) = content_range
        .and_then(|r| r.strip_prefix("bytes "))
        .and_then(|r| r.split_once('/'))
        .ok_or(invalid)?;
    let total = number(total)?;
    let (start, end) = bounds.split_once('-').ok_or(invalid)?;
    let start = number(start)?;
    let end = number(end)?;
    let wanted_end = requested
        .offset
        .checked_add(requested.length)
        .and_then(|n| n.checked_sub(1))
        .ok_or(invalid)?;
    if start != requested.offset
        || end < start
        || end > wanted_end
        || total <= end
        || (end != wanted_end && (original.is_some() || end.checked_add(1) != Some(total)))
        || original.is_some_and(|r| r.offset.checked_add(r.length).is_none_or(|n| n > total))
    {
        return Err(invalid);
    }
    let count = end
        .checked_sub(start)
        .and_then(|n| n.checked_add(1))
        .ok_or(invalid)?;
    if length.is_some_and(|n| n != count) {
        return Err(invalid);
    }
    Ok((count, Some(count)))
}

#[cfg(test)]
mod tests {
    use super::*;
    const MIB: u64 = 1_048_576;
    fn range(offset: u64, length: u64) -> Option<HlsByteRange> {
        Some(HlsByteRange { offset, length })
    }
    #[test]
    fn prefix_request_stays_within_original_range_and_remaining_budget() {
        for (original, max, want) in [
            (None, 32 * MIB, (0, MIB)),
            (range(27, 9), 32 * MIB, (27, 9)),
            (range(27, 2 * MIB), 123, (27, 123)),
        ] {
            let actual = request_range(original, max).unwrap();
            assert_eq!((actual.offset, actual.length), want);
        }
        for (original, max) in [
            (None, 0),
            (None, 513 * MIB),
            (range(1, 0), MIB),
            (range(u64::MAX, 2), MIB),
            (range(0, 513 * MIB), MIB),
        ] {
            assert!(request_range(original, max).is_err());
        }
    }
    #[test]
    fn partial_response_accepts_exact_prefix_or_real_small_eof_only() {
        for (original, max, raw, length, want) in [
            (
                None,
                32 * MIB,
                "bytes 0-1048575/4000000",
                Some(MIB),
                (MIB, Some(MIB)),
            ),
            (None, 32 * MIB, "bytes 0-48/49", Some(49), (49, Some(49))),
            (None, 100, "bytes 0-99/4000000", None, (100, Some(100))),
            (
                range(200, 2 * MIB),
                32 * MIB,
                "bytes 200-1048775/4000000",
                Some(MIB),
                (MIB, Some(MIB)),
            ),
            (
                range(200, 50),
                32 * MIB,
                "bytes 200-249/250",
                None,
                (50, Some(50)),
            ),
        ] {
            assert_eq!(
                read_limit(original, max, 206, Some(raw), length).unwrap(),
                want
            );
        }
    }
    #[test]
    fn partial_response_rejects_lies_unknown_totals_and_outside_original_range() {
        for raw in [
            "bytes 1-1048576/4000000",
            "bytes 0-99/4000000",
            "bytes 0-1048576/4000000",
            "bytes 0-1048575/*",
            "bytes 0-1048575/1048575",
            "bytes +0-48/49",
            "bytes 0-48/49junk",
            "bytes 0-48/0",
            "bytes 0-18446744073709551615/18446744073709551615",
        ] {
            assert!(
                read_limit(None, 32 * MIB, 206, Some(raw), None).is_err(),
                "{raw}"
            );
        }
        assert!(read_limit(None, 32 * MIB, 206, None, Some(MIB)).is_err());
        assert!(read_limit(
            None,
            32 * MIB,
            206,
            Some("bytes 0-1048575/4000000"),
            Some(5)
        )
        .is_err());
        assert!(read_limit(
            range(200, 1000),
            32 * MIB,
            206,
            Some("bytes 200-999/1000"),
            Some(800)
        )
        .is_err());
        assert!(read_limit(
            range(200, 1000),
            32 * MIB,
            206,
            Some("bytes 200-1199/1100"),
            Some(1000)
        )
        .is_err());
    }
    #[test]
    fn range_ignoring_server_is_full_bounded_read_only_without_original_range() {
        assert_eq!(
            read_limit(None, 32 * MIB, 200, None, Some(2 * MIB)).unwrap(),
            (32 * MIB, Some(2 * MIB))
        );
        assert_eq!(
            read_limit(None, 32 * MIB, 200, None, None).unwrap(),
            (32 * MIB, None)
        );
        for (original, status, raw, length) in [
            (range(0, 49), 200, None, Some(49)),
            (None, 200, None, Some(33 * MIB)),
            (None, 200, None, Some(0)),
            (None, 200, Some("bytes 0-48/49"), Some(49)),
            (None, 204, None, None),
            (None, 416, None, None),
        ] {
            assert!(read_limit(original, 32 * MIB, status, raw, length).is_err());
        }
    }
}
