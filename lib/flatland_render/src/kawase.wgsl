// All channels are premultiplied. Clamp-to-edge sampling preserves surface edges.
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
@group(0) @binding(2) var<uniform> target_size: vec4<f32>;

@vertex fn vertex(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let points = array(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
    return vec4<f32>(points[index], 0.0, 1.0);
}
fn sample_at(uv: vec2<f32>, offset: vec2<f32>) -> vec4<f32> {
    return textureSampleLevel(source, linear_sampler, uv + offset / vec2<f32>(textureDimensions(source)), 0.0);
}
@fragment fn downsample(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / target_size.xy;
    return (sample_at(uv, vec2<f32>(0.0)) * 4.0
        + sample_at(uv, vec2<f32>(-1.0, -1.0)) + sample_at(uv, vec2<f32>(1.0, -1.0))
        + sample_at(uv, vec2<f32>(-1.0, 1.0)) + sample_at(uv, vec2<f32>(1.0, 1.0))) / 8.0;
}
@fragment fn upsample(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let uv = position.xy / target_size.xy;
    return (sample_at(uv, vec2<f32>(-2.0, 0.0)) + sample_at(uv, vec2<f32>(2.0, 0.0))
        + sample_at(uv, vec2<f32>(0.0, -2.0)) + sample_at(uv, vec2<f32>(0.0, 2.0))
        + (sample_at(uv, vec2<f32>(-1.0, -1.0)) + sample_at(uv, vec2<f32>(1.0, -1.0))
        + sample_at(uv, vec2<f32>(-1.0, 1.0)) + sample_at(uv, vec2<f32>(1.0, 1.0))) * 2.0) / 12.0;
}
