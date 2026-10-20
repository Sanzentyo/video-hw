use video_hw_core::{ColorMatrix, ColorRange};

/// Colorimetry validated from a decoder output format before image publication.
///
/// These are codec output facts, not hardware-buffer conversion suggestions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DecodedColorimetry {
    standard: ColorMatrix,
    range: ColorRange,
}

impl DecodedColorimetry {
    /// Validated output color standard.
    pub const fn standard(self) -> ColorMatrix {
        self.standard
    }

    /// Validated output quantization range.
    pub const fn range(self) -> ColorRange {
        self.range
    }

    #[cfg(any(target_os = "android", test))]
    pub(super) fn from_output_format(
        standard: Option<i32>,
        range: Option<i32>,
        expected_range: ColorRange,
    ) -> Result<Self, super::DecodeError> {
        // MediaFormat: COLOR_STANDARD_BT709=1, COLOR_RANGE_FULL=1,
        // COLOR_RANGE_LIMITED=2. Missing metadata never supplies an implicit range.
        let observed = match range {
            Some(1) => Some(ColorRange::Full),
            Some(2) => Some(ColorRange::Limited),
            _ => None,
        };
        match observed {
            Some(range) if standard == Some(1) && range == expected_range => Ok(Self {
                standard: ColorMatrix::Bt709,
                range,
            }),
            _ => Err(super::DecodeError::OutputColorimetry {
                standard,
                range,
                expected_range,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_range_must_match_the_explicit_policy() {
        for (expected, value) in [(ColorRange::Limited, 2), (ColorRange::Full, 1)] {
            let facts =
                DecodedColorimetry::from_output_format(Some(1), Some(value), expected).unwrap();
            assert_eq!(facts.standard(), ColorMatrix::Bt709);
            assert_eq!(facts.range(), expected);
            let different = if value == 1 { 2 } else { 1 };
            assert!(
                DecodedColorimetry::from_output_format(Some(1), Some(different), expected).is_err()
            );
        }
    }

    #[test]
    fn missing_unknown_or_wrong_standard_metadata_is_rejected() {
        for expected in [ColorRange::Limited, ColorRange::Full] {
            for standard in [None, Some(-1), Some(0), Some(2), Some(6)] {
                assert!(
                    DecodedColorimetry::from_output_format(standard, Some(1), expected).is_err()
                );
                assert!(
                    DecodedColorimetry::from_output_format(standard, Some(2), expected).is_err()
                );
            }
            for range in [None, Some(-1), Some(0), Some(3), Some(i32::MAX)] {
                let error =
                    DecodedColorimetry::from_output_format(Some(1), range, expected).unwrap_err();
                assert!(
                    matches!(error, super::super::DecodeError::OutputColorimetry {
                    standard: Some(1), range: actual, expected_range
                } if actual == range && expected_range == expected)
                );
            }
        }
    }
}
