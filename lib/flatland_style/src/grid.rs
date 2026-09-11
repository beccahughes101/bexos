//! Bounded projection of equal fractional CSS tracks into Flatland's grid.
use crate::Error;
use style::values::{
    computed::{GridTemplateComponent, LengthPercentage},
    generics::grid::{RepeatCount, TrackBreadth, TrackListValue, TrackSize},
};

pub fn columns(value: &GridTemplateComponent, fallback: u32) -> Result<u32, Error> {
    let list = match value {
        GridTemplateComponent::None => return Ok(fallback),
        GridTemplateComponent::TrackList(list) => list,
        _ => return Err(Error::UnsupportedLayout),
    };
    let mut fraction = None;
    let mut count = 0u32;
    let mut add = |size: &TrackSize<LengthPercentage>, repeat: u32| -> Result<(), Error> {
        let TrackSize::Breadth(TrackBreadth::Flex(value)) = size else {
            return Err(Error::UnsupportedLayout);
        };
        if !value.0.is_finite() || value.0 <= 0. || fraction.is_some_and(|f| f != value.0) {
            return Err(Error::UnsupportedLayout);
        }
        fraction = Some(value.0);
        count = count
            .checked_add(repeat)
            .filter(|v| (1..=64).contains(v))
            .ok_or(Error::UnsupportedLayout)?;
        Ok(())
    };
    for track in list.values.iter() {
        match track {
            TrackListValue::TrackSize(size) => add(size, 1)?,
            TrackListValue::TrackRepeat(repeat) => {
                let RepeatCount::Number(n) = repeat.count else {
                    return Err(Error::UnsupportedLayout);
                };
                let n = u32::try_from(n)
                    .ok()
                    .filter(|n| (1..=64).contains(n))
                    .ok_or(Error::UnsupportedLayout)?;
                for size in repeat.track_sizes.iter() {
                    add(size, n)?;
                }
            }
        }
    }
    if count == 0 {
        return Err(Error::UnsupportedLayout);
    }
    Ok(count)
}
