// Waypoint M0 frame pipeline: sky, ground, vehicles.
// Coordinates: Z up, X east, Y north, metres. Shaders output linear colour;
// the render target is sRGB, so the GPU encodes on write.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    camera_pos: vec4<f32>,
    // xyz: direction towards the sun, w: intensity.
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
    sky_zenith: vec4<f32>,
    sky_horizon: vec4<f32>,
    ground_base: vec4<f32>,
    ground_line: vec4<f32>,
    // x: fog density, y: shadow count, z: exposure, w: unused.
    params: vec4<f32>,
    // Blob shadows: centre xy, half extents zw.
    shadows: array<vec4<f32>, 16>,
    // cos(heading), sin(heading), strength, softness.
    shadow_rot: array<vec4<f32>, 16>,
    // Sun shadow map: camera-relative position → light clip space.
    light_view_proj: mat4x4<f32>,
    // x: 1 / shadow map size, y: 1 when the shadow map is on, z: darkness
    // (0 day – 1 night; lights windows), w: unused.
    shadow_params: vec4<f32>,
    // Local lights, three vec4 each: camera-relative position + range,
    // colour × intensity + spot cosine (< −1: omni), spot direction.
    // params.w holds the count.
    lights: array<vec4<f32>, 192>,
    // Art style: x toon bands (0 = smooth), y saturation, z ground grid (0/1), w flat (0/1).
    style: vec4<f32>,
    // Windscreen (cab view in the rain): [0] bottom-left corner (camera-relative)
    // + width, [1] unit across + height, [2] unit up + rain, [3] wiper pivots
    // (glass m), [4] time, cycle, stroke (s), arm (m), [5] mirrored, park and
    // top angles (rad), share of the arm the blade covers from.
    glass: array<vec4<f32>, 6>,
    // x: road wetness (0–1), y: rain (0–1), z: simulated time (s),
    // w: ground fog extinction per m (0: no fog).
    weather: vec4<f32>,
    // Street reflections in wet roads: x, y: 1 / the target's size (screen
    // position → texture coordinates), z: 1 when this frame has them.
    reflect_params: vec4<f32>,
    // Stylised looks: x: light wrap (0: plain Lambert), y: height scale of
    // ground fog and haze (m), z: 1 to write linear colour for the
    // post-processing chain, w: height of the flat ground plane (m).
    look: vec4<f32>,
    // Tint of sky light in shade (rgb; white: none).
    shade: vec4<f32>,
};

// Sunlight on a surface facing `ndotl`: Lambert, or wrapped past the
// terminator for a softer form in stylised looks.
fn lambert(ndotl: f32) -> f32 {
    let w = g.look.x;
    return select(max(ndotl, 0.0), clamp((ndotl + w) / (1.0 + w), 0.0, 1.0), w > 0.0);
}

// Tone-mapped colour, or linear (with the distance in alpha) when the
// post-processing chain does the rest.
fn finish(col: vec3<f32>, rel: vec3<f32>) -> vec4<f32> {
    if (g.look.z > 0.5) {
        return vec4<f32>(col, length(rel));
    }
    return vec4<f32>(tonemap(col), 1.0);
}

// Light falling on a surface (0–1) in the game's style: smooth, in hard toon
// bands, or wrapped softly round the shape for the flat style.
fn styled_light(x: f32) -> f32 {
    if (g.style.x > 0.5) {
        let b = g.style.x;
        return clamp(ceil(x * b - 0.15) / b, 0.0, 1.0);
    }
    if (g.style.w > 0.5) {
        return x * 0.6 + 0.4 * step(0.001, x);
    }
    return x;
}

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var shadow_map: texture_depth_2d;
@group(0) @binding(2) var shadow_sampler: sampler_comparison;
// The scene mirrored in the road plane (linear colour), for wet roads.
@group(0) @binding(3) var reflection_tex: texture_2d<f32>;
@group(0) @binding(4) var reflection_sampler: sampler;

// What a wet road at screen position `frag` mirrors: the street drawn upside
// down into the reflection texture, smeared down the screen as rough water
// smears it, more so the rougher the surface.
fn street_reflection(frag: vec2<f32>, rough: f32) -> vec3<f32> {
    let uv = frag * g.reflect_params.xy;
    let spread = 0.05 * rough;
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var k = -2; k <= 2; k = k + 1) {
        let w = 3.0 - abs(f32(k));
        let at = clamp(uv + vec2<f32>(0.0, f32(k) * spread), vec2<f32>(0.0), vec2<f32>(1.0));
        sum += textureSampleLevel(reflection_tex, reflection_sampler, at, 0.0).rgb * w;
        weight += w;
    }
    return sum / weight;
}
// The mesh's material maps (neutral when it has none): base colour (sRGB),
// tangent-space normal, metallic-roughness (G roughness, B metallic),
// emissive (sRGB), occlusion (R); strengths in mat_params.
struct MaterialParams {
    // x: normal strength (0 = none), y: occlusion strength, z: emissive from map (0/1).
    params: vec4<f32>,
};
@group(1) @binding(0) var base_tex: texture_2d<f32>;
@group(1) @binding(1) var normal_tex: texture_2d<f32>;
@group(1) @binding(2) var mr_tex: texture_2d<f32>;
@group(1) @binding(3) var emissive_tex: texture_2d<f32>;
@group(1) @binding(4) var occlusion_tex: texture_2d<f32>;
@group(1) @binding(5) var base_sampler: sampler;
@group(1) @binding(6) var<uniform> mat: MaterialParams;

