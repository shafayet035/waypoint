// Upscale: draw the scene texture (rendered below window resolution) over
// the whole target: Catmull-Rom, then sharpening clamped to the nearest pixels.

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var scene_sampler: sampler;

struct Out {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> Out {
    // One triangle covering the screen.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.position = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(p.x, 1.0 - p.y);
    return o;
}

// Catmull-Rom bicubic from nine bilinear taps: as cheap as a few lookups,
// much sharper than bilinear stretching.
@fragment
fn fs_main(v: Out) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(scene));
    let texel = 1.0 / size;
    let pos = v.uv * size;
    let centre = floor(pos - 0.5) + 0.5;
    let f = pos - centre;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let t12 = (centre + w2 / w12) * texel;
    let t0 = (centre - 1.0) * texel;
    let t3 = (centre + 2.0) * texel;
    let xs = array<f32, 3>(t0.x, t12.x, t3.x);
    let ys = array<f32, 3>(t0.y, t12.y, t3.y);
    let wx = array<f32, 3>(w0.x, w12.x, w3.x);
    let wy = array<f32, 3>(w0.y, w12.y, w3.y);
    var sum = vec3<f32>(0.0);
    for (var j = 0; j < 3; j = j + 1) {
        for (var i = 0; i < 3; i = i + 1) {
            sum += textureSampleLevel(scene, scene_sampler, vec2<f32>(xs[i], ys[j]), 0.0).rgb
                * (wx[i] * wy[j]);
        }
    }
    // Sharpen: push away from the softer bilinear value, but never past the
    // four nearest pixels (no halos round edges).
    let soft = textureSampleLevel(scene, scene_sampler, v.uv, 0.0).rgb;
    let base = vec2<i32>(floor(pos - 0.5));
    let last = vec2<i32>(size) - vec2<i32>(1);
    let a = textureLoad(scene, clamp(base, vec2<i32>(0), last), 0).rgb;
    let b = textureLoad(scene, clamp(base + vec2<i32>(1, 0), vec2<i32>(0), last), 0).rgb;
    let c = textureLoad(scene, clamp(base + vec2<i32>(0, 1), vec2<i32>(0), last), 0).rgb;
    let d = textureLoad(scene, clamp(base + vec2<i32>(1, 1), vec2<i32>(0), last), 0).rgb;
    let lo = min(min(a, b), min(c, d));
    let hi = max(max(a, b), max(c, d));
    let sharp = clamp(sum + SHARPEN * (sum - soft), lo, hi);
    return vec4<f32>(sharp, 1.0);
}

// How strongly the upscale sharpens.
const SHARPEN: f32 = 1.0;
