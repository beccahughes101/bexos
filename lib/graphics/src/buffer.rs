use crate::{Damage, Error, Surface};
/// Repair a recycled buffer using its own accumulated damage, then apply the
/// current update. The caller retains damage independently for each resource.
pub fn update_tracked_back_buffer(
    target: Surface,
    front: &[u8],
    back: &mut [u8],
    source: Surface,
    pixels: &[u8],
    stale: Option<Damage>,
    damage: Damage,
) -> Result<Damage, Error> {
    target.validate(front.len() as u64)?;
    target.validate(back.len() as u64)?;
    source.validate(pixels.len() as u64)?;
    damage.validate(source)?;
    if target.width != source.width || target.height != source.height {
        return Err(Error::Invalid);
    }
    if let Some(stale) = stale {
        stale.validate(target)?;
        for y in stale.y..stale.y + stale.height {
            let start = (y * target.stride + stale.x * 4) as usize;
            let end = start + stale.width as usize * 4;
            back[start..end].copy_from_slice(&front[start..end]);
        }
    }
    for y in damage.y..damage.y + damage.height {
        let start = (y * target.stride + damage.x * 4) as usize;
        let input = (y * source.stride + damage.x * 4) as usize;
        let len = damage.width as usize * 4;
        let row = &mut back[start..start + len];
        row.copy_from_slice(&pixels[input..input + len]);
        if source.format != target.format {
            for pixel in row.chunks_exact_mut(4) {
                let converted = source
                    .format
                    .convert([pixel[0], pixel[1], pixel[2], pixel[3]], target.format);
                pixel.copy_from_slice(&converted);
            }
        }
    }
    Ok(stale.map_or(damage, |s| s.union(damage)))
}
/// Recycle a scanout back buffer and return the region whose device copy is stale.
/// Comparing against this buffer (not just the latest front) handles alternating resources.
pub fn update_back_buffer(
    target: Surface,
    front: &[u8],
    back: &mut [u8],
    source: Surface,
    pixels: &[u8],
    damage: Damage,
) -> Result<Option<Damage>, Error> {
    target.validate(front.len() as u64)?;
    target.validate(back.len() as u64)?;
    source.validate(pixels.len() as u64)?;
    damage.validate(source)?;
    if target.width != source.width || target.height != source.height {
        return Err(Error::Invalid);
    }
    let mut dirty: Option<Damage> = None;
    for y in 0..target.height {
        let row = y as usize * target.stride as usize;
        let source_row = y as usize * source.stride as usize;
        for x in 0..target.width {
            let offset = row + x as usize * 4;
            let mut pixel: [u8; 4] = front[offset..offset + 4].try_into().unwrap();
            if y >= damage.y
                && y < damage.y + damage.height
                && x >= damage.x
                && x < damage.x + damage.width
            {
                let offset = source_row + x as usize * 4;
                pixel.copy_from_slice(&pixels[offset..offset + 4]);
                if source.format != target.format {
                    pixel = source.format.convert(pixel, target.format);
                }
            }
            if back[offset..offset + 4] != pixel {
                let point = Damage {
                    x,
                    y,
                    width: 1,
                    height: 1,
                };
                dirty = Some(dirty.map_or(point, |d| d.union(point)));
                back[offset..offset + 4].copy_from_slice(&pixel);
            }
        }
    }
    Ok(dirty)
}