// Normal mapping without stored tangents: build the tangent frame from
// screen-space derivatives of position and texture coordinates.
fn perturb_normal(n: vec3<f32>, p: vec3<f32>, uv: vec2<f32>, map: vec3<f32>, strength: f32) -> vec3<f32> {
    let dp1 = dpdx(p);
    let dp2 = dpdy(p);
    let duv1 = dpdx(uv);
    let duv2 = dpdy(uv);
    let dp2perp = cross(dp2, n);
    let dp1perp = cross(n, dp1);
    let t = dp2perp * duv1.x + dp1perp * duv2.x;
    // glTF normal maps point +Y up the image; texture v runs down it.
    let b = -(dp2perp * duv1.y + dp1perp * duv2.y);
    let scale = inverseSqrt(max(max(dot(t, t), dot(b, b)), 1e-12));
    let m = vec3<f32>((map.xy * 2.0 - 1.0) * strength, map.z * 2.0 - 1.0);
    let bent = normalize(t * scale * m.x + b * scale * m.y + n * m.z);
    return select(n, bent, strength > 0.0);
}

// Sun visibility from the shadow map, 0 (shadowed) to 1 (lit), with a 3×3
// filter for soft edges. Outside the map, everything is lit; shadows fade
// out towards its edge so the boundary never shows.
fn sun_shadow(rel: vec3<f32>, n: vec3<f32>) -> f32 {
    if (g.shadow_params.y < 0.5) {
        return 1.0;
    }
    let c = g.light_view_proj * vec4<f32>(rel + n * 0.04, 1.0);
    let ndc = c.xyz / c.w;
    let uv = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0)) || ndc.z > 1.0 || ndc.z < 0.0) {
        return 1.0;
    }
    let t = g.shadow_params.x;
    var sum = 0.0;
    for (var y = -1; y <= 1; y = y + 1) {
        for (var x = -1; x <= 1; x = x + 1) {
            sum = sum + textureSampleCompareLevel(
                shadow_map, shadow_sampler, uv + vec2<f32>(f32(x), f32(y)) * t, ndc.z - 0.0008);
        }
    }
    let edge = max(abs(ndc.x), abs(ndc.y));
    return mix(sum / 9.0, 1.0, smoothstep(0.8, 1.0, edge));
}

fn tonemap(c: vec3<f32>) -> vec3<f32> {
    // ACES filmic fit (Narkowicz 2015).
    let luma = dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
    let x = mix(vec3<f32>(luma), c, g.style.y) * g.params.z;
    return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let up = clamp(dir.z, 0.0, 1.0);
    var col = mix(g.sky_horizon.rgb, g.sky_zenith.rgb, pow(up, 0.45));
    let below = clamp(-dir.z * 6.0, 0.0, 1.0);
    col = mix(col, g.sky_horizon.rgb * 0.75, below);
    let s = max(dot(dir, g.sun_dir.xyz), 0.0);
    col += g.sun_color.rgb * (pow(s, 900.0) * 30.0 + pow(s, 12.0) * 0.3);
    return col;
}

fn ambient(n: vec3<f32>) -> vec3<f32> {
    let bounce = g.ground_base.rgb * g.sun_color.rgb * g.sun_dir.w * 0.35;
    // The whole sky dome lights a surface, not just the zenith: average it.
    let sky = mix(g.sky_horizon.rgb, g.sky_zenith.rgb, 0.55);
    return mix(bounce, sky * 0.95, n.z * 0.5 + 0.5) * g.shade.rgb;
}

// `rel`: position relative to the camera (all geometry is drawn camera-relative,
// so big maps keep their precision).
fn apply_fog(col: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let d = length(rel);
    if (g.weather.w > 0.0) {
        return in_fog(col, rel, d);
    }
    let f = 1.0 - exp(-d * g.params.x);
    return mix(col, g.sky_horizon.rgb, f);
}

// ---- Weather fog -------------------------------------------------------------
// Ground fog thins with height (scale `g.look.y`, m), so the fog along a ray has
// a closed form. It is lit by the sky, by the sun (scattered mostly forwards:
// a glow round the sun) and by every local light: headlamp beams and halos
// round street lamps, integrated along the view ray.

const FOG_LIGHT_STEPS: u32 = 8u;
// Lamps are far brighter than their light on surfaces suggests (most of it
// misses what they light); this brings their glow in fog up to that.
const FOG_GLOW: f32 = 4.0;

// Henyey-Greenstein phase function: how much light turning by an angle with
// cosine `c` scatters, for forward bias `k`.
fn phase(c: f32, k: f32) -> f32 {
    let d = 1.0 + k * k - 2.0 * k * c;
    return (1.0 - k * k) / (12.566 * d * sqrt(d));
}

