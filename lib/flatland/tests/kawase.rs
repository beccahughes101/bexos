use bexos_flatland::{Damage, kawase::Pyramid};
use bexos_kawase_reference::render;

#[test]
fn damaged_pyramids_match_full_recomputation_at_odd_sizes_and_edges() {
    for (width, height) in [(1, 1), (17, 9), (65, 43), (257, 129)] {
        let input: Vec<u8> = (0..width * height)
            .flat_map(|i| {
                [
                    (i * 31 % 256) as u8,
                    (i * 53 % 256) as u8,
                    (i * 97 % 256) as u8,
                    255,
                ]
            })
            .collect();
        for levels in 1..=5 {
            let full = Damage {
                x: 0,
                y: 0,
                width,
                height,
            };
            let reference = render(width, height, levels, &input, full, false);
            for output in [
                Damage {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                Damage {
                    x: width - 1,
                    y: height - 1,
                    width: 1,
                    height: 1,
                },
                Damage {
                    x: width / 3,
                    y: height / 3,
                    width: (width / 5).max(1),
                    height: (height / 5).max(1),
                },
            ] {
                let partial = render(width, height, levels, &input, output, true);
                for y in 0..height {
                    for x in 0..width {
                        let index = ((y * width + x) * 4) as usize;
                        let actual = &partial[index..index + 4];
                        if x >= output.x
                            && y >= output.y
                            && x < output.x + output.width
                            && y < output.y + output.height
                        {
                            assert_eq!(
                                actual,
                                &reference[index..index + 4],
                                "{width}x{height}, depth {levels}, ({x},{y})"
                            );
                        } else {
                            assert_eq!(actual, &[213; 4]);
                        }
                    }
                }
            }
        }
    }
}
#[test]
fn bounds_are_checked_before_encoding_and_small_damage_reduces_work() {
    assert!(Pyramid::new(0, 1080, 3).is_err());
    assert!(Pyramid::new(1920, 1080, 6).is_err());
    let pyramid = Pyramid::new(1920, 1080, 3).unwrap();
    assert!(
        pyramid
            .plan(Damage {
                x: u32::MAX,
                y: 0,
                width: 2,
                height: 1
            })
            .is_err()
    );
    let partial = pyramid
        .plan(Damage {
            x: 100,
            y: 100,
            width: 16,
            height: 16,
        })
        .unwrap();
    let full = pyramid
        .plan(Damage {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        })
        .unwrap();
    let pixels = |plan: &bexos_flatland::kawase::Plan| {
        plan.passes()
            .iter()
            .map(|p| p.damage.width as u64 * p.damage.height as u64)
            .sum::<u64>()
    };
    assert!(pixels(&partial) * 100 < pixels(&full));
}

#[test]
fn source_damage_contains_every_changed_output_pixel() {
    let (width, height) = (129, 65);
    let full = Damage {
        x: 0,
        y: 0,
        width,
        height,
    };
    let original = vec![0; (width * height * 4) as usize];
    for levels in 1..=5 {
        let pyramid = Pyramid::new(width, height, levels).unwrap();
        let before = render(width, height, levels, &original, full, false);
        for (x, y) in [(0, 0), (width / 2, height / 2), (width - 1, height - 1)] {
            let input = Damage {
                x,
                y,
                width: 1,
                height: 1,
            };
            let affected = pyramid.affected_output(input).unwrap();
            let mut changed = original.clone();
            changed[((y * width + x) * 4) as usize..((y * width + x) * 4 + 4) as usize].fill(255);
            let after = render(width, height, levels, &changed, full, false);
            for py in 0..height {
                for px in 0..width {
                    if px < affected.x
                        || py < affected.y
                        || px >= affected.x + affected.width
                        || py >= affected.y + affected.height
                    {
                        let index = ((py * width + px) * 4) as usize;
                        assert_eq!(&before[index..index + 4], &after[index..index + 4]);
                    }
                }
            }
        }
    }
}
