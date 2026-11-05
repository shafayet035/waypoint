// Post-processing for stylised looks: the scene arrives in linear colour
// with each pixel's distance in alpha. A blur pyramid (five halvings, then
// back up) gives a soft glow round bright things (bloom) and the blurred
// distance; the composite blends them in, then exposes, tone-maps, grades
// (warm highlights, cool shade, a gentle S-curve) and darkens the corners.

struct Params {
    // x: exposure, y: saturation, z: bloom, w: depth blur.
    a: vec4<f32>,
    // x: vignette, y: focus distance (m), z, w: 1 / output size.
    b: vec4<f32>,
    // Highlight tint (rgb), w: unused.
    highlights: vec4<f32>,
    // Shade tint (rgb), w: unused.
    shade: vec4<f32>,
};

@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var lin: sampler;
@group(0) @binding(2) var<uniform> p: Params;
// Upsampling: the coarser level to add (unused by the other passes).
@group(0) @binding(3) var coarse: texture_2d<f32>;
// Composite: the blur pyramid's finest up level and the quarter-size blur.
@group(0) @binding(4) var bloom_tex: texture_2d<f32>;
@group(0) @binding(5) var blur_tex: texture_2d<f32>;

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

fn tap(t: texture_2d<f32>, uv: vec2<f32>) -> vec3<f32> {
    return textureSampleLevel(t, lin, uv, 0.0).rgb;
}

// Halve: a 13-tap filter (Jimenez 2014) that keeps the pyramid free of
// flicker. Bright single pixels are tamed so they don't sparkle.
@fragment
fn fs_down(in: VsOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(src));
    let uv = in.uv;
    let a = tap(src, uv + t * vec2<f32>(-2.0, -2.0));
    let b = tap(src, uv + t * vec2<f32>(0.0, -2.0));
    let c = tap(src, uv + t * vec2<f32>(2.0, -2.0));
    let d = tap(src, uv + t * vec2<f32>(-2.0, 0.0));
    let e = tap(src, uv);
    let f = tap(src, uv + t * vec2<f32>(2.0, 0.0));
    let g2 = tap(src, uv + t * vec2<f32>(-2.0, 2.0));
    let h = tap(src, uv + t * vec2<f32>(0.0, 2.0));
    let i = tap(src, uv + t * vec2<f32>(2.0, 2.0));
    let j = tap(src, uv + t * vec2<f32>(-1.0, -1.0));
    let k = tap(src, uv + t * vec2<f32>(1.0, -1.0));
    let l = tap(src, uv + t * vec2<f32>(-1.0, 1.0));
    let m = tap(src, uv + t * vec2<f32>(1.0, 1.0));
    var col = e * 0.125 + (a + c + g2 + i) * 0.03125 + (b + d + f + h) * 0.0625
        + (j + k + l + m) * 0.125;
    // Keep the brightest specks from dominating.
    col = col / (1.0 + max(dot(col, vec3<f32>(0.299, 0.587, 0.114)) - 8.0, 0.0) * 0.1);
    return vec4<f32>(col, 1.0);
}

// Double: a 3×3 tent over the coarser level, plus this level.
@fragment
fn fs_up(in: VsOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(coarse));
    let uv = in.uv;
    var c = tap(coarse, uv) * 4.0;
    c += (tap(coarse, uv + vec2<f32>(t.x, 0.0)) + tap(coarse, uv - vec2<f32>(t.x, 0.0))
        + tap(coarse, uv + vec2<f32>(0.0, t.y)) + tap(coarse, uv - vec2<f32>(0.0, t.y))) * 2.0;
    c += tap(coarse, uv + t) + tap(coarse, uv - t) + tap(coarse, uv + vec2<f32>(t.x, -t.y))
        + tap(coarse, uv + vec2<f32>(-t.x, t.y));
    return vec4<f32>(c / 16.0 + tap(src, uv), 1.0);
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn fs_composite(in: VsOut) -> @location(0) vec4<f32> {
    let here = textureSampleLevel(src, lin, in.uv, 0.0);
    var col = here.rgb;
    let dist = here.a;
    // Distance blur: sharp up to about twice the focus distance, then
    // blending into the blurred picture.
    let focus = p.b.y;
    let coc = clamp(p.a.w, 0.0, 2.0) * 0.5
        * smoothstep(focus * 2.2, focus * 2.2 + 220.0, dist);
    col = mix(col, tap(blur_tex, in.uv), clamp(coc, 0.0, 1.0));
    // Bloom: a little of the whole pyramid (five levels added up).
    let glow = tap(bloom_tex, in.uv) / 5.0;
    col = mix(col, glow, clamp(0.06 * p.a.z, 0.0, 0.5));
    // Expose and tone-map (with the look's saturation).
    let luma_in = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
    col = mix(vec3<f32>(luma_in), col, p.a.y) * p.a.x;
    col = aces(col);
    // Grade: shade towards the shade tint, highlights towards theirs, and
    // a gentle S-curve for punch.
    let l = dot(col, vec3<f32>(0.2126, 0.7152, 0.0722));
    let shade_w = 1.0 - smoothstep(0.0, 0.45, l);
    let light_w = smoothstep(0.35, 1.0, l);
    let shade_tint = p.shade.rgb / max(dot(p.shade.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-3);
    let light_tint = p.highlights.rgb / max(dot(p.highlights.rgb, vec3<f32>(0.2126, 0.7152, 0.0722)), 1e-3);
    col = col * mix(vec3<f32>(1.0), shade_tint, 0.35 * shade_w);
    col = col * mix(vec3<f32>(1.0), light_tint, 0.5 * light_w);
    col = mix(col, col * col * (3.0 - 2.0 * col), 0.35);
    // Vignette.
    let d = (in.uv - 0.5) * vec2<f32>(1.0, 0.8);
    col = col * (1.0 - p.b.x * smoothstep(0.3, 0.75, length(d)));
    return vec4<f32>(clamp(col, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