// Fog extinction at world height `z`, per m.
fn fog_at(z: f32) -> f32 {
    return g.weather.w * exp(-max(z, 0.0) / g.look.y);
}

// How much haze and ground fog lie between the camera and `rel` (`d` = its length).
fn optical_depth(rel: vec3<f32>, d: f32) -> f32 {
    let k = rel.z / g.look.y;
    // Mean of exp(-z/H) along the ray, relative to the camera's height.
    let mean = select((1.0 - exp(-k)) / k, 1.0 - 0.5 * k, abs(k) < 1e-3);
    return d * (g.params.x + fog_at(g.camera_pos.z) * mean);
}

// Local lights scattered towards the camera by the fog along the ray `v`,
// out to `len` m. Each light's stretch of the ray is cut into pieces; within a
// piece the 1/(d² + 1) falloff is integrated exactly (so a lamp's bright core
// never bands) and the beam shape, range and phase are taken at its middle.
fn fog_lights(v: vec3<f32>, len: f32) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    let count = u32(g.params.w);
    for (var i = 0u; i < count; i = i + 1u) {
        let a = g.lights[i * 3u];
        let b = g.lights[i * 3u + 1u];
        let tc = dot(a.xyz, v);
        let h2 = max(dot(a.xyz, a.xyz) - tc * tc, 0.0);
        if (h2 >= a.w * a.w) {
            continue;
        }
        let half_chord = sqrt(a.w * a.w - h2);
        let t0 = max(tc - half_chord, 0.0);
        let t1 = min(tc + half_chord, len);
        if (t1 <= t0) {
            continue;
        }
        let dir = g.lights[i * 3u + 2u].xyz;
        let hh = sqrt(h2 + 1.0);
        let piece = (t1 - t0) / f32(FOG_LIGHT_STEPS);
        var acc = 0.0;
        for (var k = 0u; k < FOG_LIGHT_STEPS; k = k + 1u) {
            let s0 = t0 + piece * f32(k);
            let mid = s0 + piece * 0.5;
            let falloff = (atan((s0 + piece - tc) / hh) - atan((s0 - tc) / hh)) / hh;
            let p = v * mid;
            let to_p = p - a.xyz;
            let dist = length(to_p);
            let l = to_p / max(dist, 1e-3);
            let x = dist / a.w;
            let window = clamp(1.0 - x * x * x * x, 0.0, 1.0);
            var spot = 1.0;
            if (b.w > -1.5) {
                spot = smoothstep(b.w, b.w + 0.1, dot(l, dir));
            }
            let dens = fog_at(g.camera_pos.z + p.z);
            acc += falloff * window * window * spot * phase(dot(l, -v), 0.5) * dens * exp(-dens * mid);
        }
        sum += b.rgb * acc * FOG_GLOW;
    }
    return sum;
}

fn in_fog(col: vec3<f32>, rel: vec3<f32>, d: f32) -> vec3<f32> {
    let v = rel / max(d, 1e-4);
    let f = 1.0 - exp(-optical_depth(rel, d));
    let sun = g.sun_color.rgb * g.sun_dir.w * phase(dot(v, g.sun_dir.xyz), 0.6) * 0.5;
    return mix(col, g.sky_horizon.rgb + sun, f) + fog_lights(v, d);
}

// Headlamps, tail lights and street lamps: Lambert diffuse plus a Blinn
// highlight (bright on wet, glossy asphalt), with a smooth range falloff.
fn local_lights(rel: vec3<f32>, n: vec3<f32>, v: vec3<f32>, albedo: vec3<f32>, rough: f32) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    let count = u32(g.params.w);
    let shininess = mix(512.0, 6.0, rough);
    for (var i = 0u; i < count; i = i + 1u) {
        let a = g.lights[i * 3u];
        let b = g.lights[i * 3u + 1u];
        let d = a.xyz - rel;
        let dist = length(d);
        if (dist >= a.w) {
            continue;
        }
        let l = d / max(dist, 1e-4);
        let x = dist / a.w;
        let window = clamp(1.0 - x * x * x * x, 0.0, 1.0);
        var att = window * window / (dist * dist + 1.0);
        if (b.w > -1.5) {
            let dir = g.lights[i * 3u + 2u].xyz;
            att = att * smoothstep(b.w, b.w + 0.1, dot(-l, dir));
        }
        let ndl = max(dot(n, l), 0.0);
        let h = normalize(l + v);
        let spec = pow(max(dot(n, h), 0.0), shininess) * (shininess + 8.0) / 25.0 * (1.0 - rough * 0.8);
        sum = sum + b.rgb * att * ndl * (albedo + vec3<f32>(spec * 0.04));
    }
    return sum;
}

// ---- Sky: a fullscreen triangle -----------------------------------------

