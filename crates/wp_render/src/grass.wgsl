// ---- Grass ---------------------------------------------------------------
// Tufts placed on the GPU: instance k is one cell of a square grid round the
// camera, snapped to world cells (so tufts stay put as the camera moves),
// jittered by a hash of the cell. A mask says where grass grows; tufts thin
// out and shrink with distance. Each tuft is five blades (one triangle
// each) whose tips sway with the wind (simulated time, so frames repeat).

struct GrassParams {
    // x: cell spacing (m), y: grid side (cells), z: reach (m), w: height (m).
    a: vec4<f32>,
    // x: sway, y: 1 on hilly land, z: land mesh drop (m), w: unused.
    b: vec4<f32>,
    base: vec4<f32>,
    tip: vec4<f32>,
    // Land heights: origin xy, cell (m), unused; then nx, ny.
    land: vec4<f32>,
    land_size: vec4<f32>,
    // Mask: origin xy, cell (m), unused; then nx, ny.
    mask: vec4<f32>,
    mask_size: vec4<f32>,
};

@group(2) @binding(0) var<uniform> gp: GrassParams;
@group(2) @binding(1) var land_heights: texture_2d<f32>;
@group(2) @binding(2) var grass_mask: texture_2d<f32>;

struct GrassOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) rel: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

// A grid value with bilinear filtering by hand (heights are 32-bit floats,
// which cannot be filtered by the sampler on every GPU).
fn grid_at(t: texture_2d<f32>, origin: vec2<f32>, cell: f32, size: vec2<f32>, p: vec2<f32>, centred: f32) -> f32 {
    // Values sit at cell centres (grid point + half a cell).
    let q = (p - origin) / cell - vec2<f32>(0.5 * centred);
    if (any(q < vec2<f32>(0.0)) || any(q > size - vec2<f32>(1.0))) {
        return 0.0;
    }
    let i = min(vec2<i32>(floor(q)), vec2<i32>(size) - vec2<i32>(2));
    let f = q - vec2<f32>(i);
    let a = textureLoad(t, i, 0).r;
    let b = textureLoad(t, i + vec2<i32>(1, 0), 0).r;
    let c = textureLoad(t, i + vec2<i32>(0, 1), 0).r;
    let d = textureLoad(t, i + vec2<i32>(1, 1), 0).r;
    return mix(mix(a, b, f.x), mix(c, d, f.x), f.y);
}

// 0–1 from a cell and a channel: an integer hash (PCG), well spread even
// for cells far from the origin (sin-based hashes go coarse there).
fn cell_hash(c: vec2<f32>, k: f32) -> f32 {
    let ic = vec2<i32>(c);
    var x = bitcast<u32>(ic.x) * 1664525u + bitcast<u32>(ic.y) * 2246822519u + u32(k) * 3266489917u;
    x = x * 747796405u + 2891336453u;
    x = ((x >> ((x >> 28u) + 4u)) ^ x) * 277803737u;
    x = (x >> 22u) ^ x;
    return f32(x & 0x00FFFFFFu) / 16777216.0;
}

