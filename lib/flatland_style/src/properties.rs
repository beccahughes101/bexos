//! Projection of computed CSS onto Flatland's public content/layout properties.
//! This API deliberately rejects dimensions it cannot express faithfully.
use crate::Error;
use bexos_flatland::{effects::Effects, layout::Properties};
use style::{
    properties::{ComputedValues, longhands::flex_direction::computed_value::T as FlexDirection},
    values::{
        computed::{NonNegativeLengthPercentage, Size},
        generics::length::GenericLengthPercentageOrNormal,
        specified::box_::DisplayInside,
    },
};
fn pixels(value: &NonNegativeLengthPercentage) -> Result<f32, Error> {
    value
        .0
        .to_length()
        .map(|v| v.px())
        .ok_or(Error::UnsupportedLayout)
}
fn size(value: Size, current: f32) -> Result<f32, Error> {
    match value {
        Size::Auto => Ok(current),
        Size::LengthPercentage(v) => pixels(&v),
        _ => Err(Error::UnsupportedLayout),
    }
}
pub fn apply(
    computed: &ComputedValues,
    layout: &mut Properties,
    opacity: &mut f32,
    effects: &mut Effects,
) -> Result<(), Error> {
    let (mut next_layout, mut next_opacity, mut next_effects) = (*layout, *opacity, *effects);
    project(
        computed,
        &mut next_layout,
        &mut next_opacity,
        &mut next_effects,
    )?;
    next_layout
        .validate()
        .map_err(|_| Error::UnsupportedLayout)?;
    next_effects
        .validate()
        .map_err(|_| Error::UnsupportedLayout)?;
    if !next_opacity.is_finite() || !(0. ..=1.).contains(&next_opacity) {
        return Err(Error::UnsupportedLayout);
    }
    (*layout, *opacity, *effects) = (next_layout, next_opacity, next_effects);
    Ok(())
}
fn project(
    computed: &ComputedValues,
    layout: &mut Properties,
    opacity: &mut f32,
    effects: &mut Effects,
) -> Result<(), Error> {
    let position = computed.get_position();
    layout.width = size(position.clone_width(), layout.width)?;
    layout.height = size(position.clone_height(), layout.height)?;
    layout.mode = match computed.get_box().clone_display().inside() {
        DisplayInside::None => 4,
        DisplayInside::Grid => 3,
        DisplayInside::Flex => match position.clone_flex_direction() {
            FlexDirection::Row => 1,
            FlexDirection::Column => 2,
            FlexDirection::RowReverse => 5,
            FlexDirection::ColumnReverse => 6,
        },
        _ => return Err(Error::UnsupportedLayout),
    };
    layout.grow = position.clone_flex_grow().0;
    if layout.mode == 3 {
        layout.columns =
            crate::grid::columns(&position.clone_grid_template_columns(), layout.columns)?;
    }
    let gap = |value| match value {
        GenericLengthPercentageOrNormal::Normal => Ok(0.),
        GenericLengthPercentageOrNormal::LengthPercentage(v) => pixels(&v),
    };
    let column_gap = gap(position.clone_column_gap())?;
    let row_gap = gap(position.clone_row_gap())?;
    if column_gap != row_gap {
        return Err(Error::UnsupportedLayout);
    }
    layout.gap = column_gap;
    let padding = computed.get_padding();
    let sides = [
        padding.clone_padding_top(),
        padding.clone_padding_right(),
        padding.clone_padding_bottom(),
        padding.clone_padding_left(),
    ];
    let p = pixels(&sides[0])?;
    for side in &sides[1..] {
        if pixels(side)? != p {
            return Err(Error::UnsupportedLayout);
        }
    }
    layout.padding = p;
    *opacity *= computed.get_effects().clone_opacity();
    if layout.mode == 4 {
        *opacity = 0.;
    }
    let border = computed.get_border();
    let corners = [
        border.clone_border_top_left_radius(),
        border.clone_border_top_right_radius(),
        border.clone_border_bottom_right_radius(),
        border.clone_border_bottom_left_radius(),
    ];
    let radius = pixels(&corners[0].0.width)?;
    for corner in &corners {
        if pixels(&corner.0.width)? != radius || pixels(&corner.0.height)? != radius {
            return Err(Error::UnsupportedLayout);
        }
    }
    if radius != 0. {
        effects.corner_radius = radius;
    }
    Ok(())
}