struct SkyOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) i: u32) -> SkyOut {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    let p = uv * 2.0 - 1.0;
    var out: SkyOut;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

fn shade_sky(ndc: vec2<f32>) -> vec3<f32> {
    // A point on the near plane (depth 1 with reversed-Z), seen from the camera.
    let p = g.inv_view_proj * vec4<f32>(ndc, 1.0, 1.0);
    let dir = normalize(p.xyz / p.w);
    var col = sky_color(dir);
    if (g.weather.w > 0.0) {
        col = in_fog(col, dir * 4000.0, 4000.0);
    }
    return col;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    // The sky is as far as anything gets.
    return finish(shade_sky(in.ndc), vec3<f32>(0.0, 0.0, 1.0e4));
}

// The mirrored pass keeps linear colour (the road shader tone-maps it).
@fragment
fn fs_sky_mirror(in: SkyOut) -> @location(0) vec4<f32> {
    return vec4<f32>(shade_sky(in.ndc), 1.0);
}

// ---- Ground: a large quad that follows the camera ------------------------

struct GroundOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) rel: vec3<f32>,
};

@vertex
fn vs_ground(@builtin(vertex_index) i: u32) -> GroundOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0),
    );
    let c = corners[i] * 2000.0;
    var out: GroundOut;
    // look.w: the plane's height (below any hilly land; 0 otherwise).
    out.rel = vec3<f32>(c, g.look.w - g.camera_pos.z);
    out.world = vec3<f32>(c + g.camera_pos.xy, g.look.w);
    out.pos = g.view_proj * vec4<f32>(out.rel, 1.0);
    return out;
}

fn hash2(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn value_noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash2(i);
    let b = hash2(i + vec2<f32>(1.0, 0.0));
    let c = hash2(i + vec2<f32>(0.0, 1.0));
    let d = hash2(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Anti-aliased grid lines: `spacing` metres apart, `width` metres wide.
fn grid(p: vec2<f32>, spacing: f32, width: f32) -> f32 {
    let d = abs(fract(p / spacing - 0.5) - 0.5) * spacing;
    let fw = max(fwidth(p), vec2<f32>(1e-4));
    let l = 1.0 - smoothstep(vec2<f32>(width * 0.5), vec2<f32>(width * 0.5) + fw * 1.5, d);
    // Fade lines out where they would alias into noise.
    let fade = clamp(1.0 - max(fw.x, fw.y) / (spacing * 0.15), 0.0, 1.0);
    return max(l.x, l.y) * fade;
}

fn blob_shadows(p: vec2<f32>) -> f32 {
    var lit = 1.0;
    let count = u32(g.params.y);
    for (var k = 0u; k < count; k = k + 1u) {
        let s = g.shadows[k];
        let r = g.shadow_rot[k];
        let d = p - s.xy;
        let local = vec2<f32>(d.x * r.x + d.y * r.y, -d.x * r.y + d.y * r.x);
        let q = abs(local) - s.zw;
        let sd = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
        let a = 1.0 - smoothstep(-r.w, r.w, sd);
        lit = min(lit, 1.0 - a * r.z);
    }
    return lit;
}

@fragment
fn fs_ground(in: GroundOut) -> @location(0) vec4<f32> {
    let p = in.world.xy;
    // Asphalt: smooth large-scale variation plus fine aggregate that fades with distance.
    let coarse = value_noise(p * 0.15) * 0.5 + value_noise(p * 0.6) * 0.5;
    let fine_fade = clamp(1.0 - length(fwidth(p)) * 8.0, 0.0, 1.0);
    let fine = mix(0.5, hash2(floor(p * 12.0)), fine_fade);
    var base = g.ground_base.rgb * (0.82 + 0.22 * coarse + 0.12 * (fine - 0.5));
    base = mix(base, g.ground_line.rgb, grid(p, 10.0, 0.06) * 0.35 * g.style.z);
    base = mix(base, g.ground_line.rgb, grid(p, 50.0, 0.14) * 0.5 * g.style.z);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let ndl = lambert(dot(n, g.sun_dir.xyz));
    let blob = blob_shadows(p);
    // With a shadow map, blobs only darken contact areas a little (the map
    // casts the real shadow); without one they stand in for it.
    let blob_sun = mix(blob, mix(1.0, blob, 0.3), g.shadow_params.y);
    let sun_vis = min(sun_shadow(in.rel, n), blob_sun);
    var col = base * (g.sun_color.rgb * g.sun_dir.w * styled_light(ndl * sun_vis) + ambient(n) * mix(0.55, 1.0, blob));
    col = col + local_lights(in.rel, n, normalize(-in.rel), base, 0.85);
    col = apply_fog(col, in.rel);
    return finish(col, in.rel);
}

// ---- Meshes: instanced, lit -----------------------------------------------

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tint: f32,
    @location(12) uv: vec2<f32>,
    // sRGB colour multiplying the instance colour; alpha: window seed.
    @location(13) color: vec4<f32>,
    // Building facade: storey m / 10, bay m / 10, glass share, lit share
    // (× 127/255, + 128/255 for shop windows). Zero: no windows.
    @location(14) facade: vec4<f32>,
};

struct InstanceIn {
    @location(3) m0: vec4<f32>,
    @location(4) m1: vec4<f32>,
    @location(5) m2: vec4<f32>,
    @location(6) m3: vec4<f32>,
    @location(7) n0: vec4<f32>,
    @location(8) n1: vec4<f32>,
    @location(9) n2: vec4<f32>,
    @location(10) color: vec4<f32>,
    // roughness, metallic, emissive, unused
    @location(11) material: vec4<f32>,
};

struct MeshOut {
    @builtin(position) pos: vec4<f32>,
    // Relative to the camera.
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) material: vec4<f32>,
    @location(4) uv: vec2<f32>,
    @location(5) facade: vec4<f32>,
    @location(6) seed: f32,
};

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let lin = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    return select(lin, vec3<f32>(1.0), c >= vec3<f32>(1.0));
}

