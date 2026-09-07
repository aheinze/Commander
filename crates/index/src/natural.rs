use std::cmp::Ordering;

/// Compares normalized UTF-8 sort keys naturally without allocating per comparison.
pub(crate) fn compare(left: &str, right: &str) -> Ordering {
    let left = left.as_bytes();
    let right = right.as_bytes();
    let mut left_offset = 0;
    let mut right_offset = 0;

    while left_offset < left.len() && right_offset < right.len() {
        if left[left_offset].is_ascii_digit() && right[right_offset].is_ascii_digit() {
            let (left_end, left_significant) = number_bounds(left, left_offset);
            let (right_end, right_significant) = number_bounds(right, right_offset);
            let left_digits = &left[left_significant..left_end];
            let right_digits = &right[right_significant..right_end];

            match left_digits.len().cmp(&right_digits.len()) {
                Ordering::Equal => {}
                ordering => return ordering,
            }
            match left_digits.cmp(right_digits) {
                Ordering::Equal => {}
                ordering => return ordering,
            }

            let left_leading_zeroes = left_significant - left_offset;
            let right_leading_zeroes = right_significant - right_offset;
            match left_leading_zeroes.cmp(&right_leading_zeroes) {
                Ordering::Equal => {}
                ordering => return ordering,
            }

            left_offset = left_end;
            right_offset = right_end;
            continue;
        }

        match left[left_offset].cmp(&right[right_offset]) {
            Ordering::Equal => {
                left_offset += 1;
                right_offset += 1;
            }
            ordering => return ordering,
        }
    }

    left.len().cmp(&right.len())
}

fn number_bounds(bytes: &[u8], start: usize) -> (usize, usize) {
    let mut end = start;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let mut significant = start;
    while significant + 1 < end && bytes[significant] == b'0' {
        significant += 1;
    }
    (end, significant)
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering;

    use proptest::prelude::*;

    use super::compare;

    #[test]
    fn numbers_are_ordered_by_value_without_parsing_overflow() {
        assert_eq!(compare("img2", "img10"), Ordering::Less);
        assert_eq!(
            compare("n999999999999999999999", "n1000000000000000000000"),
            Ordering::Less
        );
    }

    #[test]
    fn fewer_leading_zeroes_sort_first_for_equal_numbers() {
        assert_eq!(compare("img2", "img002"), Ordering::Less);
    }

    proptest! {
        #[test]
        fn comparison_is_antisymmetric(left in "[a-z0-9]{0,30}", right in "[a-z0-9]{0,30}") {
            prop_assert_eq!(compare(&left, &right), compare(&right, &left).reverse());
        }
    }
}
