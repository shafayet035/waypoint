#!/usr/bin/env python3
"""Low-poly vehicle models for the sandbox, written as glTF 2.0 binaries (.glb).

Our own work, released as CC0. Run from the repo root:

    python3 tools/models/vehicles.py examples/sandbox/assets/vehicles hatchback city_bus
    python3 tools/models/vehicles.py examples/harbour_shuttle/assets/vehicles minibus

Shapes are built in the engine's frame (x forward, y left, z up, metres) with
the origin on the ground midway between the axles, then written in glTF axes
(+Z forward, +Y up, +X left). Dimensions match the sandbox's .vehicle.ron files.
Material names give parts their roles in the engine: paint (vehicle colour),
glass, headlight, brake_light; anything else keeps its own colour.
"""

import json
import math
import os
import struct
import sys

MATERIALS = {
    "paint": dict(color=(0.5, 0.5, 0.5), rough=0.32),
    "glass": dict(color=(0.015, 0.02, 0.028), rough=0.05),
    "trim": dict(color=(0.02, 0.02, 0.022), rough=0.6),
    "grey": dict(color=(0.12, 0.12, 0.125), rough=0.5),
    "plate": dict(color=(0.75, 0.75, 0.7), rough=0.4),
    "headlight": dict(color=(0.9, 0.9, 0.85), rough=0.1, emissive=0.6),
    "brake_light": dict(color=(0.7, 0.02, 0.02), rough=0.2, emissive=0.8),
    "indicator": dict(color=(0.8, 0.35, 0.02), rough=0.2, emissive=0.3),
    "sign": dict(color=(0.95, 0.55, 0.08), rough=0.3, emissive=1.5),
}


def sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def dot(a, b):
    return a[0] * b[0] + a[1] * b[1] + a[2] * b[2]


