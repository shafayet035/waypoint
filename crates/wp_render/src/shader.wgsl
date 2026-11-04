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
};

@group(0) @binding(0) var<uniform> g: Globals;

fn tonemap(c: vec3<f32>) -> vec3<f32> {
    // ACES filmic fit (Narkowicz 2015).
    let x = c * g.params.z;
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
    return mix(bounce, g.sky_zenith.rgb * 0.9, n.z * 0.5 + 0.5);
}

// `rel`: position relative to the camera (all geometry is drawn camera-relative,
// so big maps keep their precision).
fn apply_fog(col: vec3<f32>, rel: vec3<f32>) -> vec3<f32> {
    let d = length(rel);
    let f = 1.0 - exp(-d * g.params.x);
    return mix(col, g.sky_horizon.rgb, f);
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

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    // A point on the near plane (depth 1 with reversed-Z), seen from the camera.
    let p = g.inv_view_proj * vec4<f32>(in.ndc, 1.0, 1.0);
    let dir = normalize(p.xyz / p.w);
    return vec4<f32>(tonemap(sky_color(dir)), 1.0);
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
    out.rel = vec3<f32>(c, -g.camera_pos.z);
    out.world = vec3<f32>(c + g.camera_pos.xy, 0.0);
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
    base = mix(base, g.ground_line.rgb, grid(p, 10.0, 0.06) * 0.35);
    base = mix(base, g.ground_line.rgb, grid(p, 50.0, 0.14) * 0.5);
    let n = vec3<f32>(0.0, 0.0, 1.0);
    let ndl = max(dot(n, g.sun_dir.xyz), 0.0);
    let shadow = blob_shadows(p);
    var col = base * (g.sun_color.rgb * g.sun_dir.w * ndl * shadow + ambient(n) * mix(0.55, 1.0, shadow));
    col = apply_fog(col, in.rel);
    return vec4<f32>(tonemap(col), 1.0);
}

// ---- Meshes: instanced, lit -----------------------------------------------

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) tint: f32,
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
};

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
    out.color = mix(inst.color.rgb, hub, v.tint);
    var m = inst.material;
    // Hub areas are polished metal.
    m.x = mix(m.x, 0.3, v.tint);
    m.y = mix(m.y, 1.0, v.tint);
    out.material = m;
    return out;
}

@fragment
fn fs_mesh(in: MeshOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let l = g.sun_dir.xyz;
    let v = normalize(-in.rel);
    let rough = clamp(in.material.x, 0.02, 1.0);
    let metal = clamp(in.material.y, 0.0, 1.0);

    let ndl = max(dot(n, l), 0.0);
    let h = normalize(l + v);
    let shininess = mix(512.0, 6.0, rough);
    let spec = pow(max(dot(n, h), 0.0), shininess) * (shininess + 8.0) / 25.0;
    let f0 = mix(vec3<f32>(0.04), in.color, metal);
    let fresnel = f0 + (1.0 - f0) * pow(1.0 - max(dot(n, v), 0.0), 5.0);

    let sun = g.sun_color.rgb * g.sun_dir.w;
    let diffuse = in.color * (1.0 - metal) * (sun * ndl + ambient(n));
    // Rough surfaces scatter the sky reflection away: fade it with roughness².
    let gloss = (1.0 - rough) * (1.0 - rough);
    let env = sky_color(reflect(-v, n)) * fresnel * gloss;
    var col = diffuse + sun * spec * ndl * fresnel * (1.0 - rough * 0.7) + env;
    col += in.color * in.material.z;
    col = apply_fog(col, in.rel);
    return vec4<f32>(tonemap(col), 1.0);
}
