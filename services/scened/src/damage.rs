//! Compare resolved paint order, retaining storage across frames. Unfenced
//! transactions can change pixels without changing geometry or buffer identity.
use bexos_graphics::{Damage, resolved::Item};
#[derive(Default)]
pub struct Tracker {
    items: Vec<(u64, Item)>,
}
impl Tracker {
    pub fn update(
        &mut self,
        items: impl Iterator<Item = (u64, Item)>,
        mutable: impl Fn(u64) -> bool,
    ) -> Option<Damage> {
        let mut damage = None;
        let mut add = |bounds: Damage| {
            if bounds.width != 0 && bounds.height != 0 {
                damage = Some(damage.map_or(bounds, |d: Damage| d.union(bounds)));
            }
        };
        let mut count = 0;
        for new in items {
            if let Some(old) = self.items.get_mut(count) {
                if *old != new || mutable(new.0) {
                    add(old.1.pixels);
                    add(new.1.pixels);
                }
                *old = new;
            } else {
                add(new.1.pixels);
                self.items.push(new);
            }
            count += 1;
        }
        for old in &self.items[count..] {
            add(old.1.pixels);
        }
        self.items.truncate(count);
        damage
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use bexos_graphics::{
        Format, Surface,
        resolved::{Rect, Transform},
    };
    fn item(node: u64, x: u32, width: u32, height: u32) -> Item {
        Item {
            node,
            buffer: node,
            surface: Surface {
                width,
                height,
                stride: width * 4,
                format: Format::Bgra,
            },
            transform: Transform {
                x: x as f64,
                ..Transform::default()
            },
            inverse_scale: (1., 1.),
            opacity: 1.,
            visible: Rect {
                x: x as f64,
                y: 0.,
                width: width as f64,
                height: height as f64,
            },
            pixels: Damage {
                x,
                y: 0,
                width,
                height,
            },
            effects: Default::default(),
        }
    }
    #[test]
    fn moving_child_repairs_only_old_and_new_pixels() {
        let mut t = Tracker::default();
        let base = item(1, 0, 800, 600);
        t.update([(1, base), (1, item(2, 100, 64, 64))].into_iter(), |_| {
            false
        });
        assert_eq!(
            t.update([(1, base), (1, item(2, 101, 64, 64))].into_iter(), |_| {
                false
            }),
            Some(Damage {
                x: 100,
                y: 0,
                width: 65,
                height: 64
            })
        );
        assert_eq!(
            t.update([(1, base), (1, item(2, 101, 64, 64))].into_iter(), |_| {
                false
            }),
            None
        );
        assert_eq!(
            t.update([(1, base), (1, item(2, 101, 64, 64))].into_iter(), |_| true),
            Some(base.pixels)
        );
    }
    #[test]
    fn removal_and_paint_reordering_damage_exposed_pixels() {
        let mut t = Tracker::default();
        let a = item(1, 10, 64, 64);
        let b = item(2, 40, 64, 64);
        t.update([(1, a), (2, b)].into_iter(), |_| false);
        assert_eq!(
            t.update([(2, b), (1, a)].into_iter(), |_| false),
            Some(a.pixels.union(b.pixels))
        );
        assert_eq!(t.update([(2, b)].into_iter(), |_| false), Some(a.pixels));
    }
}