class Mesh:
    def __init__(self):
        self.tris = {}

    def tri(self, mat, a, b, c, ref):
        """A flat triangle facing away from `ref`."""
        n = cross(sub(b, a), sub(c, a))
        length = math.sqrt(dot(n, n))
        if length < 1e-9:
            return
        centre = tuple((a[i] + b[i] + c[i]) / 3 for i in range(3))
        if dot(n, sub(centre, ref)) < 0:
            b, c = c, b
            n = (-n[0], -n[1], -n[2])
        n = (n[0] / length, n[1] / length, n[2] / length)
        self.tris.setdefault(mat, []).append((a, b, c, n))

    def quad(self, mat, a, b, c, d, ref):
        self.tri(mat, a, b, c, ref)
        self.tri(mat, a, c, d, ref)

    def box(self, mat, lo, hi):
        ref = tuple((lo[i] + hi[i]) / 2 for i in range(3))
        x0, y0, z0 = lo
        x1, y1, z1 = hi
        faces = [
            ((x1, y0, z0), (x1, y1, z0), (x1, y1, z1), (x1, y0, z1)),
            ((x0, y0, z0), (x0, y1, z0), (x0, y1, z1), (x0, y0, z1)),
            ((x0, y1, z0), (x1, y1, z0), (x1, y1, z1), (x0, y1, z1)),
            ((x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (x0, y0, z1)),
            ((x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)),
            ((x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0)),
        ]
        for f in faces:
            self.quad(mat, *f, ref)

    def mirrored_box(self, mat, lo, hi):
        """A box on the left (y > 0) and its mirror on the right."""
        self.box(mat, lo, hi)
        self.box(mat, (lo[0], -hi[1], lo[2]), (hi[0], -lo[1], hi[2]))

    def triangles(self):
        return sum(len(t) for t in self.tris.values())


def lerp_keys(keys, x):
    """Linear interpolation in a list of (x, values) sorted by x."""
    if x <= keys[0][0]:
        return keys[0][1]
    for (x0, v0), (x1, v1) in zip(keys, keys[1:]):
        if x0 <= x <= x1:
            t = (x - x0) / (x1 - x0) if x1 > x0 else 0.0
            return tuple(a + (b - a) * t for a, b in zip(v0, v1))
    return keys[-1][1]


def loft(mesh, xs, ring, band_mat, top_mat, cap_mat, z_centre):
    """Sweep a symmetric cross-section along x.

    `ring(x)` gives the left half of the section, bottom to top, as (y, z)
    points; it is mirrored to the right. Bands join consecutive points;
    `band_mat(i, x0, x1)` names the material of band i between two stations,
    `top_mat(x0, x1)` the band across the top, `cap_mat(i, end)` the end caps.
    """
    rings = [(x, ring(x)) for x in xs]
    for (xa, ra), (xb, rb) in zip(rings, rings[1:]):
        ref = ((xa + xb) / 2, 0.0, z_centre)
        for i in range(len(ra) - 1):
            mat = band_mat(i, xa, xb)
            for s in (1, -1):
                p = lambda x, q: (x, s * q[0], q[1])
                mesh.quad(mat, p(xa, ra[i]), p(xa, ra[i + 1]), p(xb, rb[i + 1]), p(xb, rb[i]), ref)
        top, bot = len(ra) - 1, 0
        for k, mat in ((top, top_mat(xa, xb)), (bot, "trim")):
            a, b = ra[k], rb[k]
            mesh.quad(mat, (xa, a[0], a[1]), (xb, b[0], b[1]), (xb, -b[0], b[1]), (xa, -a[0], a[1]), ref)
    centre = ((xs[0] + xs[-1]) / 2, 0.0, z_centre)
    for end, (x, r) in (("rear", rings[0]), ("front", rings[-1])):
        for i in range(len(r) - 1):
            a, b = r[i], r[i + 1]
            mesh.quad(cap_mat(i, end), (x, a[0], a[1]), (x, b[0], b[1]), (x, -b[0], b[1]), (x, -a[0], a[1]), centre)


def arch_z(x, axles, radius, top, base):
    z = base
    for ax in axles:
        d = (x - ax) / radius
        if abs(d) < 1.0:
            z = max(z, top * math.sqrt(1.0 - d * d))
    return z


def arch_stations(axles, radius):
    out = []
    for ax in axles:
        for d in (-1.0, -0.9, -0.7, -0.4, 0.0, 0.4, 0.7, 0.9, 1.0):
            out.append(ax + d * radius)
    return out


def stations(front, rear, extra):
    xs = sorted(set(round(x, 4) for x in [front, rear] + extra if rear <= x <= front))
    return xs


def within(x0, x1, lo, hi):
    return lo - 1e-6 <= x0 and x1 <= hi + 1e-6


# --- hatchback ------------------------------------------------------------------
# hatchback.vehicle.ron: wheelbase 2.6, track 1.52, wheel radius 0.31. The
# engine's footprint for it: front 1.75 and rear -2.12 from the centre of mass,
# 1.05 m behind the front axle, so 2.0 ahead and 1.87 behind the axle midpoint.


def hatchback():
    m = Mesh()
    front, rear, axle = 2.0, -1.87, 1.3
    # x: belt, roof, body half-width, roof half-width
    keys = [
        (-1.87, (0.90, 1.06, 0.84, 0.66)),
        (-1.80, (0.93, 1.12, 0.87, 0.68)),
        (-1.25, (0.95, 1.38, 0.88, 0.70)),
        (0.25, (0.93, 1.39, 0.88, 0.70)),
        (0.95, (0.89, 0.89, 0.88, 0.88)),
        (1.90, (0.76, 0.76, 0.86, 0.86)),
        (2.00, (0.62, 0.62, 0.80, 0.80)),
    ]
    axles = (axle, -axle)

    def ring(x):
        belt, roof, wb, wr = lerp_keys(keys, x)
        base = 0.25 if (x > axle + 0.4 or x < -axle - 0.4) else 0.22
        zb = arch_z(x, axles, 0.42, 0.70, base)
        return [
            (wb - 0.05, zb),
            (wb, zb + 0.08),
            (wb, belt),
            (wr, roof),
            (wr, roof),
            (wr - 0.08, roof + 0.035),
        ]

    breaks = [k[0] for k in keys] + [-1.62, -0.56, -0.44] + arch_stations(axles, 0.42)
    xs = stations(front, rear, breaks)

    def band_mat(i, x0, x1):
        if i == 0:
            return "trim"
        if i == 2:
            side = within(x0, x1, -1.62, 0.95) and not within(x0, x1, -0.56, -0.44)
            return "glass" if side else "paint"
        return "paint"

    def top_mat(x0, x1):
        if within(x0, x1, 0.25, 0.95) or within(x0, x1, -1.80, -1.25):
            return "glass"
        return "paint"

    loft(m, xs, ring, band_mat, top_mat, lambda i, end: "trim" if i == 0 else "paint", 0.7)

    # Bumpers, grille, lamps, plates, mirrors, door handles.
    m.box("trim", (1.93, -0.80, 0.22), (2.03, 0.80, 0.36))
    m.box("trim", (-1.90, -0.80, 0.25), (-1.80, 0.80, 0.40))
    m.box("grey", (1.97, -0.40, 0.37), (2.015, 0.40, 0.50))
    m.box("plate", (2.03, -0.26, 0.24), (2.04, 0.26, 0.34))
    m.box("plate", (-1.91, -0.26, 0.42), (-1.90, 0.26, 0.52))
    m.mirrored_box("headlight", (1.95, 0.44, 0.50), (2.015, 0.72, 0.61))
    m.mirrored_box("indicator", (1.94, 0.72, 0.50), (2.005, 0.79, 0.58))
    m.mirrored_box("brake_light", (-1.895, 0.56, 0.78), (-1.84, 0.80, 0.92))
    m.mirrored_box("paint", (0.70, 0.88, 0.94), (0.82, 1.00, 1.04))
    m.mirrored_box("glass", (0.705, 0.885, 0.945), (0.715, 0.995, 1.035))
    for x in (0.10, -0.80):
        m.mirrored_box("grey", (x - 0.08, 0.879, 0.86), (x + 0.08, 0.89, 0.88))
    return m


# --- city bus -------------------------------------------------------------------
# city_bus.vehicle.ron: 12 m long, 2.55 m wide, 3.1 m high, wheelbase 5.9,
# front overhang 2.7, wheel radius 0.48. Axle midpoint origin: front 5.65, rear -6.35.


def city_bus():
    m = Mesh()
    front, rear, axle = 5.65, -6.35, 2.95
    # x: belt, window top, roof, body half-width, roof half-width
    keys = [
        (-6.35, (1.20, 2.70, 2.98, 1.24, 1.18)),
        (-6.27, (1.15, 2.75, 3.06, 1.275, 1.23)),
        (5.57, (1.15, 2.75, 3.06, 1.275, 1.23)),
        (5.65, (0.95, 2.82, 2.98, 1.24, 1.18)),
    ]
    axles = (axle, -axle)

    def ring(x):
        belt, wtop, roof, wb, wr = lerp_keys(keys, x)
        base = 0.36 if (x > axle + 0.6 or x < -axle - 0.6) else 0.30
        zb = arch_z(x, axles, 0.64, 1.06, base)
        return [
            (wb - 0.05, zb),
            (wb, zb + 0.08),
            (wb, belt),
            (wb, wtop),
            (wb - 0.01, roof - 0.08),
            (wr, roof),
        ]

    pillars = [-4.85, -3.40, -1.95, -0.70, 0.70, 2.15, 3.60, 4.25]
    breaks = [k[0] for k in keys] + arch_stations(axles, 0.64)
    for p in pillars:
        breaks += [p - 0.07, p + 0.07]
    xs = stations(front, rear, breaks)

    def band_mat(i, x0, x1):
        if i == 0:
            return "trim"
        if i == 2:
            if not within(x0, x1, -6.27, 5.65):
                return "paint"
            if any(within(x0, x1, p - 0.07, p + 0.07) for p in pillars):
                return "trim"
            return "glass"
        return "paint"

    def cap_mat(i, end):
        if i == 0:
            return "trim"
        if i == 2:
            return "glass"
        return "paint"

    loft(m, xs, ring, band_mat, lambda x0, x1: "paint", cap_mat, 1.5)

    # Doors on the kerb side (right: y < 0): front and middle, glazed.
    for x0, x1 in ((4.30, 5.40), (-0.62, 0.62)):
        m.box("trim", (x0, -1.29, 0.36), (x1, -1.276, 2.78))
        m.box("glass", (x0 + 0.06, -1.295, 0.45), ((x0 + x1) / 2 - 0.02, -1.285, 2.70))
        m.box("glass", ((x0 + x1) / 2 + 0.02, -1.295, 0.45), (x1 - 0.06, -1.285, 2.70))
    # Destination sign, lamps, bumpers, mirrors, roof pods.
    m.box("trim", (5.64, -1.0, 2.66), (5.67, 1.0, 2.94))
    m.box("sign", (5.665, -0.9, 2.70), (5.675, 0.9, 2.90))
    m.box("trim", (-6.42, -1.2, 0.30), (-6.30, 1.2, 0.55))
    m.box("trim", (5.60, -1.2, 0.33), (5.72, 1.2, 0.58))
    m.box("plate", (5.72, -0.26, 0.36), (5.73, 0.26, 0.48))
    m.box("plate", (-6.43, -0.26, 0.60), (-6.42, 0.26, 0.72))
    m.mirrored_box("headlight", (5.64, 0.72, 0.62), (5.69, 1.08, 0.78))
    m.mirrored_box("indicator", (5.64, 1.08, 0.62), (5.69, 1.18, 0.78))
    m.mirrored_box("brake_light", (-6.37, 0.92, 0.75), (-6.32, 1.16, 1.15))
    m.mirrored_box("trim", (5.55, 1.24, 2.30), (5.75, 1.40, 2.36))
    m.mirrored_box("trim", (5.72, 1.36, 1.85), (5.80, 1.46, 2.35))
    m.box("grey", (-1.5, -0.9, 3.06), (1.0, 0.9, 3.10))
    return m


# --- minibus --------------------------------------------------------------------
# harbour_shuttle's minibus.vehicle.ron: 7.0 m long, 2.1 m wide, 2.75 m high,
# wheelbase 3.9, front overhang 1.1, wheel radius 0.36. Axle midpoint origin:
# front 3.05, rear -3.95. A van-style nose with a raked windscreen.


def minibus():
    m = Mesh()
    front, rear, axle = 3.05, -3.95, 1.95
    # x: belt, window top, roof, body half-width, roof half-width
    keys = [
        (-3.95, (1.05, 2.30, 2.62, 1.02, 0.98)),
        (-3.88, (1.00, 2.35, 2.72, 1.05, 1.00)),
        (2.20, (1.00, 2.35, 2.72, 1.05, 1.00)),
        (2.95, (1.00, 1.60, 1.75, 1.03, 0.98)),
        (3.05, (0.95, 1.20, 1.25, 1.00, 0.95)),
    ]
    axles = (axle, -axle)

    def ring(x):
        belt, wtop, roof, wb, wr = lerp_keys(keys, x)
        base = 0.30 if (x > axle + 0.5 or x < -axle - 0.5) else 0.26
        zb = arch_z(x, axles, 0.5, 0.80, base)
        return [
            (wb - 0.05, zb),
            (wb, zb + 0.08),
            (wb, belt),
            (wb, wtop),
            (wb - 0.01, roof - 0.06),
            (wr, roof),
        ]

    pillars = [-2.6, -1.3, 0.0, 1.2]
    breaks = [k[0] for k in keys] + arch_stations(axles, 0.5)
    for p in pillars:
        breaks += [p - 0.06, p + 0.06]
    xs = stations(front, rear, breaks)

    def band_mat(i, x0, x1):
        if i == 0:
            return "trim"
        if i == 2:
            if not within(x0, x1, -3.88, 2.95):
                return "paint"
            if any(within(x0, x1, p - 0.06, p + 0.06) for p in pillars):
                return "trim"
            return "glass"
        return "paint"

    def top_mat(x0, x1):
        return "glass" if within(x0, x1, 2.2, 2.95) else "paint"

    def cap_mat(i, end):
        if i == 0:
            return "trim"
        if i == 2 and end == "rear":
            return "glass"
        return "paint"

    loft(m, xs, ring, band_mat, top_mat, cap_mat, 1.3)

    # Sliding door on the kerb side, lamps, grille, bumpers, mirrors, roof rack.
    m.box("trim", (0.25, -1.065, 0.32), (1.35, -1.052, 2.32))
    m.box("glass", (0.32, -1.07, 1.05), (1.28, -1.06, 2.25))
    m.box("grey", (3.03, -0.55, 0.45), (3.07, 0.55, 0.85))
    m.box("trim", (2.98, -1.0, 0.28), (3.12, 1.0, 0.48))
    m.box("trim", (-4.02, -1.0, 0.30), (-3.92, 1.0, 0.50))
    m.box("plate", (3.12, -0.26, 0.30), (3.13, 0.26, 0.42))
    m.box("plate", (-4.03, -0.26, 0.55), (-4.02, 0.26, 0.67))
    m.mirrored_box("headlight", (3.0, 0.58, 0.88), (3.09, 0.92, 1.0))
    m.mirrored_box("indicator", (3.0, 0.92, 0.88), (3.08, 0.99, 1.0))
    m.mirrored_box("brake_light", (-3.98, 0.80, 0.85), (-3.93, 1.0, 1.45))
    m.mirrored_box("trim", (2.55, 1.03, 1.55), (2.75, 1.25, 1.62))
    m.mirrored_box("trim", (2.70, 1.20, 1.25), (2.78, 1.28, 1.60))
    m.box("grey", (-3.0, -0.8, 2.72), (1.8, 0.8, 2.76))
    return m


# --- glTF writer ----------------------------------------------------------------


def to_gltf(p):
    """Engine (x fwd, y left, z up) → glTF (X left, Y up, Z fwd)."""
    return (p[1], p[2], p[0])


def write_glb(mesh, name, path):
    bin_data = bytearray()
    accessors, views, primitives, materials = [], [], [], []
    for mat_name in sorted(mesh.tris):
        tris = mesh.tris[mat_name]
        spec = MATERIALS[mat_name]
        mat = {
            "name": mat_name,
            "pbrMetallicRoughness": {
                "baseColorFactor": list(spec["color"]) + [1.0],
                "metallicFactor": 0.0,
                "roughnessFactor": spec["rough"],
            },
        }
        e = spec.get("emissive", 0.0)
        if e > 0:
            mat["emissiveFactor"] = [min(e, 1.0)] * 3
            if e > 1.0:
                mat["extensions"] = {"KHR_materials_emissive_strength": {"emissiveStrength": e}}
        materials.append(mat)
        pos, nrm = [], []
        for a, b, c, n in tris:
            for v in (a, b, c):
                pos.append(to_gltf(v))
                nrm.append(to_gltf(n))
        attrs = {}
        for key, values in (("POSITION", pos), ("NORMAL", nrm)):
            offset = len(bin_data)
            for v in values:
                bin_data += struct.pack("<3f", *v)
            views.append({"buffer": 0, "byteOffset": offset, "byteLength": len(values) * 12, "target": 34962})
            acc = {"bufferView": len(views) - 1, "componentType": 5126, "count": len(values), "type": "VEC3"}
            if key == "POSITION":
                acc["min"] = [min(v[i] for v in values) for i in range(3)]
                acc["max"] = [max(v[i] for v in values) for i in range(3)]
            accessors.append(acc)
            attrs[key] = len(accessors) - 1
        primitives.append({"attributes": attrs, "material": len(materials) - 1})
    doc = {
        "asset": {"version": "2.0", "generator": "Waypoint tools/models/vehicles.py"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": name, "mesh": 0}],
        "meshes": [{"name": name, "primitives": primitives}],
        "materials": materials,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{"byteLength": len(bin_data)}],
        "extensionsUsed": ["KHR_materials_emissive_strength"],
    }
    js = json.dumps(doc, separators=(",", ":")).encode()
    js += b" " * (-len(js) % 4)
    bin_data += b"\0" * (-len(bin_data) % 4)
    total = 12 + 8 + len(js) + 8 + len(bin_data)
    with open(path, "wb") as f:
        f.write(struct.pack("<4sII", b"glTF", 2, total))
        f.write(struct.pack("<I4s", len(js), b"JSON"))
        f.write(js)
        f.write(struct.pack("<I4s", len(bin_data), b"BIN\0"))
        f.write(bin_data)


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "."
    os.makedirs(out, exist_ok=True)
    only = sys.argv[2:] or ["hatchback", "city_bus", "minibus"]
    builders = {"hatchback": hatchback, "city_bus": city_bus, "minibus": minibus}
    for name in only:
        build = builders[name]
        mesh = build()
        path = os.path.join(out, name + ".glb")
        write_glb(mesh, name, path)
        print(f"wrote {path}: {mesh.triangles()} triangles")


if __name__ == "__main__":
    main()