// Share of [x − w/2, x + w/2] inside the repeating band [lo, hi) of each
// unit cell: a box-filtered pulse, so distant windows blur instead of shimmering.
fn band_integral(x: f32, lo: f32, hi: f32) -> f32 {
    return floor(x) * (hi - lo) + clamp(fract(x) - lo, 0.0, hi - lo);
}
fn filtered_band(x: f32, lo: f32, hi: f32, w: f32) -> f32 {
    let h = max(w, 1e-4) * 0.5;
    return (band_integral(x + h, lo, hi) - band_integral(x - h, lo, hi)) / (2.0 * h);
}

fn hash3(p: vec3<f32>) -> f32 {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q = q + dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

// Where water stands: patches covering about a fifth of a surface with
// `share` 1 (asphalt), fewer on smaller shares (raised sidewalks drain).
fn puddles(p: vec2<f32>, share: f32) -> f32 {
    let n = value_noise(p * 0.22) * 0.65 + value_noise(p * 0.8 + 17.0) * 0.35;
    let at = 0.74 - 0.1 * share;
    return smoothstep(at, at + 0.07, n);
}

// Lights reflected in wet ground. Water roughened by rain reflects a light
// over a lobe `sigma` rad wide; seen at a low angle the lobe is narrow
// sideways and long towards the viewer, so lamps and headlights draw long
// streaks down the road. Lights count by how bright they look, not by how
// far they shine, so tail lamps streak too.
fn wet_streaks(rel: vec3<f32>, n: vec3<f32>, v: vec3<f32>, sigma: f32) -> vec3<f32> {
    let r = reflect(-v, n);
    let flat_side = cross(r, vec3<f32>(0.0, 0.0, 1.0));
    if (dot(flat_side, flat_side) < 1e-6) {
        return vec3<f32>(0.0);
    }
    let side = normalize(flat_side);
    let rise = cross(side, r);
    let cos_i = max(dot(n, v), 0.02);
    let sx = sigma * cos_i + 0.002;
    let sy = sigma * 2.5;
    let fresnel = 0.02 + 0.98 * pow(1.0 - cos_i, 5.0);
    var sum = vec3<f32>(0.0);
    let count = u32(g.params.w);
    for (var i = 0u; i < count; i = i + 1u) {
        let a = g.lights[i * 3u];
        let b = g.lights[i * 3u + 1u];
        let d = a.xyz - rel;
        let dist = length(d);
        let l = d / max(dist, 1e-3);
        if (dot(l, r) < 0.5) {
            continue;
        }
        var facing = 1.0;
        if (b.w > -1.5) {
            facing = smoothstep(b.w - 0.5, b.w, dot(-l, g.lights[i * 3u + 2u].xyz));
        }
        let dx = dot(l, side) / sx;
        let dy = dot(l, rise) / sy;
        let lobe = exp(-0.5 * (dx * dx + dy * dy)) / (6.2831853 * sx * sy);
        let peak = max(max(b.r, b.g), max(b.b, 1e-4));
        sum = sum + b.rgb / peak * sqrt(peak) * facing * lobe / (1.0 + dist * dist * 0.02);
    }
    return sum * fresnel * 0.25;
}

@vertex
fn vs_mesh(v: VertexIn, inst: InstanceIn) -> MeshOut {
    let model = mat4x4<f32>(inst.m0, inst.m1, inst.m2, inst.m3);
    let normal_m = mat3x3<f32>(inst.n0.xyz, inst.n1.xyz, inst.n2.xyz);
    let world = model * vec4<f32>(v.position, 1.0);
    var out: MeshOut;
    out.pos = g.view_proj * world;
    out.rel = world.xyz;
    out.normal = normalize(normal_m * v.normal);
    let hub = vec3<f32>(0.55, 0.56, 0.58);
    out.color = mix(inst.color.rgb * srgb_to_linear(v.color.rgb), hub, v.tint);
    var m = inst.material;
    // Hub areas are polished metal.
    m.x = mix(m.x, 0.3, v.tint);
    m.y = mix(m.y, 1.0, v.tint);
    out.material = m;
    out.uv = v.uv;
    out.facade = v.facade;
    out.seed = v.color.a;
    return out;
}

fn shade_mesh(in: MeshOut) -> vec3<f32> {
    // Sampled first, in uniform control flow.
    let albedo_map = in.color * textureSample(base_tex, base_sampler, in.uv).rgb;
    let normal_map = textureSample(normal_tex, base_sampler, in.uv).rgb;
    let mr = textureSample(mr_tex, base_sampler, in.uv);
    let glow = textureSample(emissive_tex, base_sampler, in.uv).rgb;
    let ao = mix(1.0, textureSample(occlusion_tex, base_sampler, in.uv).r, mat.params.y);

    // Building walls: windows in bays and storeys (facade.x > 0). The
    // cell size in screen space filters them, so far facades stay calm.
    let fac = in.facade;
    let on_wall = select(0.0, 1.0, fac.x > 0.0 && fac.z > 0.0);
    let cell = in.uv / vec2<f32>(max(fac.y * 10.0, 0.1), max(fac.x * 10.0, 0.1));
    let fw = fwidth(cell);
    let shops = fac.w > 0.5;
    let lit_share = (fac.w * 255.0 - select(0.0, 128.0, shops)) / 127.0;
    let shopfront = shops && cell.y < 1.0;
    let wx = select(vec2<f32>(0.5 - fac.z * 0.5, 0.5 + fac.z * 0.5), vec2<f32>(0.06, 0.94), shopfront);
    let wy = select(vec2<f32>(0.3, 0.85), vec2<f32>(0.04, 0.8), shopfront);
    let glass = filtered_band(cell.x, wx.x, wx.y, fw.x) * filtered_band(cell.y, wy.x, wy.y, fw.y) * on_wall;
    let pane = hash3(vec3<f32>(floor(cell), in.seed * 255.0));
    let lit = step(pane, lit_share + select(0.0, 0.3, shopfront));
    // Walls: a ledge at each floor and darker, dirtier bottoms.
    let ledge = 1.0 - 0.12 * filtered_band(cell.y, 0.94, 1.0, fw.y) * on_wall;
    let grime = mix(1.0, mix(0.78, 1.0, smoothstep(0.0, 0.8, in.uv.y)), on_wall);
    let interior = vec3<f32>(0.025, 0.03, 0.035) + vec3<f32>(0.05, 0.045, 0.04) * hash3(vec3<f32>(floor(cell) + 7.0, in.seed));
    let albedo = mix(albedo_map * ledge * grime, interior, glass);
    // Lit rooms: mostly warm lamps, some cool screens and strip lights; shops brighter.
    let tone = hash3(vec3<f32>(floor(cell) + 19.0, in.seed * 255.0));
    let room = mix(vec3<f32>(1.0, 0.66, 0.36), vec3<f32>(0.75, 0.82, 1.0), step(0.8, tone));
    let window_glow = mix(room * (0.18 + 0.22 * tone), vec3<f32>(1.0, 0.9, 0.75) * 0.5, select(0.0, 1.0, shopfront))
        * (glass * lit * g.shadow_params.z);

    let n_geo = normalize(in.normal);
    let n = perturb_normal(n_geo, in.rel, in.uv, normal_map, mat.params.x);
    let l = g.sun_dir.xyz;
    let v = normalize(-in.rel);
    var rough = mix(clamp(in.material.x * mr.g, 0.02, 1.0), 0.06, glass);
    // Wet roads (material.w > 0 marks surfaces that gather water, and how
    // readily puddles form): puddles, darker and mirror-smooth, and streaks
    // of every light in view.
    let wet = select(0.0, g.weather.x, in.material.w > 0.0);
    var puddle = 0.0;
    var albedo_wet = albedo;
    if (wet > 0.0) {
        puddle = puddles(in.rel.xy + g.camera_pos.xy, in.material.w) * wet;
        albedo_wet = albedo * (1.0 - 0.55 * puddle);
        rough = mix(rough, 0.03, puddle);
    }
    let metal = clamp(in.material.y * mr.b, 0.0, 1.0);

    let ndl = styled_light(lambert(dot(n, l)) * sun_shadow(in.rel, n_geo));
    let h = normalize(l + v);
    let shininess = mix(512.0, 6.0, rough);
    var spec = pow(max(dot(n, h), 0.0), shininess) * (shininess + 8.0) / 25.0;
    // Toon: one crisp highlight; flat: matte, no highlights or reflections.
    if (g.style.x > 0.5) {
        spec = step(8.0, spec) * 4.0;
    }
    spec = spec * (1.0 - g.style.w);
    let f0 = mix(vec3<f32>(0.04), albedo_wet, metal);
    let fresnel = f0 + (1.0 - f0) * pow(1.0 - max(dot(n, v), 0.0), 5.0);

    let sun = g.sun_color.rgb * g.sun_dir.w;
    let diffuse = albedo_wet * (1.0 - metal) * (sun * ndl + ambient(n) * ao);
    // Rough surfaces scatter the sky reflection away: fade it with roughness².
    let gloss = (1.0 - rough) * (1.0 - rough) * (1.0 - g.style.w) * select(1.0, 0.4, g.style.x > 0.5);
    // Puddles also mirror the darker street round them, not just the sky:
    // the real street where the frame has reflections, else a dimmed sky.
    var mirrored = sky_color(reflect(-v, n)) * (1.0 - 0.45 * puddle);
    if (wet > 0.0 && g.reflect_params.z > 0.5) {
        mirrored = street_reflection(in.pos.xy, rough);
    }
    let env = mirrored * fresnel * gloss * ao;
    var col = diffuse + sun * spec * ndl * fresnel * (1.0 - rough * 0.7) + env;
    col += local_lights(in.rel, n, v, albedo_wet * (1.0 - metal), rough);
    // Glow: the emissive map's colour if there is one, else the surface's.
    col += mix(albedo_wet, glow, mat.params.z) * in.material.z;
    col += window_glow;
    if (wet > 0.0) {
        col += wet_streaks(in.rel, n_geo, v, mix(mix(0.12, 0.05, wet), 0.012, puddle)) * wet;
    }
    col = apply_fog(col, in.rel);
    return col;
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    return finish(shade_mesh(in), in.rel);
}

// The mirrored pass: linear colour, and nothing right round the camera (a
// cab's dashboard would otherwise show up upside down in the road).
@fragment
fn fs_mesh_mirror(in: MeshOut) -> @location(0) vec4<f32> {
    if (length(in.rel) < 2.5) {
        discard;
    }
    return vec4<f32>(shade_mesh(in), 1.0);
}

// ---- Shadow map: depth from the sun -------------------------------------------

@vertex
fn vs_shadow(v: VertexIn, inst: InstanceIn) -> @builtin(position) vec4<f32> {
    let model = mat4x4<f32>(inst.m0, inst.m1, inst.m2, inst.m3);
    return g.light_view_proj * model * vec4<f32>(v.position, 1.0);
}

// ---- Windscreen: rain drops and the wipers that clear them ------------------
// Drops are not stored: each sits in a cell of a grid on the glass, and is
// there if its cell's random arrival time has passed since a blade last
// crossed it (the same maths as `Wipers::since_wiped` in Rust).

struct GlassOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) rel: vec3<f32>,
    // Metres across and up the glass from its bottom-left corner.
    @location(1) st: vec2<f32>,
};

