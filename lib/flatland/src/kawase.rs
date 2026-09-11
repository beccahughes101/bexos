//! Backward dependency bounds for a bounded Dual Kawase pyramid. The final
//! requested output determines every earlier pass's sampling halo, including
//! bilinear footprints. This avoids stale scratch pixels on partial updates.
use crate::{Damage, Error};
pub const MAX_LEVELS: usize = 5;
#[derive(Clone, Copy, Debug, Default)]
pub struct Pass {
    pub source: usize,
    pub target: usize,
    pub downsample: bool,
    pub damage: Damage,
}
pub struct Plan {
    passes: [Pass; MAX_LEVELS * 2],
    count: usize,
    pub source_damage: Damage,
}
impl Plan {
    pub fn passes(&self) -> &[Pass] {
        &self.passes[..self.count]
    }
}
pub struct Pyramid {
    sizes: [(u32, u32); MAX_LEVELS + 1],
    levels: usize,
}
impl Pyramid {
    pub fn new(width: u32, height: u32, levels: usize) -> Result<Self, Error> {
        if width == 0
            || height == 0
            || width > 4096
            || height > 4096
            || !(1..=MAX_LEVELS).contains(&levels)
        {
            return Err(Error::Bounds);
        }
        let mut sizes = [(0, 0); MAX_LEVELS + 1];
        sizes[0] = (width, height);
        for i in 1..=levels {
            sizes[i] = (sizes[i - 1].0.div_ceil(2), sizes[i - 1].1.div_ceil(2));
        }
        Ok(Self { sizes, levels })
    }
    pub fn sizes(&self) -> &[(u32, u32)] {
        &self.sizes[..=self.levels]
    }
    /// Conservative output damage after source pixels change. A compositor
    /// intersects this with the backdrop's visible bounds before encoding.
    pub fn affected_output(&self, input: Damage) -> Result<Damage, Error> {
        // Use the same validation and bounded pass topology as output planning.
        let plan = self.plan(input)?;
        let mut affected = input;
        for pass in plan.passes() {
            // Reverse the sample mapping. Upsampling can magnify a two-texel
            // offset plus bilinear footprint by at most two on either axis.
            affected = sample_bounds(
                affected,
                self.sizes[pass.target],
                self.sizes[pass.source],
                if pass.downsample { 2 } else { 6 },
            );
        }
        Ok(affected)
    }
    pub fn plan(&self, output: Damage) -> Result<Plan, Error> {
        let (width, height) = self.sizes[0];
        if output.width == 0
            || output.height == 0
            || output.x.checked_add(output.width).is_none_or(|v| v > width)
            || output
                .y
                .checked_add(output.height)
                .is_none_or(|v| v > height)
        {
            return Err(Error::Bounds);
        }
        let mut plan = Plan {
            passes: [Pass::default(); MAX_LEVELS * 2],
            count: self.levels * 2,
            source_damage: output,
        };
        for i in 0..self.levels {
            plan.passes[i] = Pass {
                source: i,
                target: i + 1,
                downsample: true,
                damage: Damage::default(),
            };
            plan.passes[self.levels + i] = Pass {
                source: self.levels - i,
                target: self.levels - i - 1,
                downsample: false,
                damage: Damage::default(),
            };
        }
        let mut needed = output;
        for pass in plan.passes[..plan.count].iter_mut().rev() {
            pass.damage = needed;
            // Downsample offsets reach one texel, upsample offsets reach two;
            // one additional texel covers the bilinear interpolation footprint.
            needed = sample_bounds(
                needed,
                self.sizes[pass.source],
                self.sizes[pass.target],
                if pass.downsample { 2 } else { 3 },
            );
        }
        plan.source_damage = needed;
        Ok(plan)
    }
}
fn sample_bounds(rect: Damage, source: (u32, u32), target: (u32, u32), halo: u32) -> Damage {
    let axis = |begin: u32, length: u32, src: u32, dst: u32| {
        let low = (u64::from(begin) * u64::from(src) / u64::from(dst)) as u32;
        let high = (u64::from(begin + length) * u64::from(src)).div_ceil(u64::from(dst)) as u32;
        let low = low.saturating_sub(halo);
        (low, high.saturating_add(halo).min(src) - low)
    };
    let (x, width) = axis(rect.x, rect.width, source.0, target.0);
    let (y, height) = axis(rect.y, rect.height, source.1, target.1);
    Damage {
        x,
        y,
        width,
        height,
    }
}