@vertex
fn vs_grass(@builtin(vertex_index) vi: u32, @builtin(instance_index) ii: u32) -> GrassOut {
    var out: GrassOut;
    out.pos = vec4<f32>(0.0, 0.0, -1.0, 1.0); // hidden unless placed below
    out.rel = vec3<f32>(0.0);
    out.normal = vec3<f32>(0.0, 0.0, 1.0);
    out.color = vec3<f32>(0.0);
    let sp = gp.a.x;
    let n = u32(gp.a.y);
    let cam = g.camera_pos.xy;
    let cam_cell = floor(cam / sp);
    let local = vec2<f32>(f32(ii % n), f32(ii / n)) - vec2<f32>(f32(n / 2u));
    let cell = cam_cell + local;
    let jitter = vec2<f32>(cell_hash(cell, 1.0), cell_hash(cell, 2.0));
    // Position relative to the camera, kept small for precision.
    let rel_xy = (local + jitter) * sp - (cam - cam_cell * sp);
    let dist = length(rel_xy);
    let reach = gp.a.z;
    if (dist > reach) {
        return out;
    }
    let world_xy = cam + rel_xy;
    // Grows where the mask allows, in clumps, thinning with distance.
    let m = grid_at(grass_mask, gp.mask.xy, gp.mask.z, gp.mask_size.xy, world_xy, 1.0);
    let clump = 0.55 + 0.45 * smoothstep(0.25, 0.65, value_noise(world_xy * 0.09));
    let keep = m * clump * (1.0 - smoothstep(reach * 0.55, reach, dist));
    if (cell_hash(cell, 3.0) >= keep) {
        return out;
    }
    var ground = 0.0;
    if (gp.b.y > 0.5) {
        ground = grid_at(land_heights, gp.land.xy, gp.land.z, gp.land_size.xy, world_xy, 0.0) - gp.b.z;
    }
    let root = vec3<f32>(rel_xy, ground - g.camera_pos.z);
    // Skip tufts well behind the camera.
    let clip_root = g.view_proj * vec4<f32>(root, 1.0);
    if (clip_root.w < -1.0) {
        return out;
    }
    let blade = vi / 3u;
    let corner = vi % 3u;
    let h_tuft = gp.a.w * (0.65 + 0.7 * cell_hash(cell, 4.0)) * (1.0 - 0.5 * smoothstep(reach * 0.4, reach, dist));
    let angle = cell_hash(cell, 5.0) * 6.2832 + f32(blade) * 2.5133;
    let d = vec2<f32>(cos(angle), sin(angle));
    let across = vec2<f32>(-d.y, d.x);
    let h = h_tuft * (0.75 + 0.5 * cell_hash(cell, 6.0 + f32(blade)));
    let width = clamp(h * 0.24, 0.03, 0.12);
    // Blades fan out from the tuft's middle.
    let base = root + vec3<f32>(d * 0.06, 0.0);
    var p = base;
    var up = 0.0;
    if (corner == 0u) {
        p = base + vec3<f32>(across * width * 0.5, 0.0);
    } else if (corner == 1u) {
        p = base - vec3<f32>(across * width * 0.5, 0.0);
    } else {
        // The tip: leaning out, pushed by the wind.
        let t = g.weather.z;
        let phase = dot(world_xy, vec2<f32>(0.13, 0.07)) + t * 1.7;
        let gust = sin(phase) * 0.6 + sin(phase * 2.3 + 1.3) * 0.25 + 0.35;
        let wind = vec2<f32>(0.94, 0.34) * gust * gp.b.x * h * 0.35;
        p = base + vec3<f32>(d * h * 0.38 + wind, h * 0.95);
        up = 1.0;
    }
    out.pos = g.view_proj * vec4<f32>(p, 1.0);
    out.rel = p;
    out.normal = normalize(vec3<f32>(d * 0.45, 1.0));
    let shade = 0.82 + 0.36 * cell_hash(cell, 9.0);
    // Darker at the roots (the tuft shades itself).
    out.color = mix(gp.base.rgb * 0.7, gp.tip.rgb, up) * shade;
    return out;
}

@fragment
fn fs_grass(in: GrassOut) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let l = g.sun_dir.xyz;
    let v = normalize(-in.rel);
    let sun = g.sun_color.rgb * g.sun_dir.w;
    let lit = styled_light(lambert(dot(n, l)) * sun_shadow(in.rel, n));
    // Light through the blades when looking towards the sun.
    let through = pow(max(dot(-v, l), 0.0), 4.0) * 0.35;
    var col = in.color * (sun * (lit + through) + ambient(n));
    col += local_lights(in.rel, n, v, in.color, 0.9);
    col = apply_fog(col, in.rel);
    return finish(col, in.rel);
}
