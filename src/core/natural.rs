use std::cmp::Ordering;

pub fn cmp_natural(a: &str, b: &str) -> Ordering {
    let left = a.as_bytes();
    let right = b.as_bytes();
    let mut i = 0;
    let mut j = 0;

    while i < left.len() && j < right.len() {
        let ca = left[i];
        let cb = right[j];

        if ca.is_ascii_digit() && cb.is_ascii_digit() {
            let (next_i, value_a, digits_a, leading_a) = read_number(left, i);
            let (next_j, value_b, digits_b, leading_b) = read_number(right, j);

            match value_a.cmp(&value_b) {
                Ordering::Equal => {
                    let significant_a = digits_a.saturating_sub(leading_a);
                    let significant_b = digits_b.saturating_sub(leading_b);
                    match significant_a.cmp(&significant_b) {
                        Ordering::Equal => {
                            i = next_i;
                            j = next_j;
                        }
                        other => return other,
                    }
                }
                other => return other,
            }
        } else {
            let la = ca.to_ascii_lowercase();
            let lb = cb.to_ascii_lowercase();
            match la.cmp(&lb) {
                Ordering::Equal => {
                    i += 1;
                    j += 1;
                }
                other => return other,
            }
        }
    }

    left.len().cmp(&right.len()).then_with(|| a.cmp(b))
}

fn read_number(bytes: &[u8], mut index: usize) -> (usize, u128, usize, usize) {
    let start = index;
    let mut value = 0u128;
    let mut leading_zeroes = 0usize;
    let mut still_leading = true;

    while index < bytes.len() && bytes[index].is_ascii_digit() {
        let digit = (bytes[index] - b'0') as u128;
        if still_leading && digit == 0 {
            leading_zeroes += 1;
        } else {
            still_leading = false;
        }
        value = value.saturating_mul(10).saturating_add(digit);
        index += 1;
    }

    (index, value, index - start, leading_zeroes)
}

#[cfg(test)]
mod tests {
    use super::cmp_natural;

    #[test]
    fn sorts_comic_page_numbers_naturally() {
        let mut pages = vec![
            "page-10.jpg".to_owned(),
            "page-2.jpg".to_owned(),
            "page-001.jpg".to_owned(),
            "page-1.jpg".to_owned(),
        ];

        pages.sort_by(|a, b| cmp_natural(a, b));

        assert_eq!(
            pages,
            vec![
                "page-1.jpg".to_owned(),
                "page-001.jpg".to_owned(),
                "page-2.jpg".to_owned(),
                "page-10.jpg".to_owned(),
            ]
        );
    }
}