@vertex
fn vs_glass(@builtin(vertex_index) i: u32) -> GlassOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let st = corners[i] * vec2<f32>(g.glass[0].w, g.glass[1].w);
    var out: GlassOut;
    out.rel = g.glass[0].xyz + g.glass[1].xyz * st.x + g.glass[2].xyz * st.y;
    out.pos = g.view_proj * vec4<f32>(out.rel, 1.0);
    out.st = st;
    return out;
}

// Seconds since a blade last crossed `p`, a number for that pass, and the
// distance from its pivot. Never wiped: (1e6, 0, 0).
fn since_wiped(p: vec2<f32>) -> vec3<f32> {
    var best = vec3<f32>(1e6, 0.0, 0.0);
    let t = g.glass[4].x;
    let cycle = g.glass[4].y;
    let stroke = g.glass[4].z;
    let arm = g.glass[4].w;
    if (cycle <= 0.0) {
        return best;
    }
    let park = g.glass[5].y;
    let top = g.glass[5].z;
    let q = vec2<f32>(select(p.x, g.glass[0].w - p.x, g.glass[5].x > 0.5), p.y);
    let n = floor(t / cycle);
    let tau = t - n * cycle;
    for (var k = 0u; k < 2u; k = k + 1u) {
        let pivot = select(g.glass[3].zw, g.glass[3].xy, k == 0u);
        let d = q - pivot;
        let r = length(d);
        let f = (atan2(d.y, d.x) - park) / (top - park);
        if (r < arm * g.glass[5].w || r > arm || f < 0.0 || f > 1.0) {
            continue;
        }
        let up = stroke / 6.2831853 * acos(1.0 - 2.0 * f);
        let down = stroke - up;
        var at = (n - 1.0) * cycle + down;
        var crossing = n * 2.0 - 1.0;
        if (tau >= down) {
            at = n * cycle + down;
            crossing = n * 2.0 + 1.0;
        } else if (tau >= up) {
            at = n * cycle + up;
            crossing = n * 2.0;
        }
        let since = t - at;
        if (since < best.x) {
            best = vec3<f32>(since, crossing * 2.0 + f32(k), r);
        }
    }
    return best;
}

