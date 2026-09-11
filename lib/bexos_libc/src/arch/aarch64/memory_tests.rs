use super::{memcpy, memmove, memset};

#[test]
fn copies_unaligned_words_and_tails_without_touching_guards() {
    let input: [u8; 192] = core::array::from_fn(|i| i as u8);
    for source in 0..16 {
        for destination in 0..16 {
            for length in 0..129 {
                let mut output = [0xcc; 192];
                let mut expected = output;
                expected[destination..destination + length]
                    .copy_from_slice(&input[source..source + length]);
                let pointer = unsafe { output.as_mut_ptr().add(destination).cast() };
                assert_eq!(
                    unsafe { memcpy(pointer, input.as_ptr().add(source).cast(), length) },
                    pointer
                );
                assert_eq!(output, expected);
            }
        }
    }
}

#[test]
fn overlapping_moves_work_in_both_directions_at_every_word_offset() {
    for source in 0..24 {
        for destination in 0..24 {
            for length in 0..97 {
                let mut output: [u8; 128] = core::array::from_fn(|i| i as u8);
                let mut expected = output;
                expected.copy_within(source..source + length, destination);
                let pointer = unsafe { output.as_mut_ptr().add(destination).cast() };
                assert_eq!(
                    unsafe { memmove(pointer, output.as_ptr().add(source).cast(), length) },
                    pointer
                );
                assert_eq!(output, expected);
            }
        }
    }
}

#[test]
fn fills_words_and_tails_using_only_the_low_byte() {
    for value in [-1, 0, 0x1234, 255] {
        for destination in 0..16 {
            for length in 0..129 {
                let mut output = [0x5a; 160];
                let mut expected = output;
                expected[destination..destination + length].fill(value as u8);
                let pointer = unsafe { output.as_mut_ptr().add(destination).cast() };
                assert_eq!(unsafe { memset(pointer, value, length) }, pointer);
                assert_eq!(output, expected);
            }
        }
    }
}
