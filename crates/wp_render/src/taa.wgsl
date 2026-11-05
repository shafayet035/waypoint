// Temporal anti-aliasing and upscaling: each frame is drawn at the internal
// resolution with a sub-pixel offset (jitter) that changes every frame, plus
// how far every pixel moved since the last frame. This pass rebuilds the
// window-sized picture from this frame's samples and the last result, moved
// to where things are now: over a few frames each output pixel gathers many
// samples, which smooths edges and adds detail the internal resolution lacks.
// History that no longer fits (something appeared, or moved unpredictably)
// is clipped to the colours around the pixel this frame, so nothing ghosts.

@group(0) @binding(0) var scene: texture_2d<f32>;
@group(0) @binding(1) var motion_tex: texture_2d<f32>;
@group(0) @binding(2) var history: texture_2d<f32>;
@group(0) @binding(3) var lin: sampler;

struct Params {
    // Internal size and its inverse.
    internal: vec4<f32>,
    // Output size and its inverse.
    output: vec4<f32>,
    // x, y: this frame's jitter in internal pixels (image axes: right, down);
    // z: 1 when the history is usable; w: unused.
    jitter: vec4<f32>,
};
@group(0) @binding(4) var<uniform> p: Params;

struct VsOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let q = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var o: VsOut;
    o.position = vec4<f32>(q * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2<f32>(q.x, 1.0 - q.y);
    return o;
}

fn to_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(
        0.25 * c.r + 0.5 * c.g + 0.25 * c.b,
        0.5 * c.r - 0.5 * c.b,
        -0.25 * c.r + 0.5 * c.g - 0.25 * c.b,
    );
}

fn from_ycocg(c: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(c.x + c.y - c.z, c.x + c.z, c.x - c.y - c.z);
}

// Pull `h` towards the box's centre until it lies inside the box.
fn clip_to_box(h: vec3<f32>, lo: vec3<f32>, hi: vec3<f32>) -> vec3<f32> {
    let centre = 0.5 * (hi + lo);
    let half = 0.5 * (hi - lo) + vec3<f32>(1e-4);
    let off = h - centre;
    let unit = abs(off / half);
    let m = max(unit.x, max(unit.y, unit.z));
    return select(h, centre + off / m, m > 1.0);
}

// The history at `uv`, sharp: a Catmull-Rom filter (16 texels) from nine
// bilinear taps.
fn sample_history(uv: vec2<f32>) -> vec3<f32> {
    let size = p.output.xy;
    let pos = uv * size;
    let centre = floor(pos - 0.5) + 0.5;
    let f = pos - centre;
    let w0 = f * (-0.5 + f * (1.0 - 0.5 * f));
    let w1 = 1.0 + f * f * (-2.5 + 1.5 * f);
    let w2 = f * (0.5 + f * (2.0 - 1.5 * f));
    let w3 = f * f * (-0.5 + 0.5 * f);
    let w12 = w1 + w2;
    let t12 = (centre + w2 / w12) * p.output.zw;
    let t0 = (centre - 1.0) * p.output.zw;
    let t3 = (centre + 2.0) * p.output.zw;
    let xs = array<f32, 3>(t0.x, t12.x, t3.x);
    let ys = array<f32, 3>(t0.y, t12.y, t3.y);
    let wx = array<f32, 3>(w0.x, w12.x, w3.x);
    let wy = array<f32, 3>(w0.y, w12.y, w3.y);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var j = 0; j < 3; j = j + 1) {
        for (var i = 0; i < 3; i = i + 1) {
            let w = wx[i] * wy[j];
            sum += textureSampleLevel(history, lin, vec2<f32>(xs[i], ys[j]), 0.0).rgb * w;
            weight += w;
        }
    }
    return max(sum / max(weight, 1e-4), vec3<f32>(0.0));
}

// Sharpness of the reconstruction from this frame's samples (1/2σ², σ in
// internal pixels).
const KERNEL: f32 = 8.0;
// How much more this frame counts per output pixel of motion.
const SPEED_TRUST: f32 = 0.25;

struct FsOut {
    @location(0) color: vec4<f32>,
    @location(1) history: vec4<f32>,
};

@fragment
fn fs_main(in: VsOut) -> FsOut {
    // This output pixel's centre on the internal grid, where texel t was
    // sampled at t + 0.5 − jitter.
    let q = in.uv * p.internal.xy;
    let jitter = p.jitter.xy;
    let base = floor(q + jitter);
    // Fewer samples than pixels (upscaling): reach further for them.
    let ratio = p.internal.x * p.output.z;
    let kernel = KERNEL * ratio * ratio;
    let last = vec2<i32>(p.internal.xy) - vec2<i32>(1);
    var sum = vec3<f32>(0.0);
    var wsum = 0.0;
    var m1 = vec3<f32>(0.0);
    var m2 = vec3<f32>(0.0);
    var closest = 0.0;
    var mv = vec2<f32>(0.0);
    var best = -1.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            let t = base + vec2<f32>(f32(x), f32(y));
            let texel = clamp(vec2<i32>(t), vec2<i32>(0), last);
            let c = textureLoad(scene, texel, 0).rgb;
            let d = t + 0.5 - jitter - q;
            // A narrow Gaussian: over the jittered frames the samples nearest
            // each pixel average to a box over it (what supersampling gives).
            let w = exp(-kernel * dot(d, d));
            sum += c * w;
            wsum += w;
            closest = max(closest, w);
            let yc = to_ycocg(c);
            m1 += yc;
            m2 += yc * yc;
            // The motion of the sample nearest this pixel.
            if (w > best) {
                best = w;
                mv = textureLoad(motion_tex, texel, 0).xy;
            }
        }
    }
    let current = sum / max(wsum, 1e-4);
    let prev_uv = in.uv - mv;
    let inside = all(prev_uv >= vec2<f32>(0.0)) && all(prev_uv <= vec2<f32>(1.0));
    var result = current;
    if (p.jitter.z > 0.5 && inside) {
        let mean = m1 / 9.0;
        let sigma = sqrt(max(m2 / 9.0 - mean * mean, vec3<f32>(0.0)));
        let lo = mean - 1.25 * sigma;
        let hi = mean + 1.25 * sigma;
        var hist = from_ycocg(clip_to_box(to_ycocg(sample_history(prev_uv)), lo, hi));
        if (p.jitter.w < 0.0) { hist = sample_history(prev_uv); }
        // Trust this frame more where one of its samples fell close to the
        // pixel, and where things move fast (history is blurrier there).
        let speed = length(mv * p.output.xy);
        var alpha = clamp(0.04 + 0.12 * closest + SPEED_TRUST * speed, 0.04, 0.6);
        if (p.jitter.w != 0.0) { alpha = abs(p.jitter.w); }
        result = mix(hist, current, alpha);
    }
    return FsOut(vec4<f32>(result, 1.0), vec4<f32>(result, 1.0));
}