// Bright lights seen through wet glass: water scatters them into a soft
// glow round each one (`spread`: higher is tighter), for view direction `dir`.
fn wet_glow(dir: vec3<f32>, rel: vec3<f32>, spread: f32) -> vec3<f32> {
    var sum = vec3<f32>(0.0);
    let count = u32(g.params.w);
    for (var i = 0u; i < count; i = i + 1u) {
        let a = g.lights[i * 3u];
        let b = g.lights[i * 3u + 1u];
        let d = a.xyz - rel;
        let dist = length(d);
        let l = d / max(dist, 1e-3);
        var facing = 1.0;
        if (b.w > -1.5) {
            // Lamps glint even a little outside their beam.
            facing = smoothstep(b.w - 0.35, b.w + 0.05, dot(-l, g.lights[i * 3u + 2u].xyz));
        }
        sum = sum + b.rgb * facing * pow(max(dot(dir, l), 0.0), spread) / (1.0 + dist * dist * 0.01);
    }
    return sum;
}

// What a drop shows: the scene beyond, upside down and shrunk (a tiny lens),
// as the sky above and the lit ground below, with a bright point for every
// light in view.
fn seen_through(dir: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let sun = g.sun_color.rgb * g.sun_dir.w * max(g.sun_dir.z, 0.0);
    let ground = g.ground_base.rgb * (sun + ambient(vec3<f32>(0.0, 0.0, 1.0)));
    let col = mix(sky_color(dir), ground, smoothstep(0.0, -0.12, dir.z));
    return col + wet_glow(dir, rel, 80.0) * 3.0;
}

// One layer of drops: cells `cell` m wide, radii `r0`–`r1` m, a drop in a
// share `chance` of cells, all arrived within `fill` s of a wipe.
// Returns premultiplied colour and coverage.
fn drop_layer(p: vec2<f32>, fw: f32, cell: f32, r0: f32, r1: f32, chance: f32, fill: f32,
              layer: f32, rel: vec3<f32>, toward_eye: vec3<f32>) -> vec4<f32> {
    let c = floor(p / cell);
    let wiped = since_wiped((c + 0.5) * cell);
    let seed = vec3<f32>(c, layer * 41.0 + wiped.y * 7.0);
    if (hash3(seed) > chance || wiped.x < hash3(seed + 11.3) * fill) {
        return vec4<f32>(0.0);
    }
    let h = hash3(seed + 23.7);
    let r = mix(r0, r1, h * h);
    let centre = c * cell + vec2<f32>(r) + (cell - 2.0 * r)
        * vec2<f32>(hash3(seed + 37.1), hash3(seed + 51.9));
    // Drops smaller than a pixel spread to one and fade instead of flickering.
    let re = max(r, fw);
    let q = (p - centre) / re;
    let dd = length(q);
    let cover = clamp((1.0 - dd) * re / fw + 0.5, 0.0, 1.0) * (r / re) * (r / re);
    if (cover <= 0.0) {
        return vec4<f32>(0.0);
    }
    let across = g.glass[1].xyz;
    let up = g.glass[2].xyz;
    let v = normalize(rel);
    // Looking through a drop bends the view outwards from its centre:
    // its top shows the road, its bottom the sky.
    let bend = (across * q.x + up * q.y) * 0.5;
    let dir = normalize(v - bend);
    var col = seen_through(dir, rel) + wet_glow(v, rel, 60.0) * 0.12;
    // The rim turns light away; the near side catches the sky faintly.
    let rim = smoothstep(0.45, 1.0, dd);
    col = col * mix(1.0, 0.2, rim);
    let n = normalize(toward_eye * sqrt(max(1.0 - dd * dd, 0.0)) * 1.5 + across * q.x + up * q.y);
    col = col + sky_color(reflect(v, n)) * 0.04;
    // The middle of a drop lets some of the view behind it through.
    let a = cover * mix(0.6, 0.95, rim);
    let shown = select(tonemap(col), col, g.look.z > 0.5);
    return vec4<f32>(shown * a, a);
}

@fragment
fn fs_glass(in: GlassOut) -> @location(0) vec4<f32> {
    let fw = max(length(fwidth(in.st)) * 0.7, 1e-5);
    let rain = g.glass[2].w;
    let across = g.glass[1].xyz;
    let up = g.glass[2].xyz;
    var toward_eye = normalize(cross(across, up));
    if (dot(toward_eye, -in.rel) < 0.0) {
        toward_eye = -toward_eye;
    }
    // Drops gather faster in harder rain.
    let fill = mix(8.0, 1.6, rain);
    let wiped = since_wiped(in.st);
    // A thin veil of water, thinner where a blade has just passed, with
    // fresh streaks behind the blade that dry in a moment.
    let settled = min(wiped.x / fill, 1.0);
    let streaks = exp(-wiped.x * 12.0) * smoothstep(0.0, 0.04, wiped.x) * smoothstep(0.55, 0.9, value_noise(vec2<f32>(wiped.z * 900.0, wiped.y * 3.1)));
    let veil_a = rain * (0.03 + 0.05 * settled) + 0.05 * rain * streaks;
    let veil = g.sky_horizon.rgb * 0.9 + wet_glow(normalize(in.rel), in.rel, 60.0) * 0.6;
    var acc = vec4<f32>(tonemap(veil) * veil_a, veil_a);
    let more = 0.35 + 0.65 * rain;
    let layers = array<vec4<f32>, 4>(
        // cell, smallest and largest radius (m), share of cells with a drop
        vec4<f32>(0.006, 0.0006, 0.0014, 0.5),
        vec4<f32>(0.012, 0.0012, 0.003, 0.5),
        vec4<f32>(0.022, 0.0025, 0.0055, 0.45),
        vec4<f32>(0.045, 0.004, 0.0095, 0.3),
    );
    for (var k = 0u; k < 4u; k = k + 1u) {
        let L = layers[k];
        let d = drop_layer(in.st, fw, L.x, L.y, L.z, L.w * more, fill, f32(k), in.rel, toward_eye);
        acc = d + acc * (1.0 - d.a);
    }
    return acc;
}
