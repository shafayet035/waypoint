"""A stylised boxy 4x4 for Waypoint: roof rack with luggage, bull bar, snorkel,
fender flares and a spare wheel on the back. Our own work, released as CC0.

    /Applications/Blender.app/Contents/MacOS/Blender -b -P tools/models/blender/offroader.py -- <out dir>

Built in the engine's frame (x forward, y left, z up; origin on the ground
midway between the axles), then placed in Blender's (forward is -Y, left is +X)
for the glTF exporter. The road wheels are not modelled: the engine draws them.
Material names give parts their roles: paint (the vehicle's colour), glass,
headlight, brake_light; the rest keep their colour. Sized for
examples/*/vehicles/offroader.vehicle.ron: wheelbase 2.7 m, track 1.62 m,
wheel radius 0.4 m.
"""

import math
import os
import sys

import bmesh
import bpy
from mathutils import Matrix, Vector


def srgb(r, g, b):
    def lin(c):
        c /= 255.0
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    return (lin(r), lin(g), lin(b), 1.0)


MATERIALS = {
    # name: (colour, roughness, metallic, emission strength)
    "paint": (srgb(120, 160, 205), 0.45, 0.0, 0.0),
    "glass": (srgb(28, 34, 42), 0.08, 0.0, 0.0),
    "trim": (srgb(30, 30, 32), 0.7, 0.0, 0.0),
    "metal": (srgb(150, 152, 156), 0.35, 0.8, 0.0),
    "tyre": (srgb(26, 26, 28), 0.9, 0.0, 0.0),
    "headlight": (srgb(235, 235, 220), 0.1, 0.0, 0.6),
    "brake_light": (srgb(190, 16, 14), 0.2, 0.0, 0.8),
    "indicator": (srgb(230, 120, 20), 0.2, 0.0, 0.4),
    "cargo_olive": (srgb(98, 112, 64), 0.85, 0.0, 0.0),
    "cargo_orange": (srgb(214, 108, 40), 0.85, 0.0, 0.0),
    "cargo_canvas": (srgb(196, 176, 132), 0.9, 0.0, 0.0),
}

# Engine frame → Blender frame.
TO_BLENDER = Matrix(((0, 1, 0, 0), (-1, 0, 0, 0), (0, 0, 1, 0), (0, 0, 0, 1)))

WHEELBASE = 2.7
TRACK = 1.62
WHEEL_R = 0.40


def material(name):
    mat = bpy.data.materials.get(name)
    if mat:
        return mat
    colour, rough, metal, glow = MATERIALS[name]
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = next(n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Base Color"].default_value = colour
    bsdf.inputs["Roughness"].default_value = rough
    bsdf.inputs["Metallic"].default_value = metal
    if glow > 0:
        bsdf.inputs["Emission Color"].default_value = colour
        bsdf.inputs["Emission Strength"].default_value = glow
    return mat


PARTS = []


def add(bm, mat, bevel=0.0):
    """Finish a part built in the engine frame."""
    if bevel > 0:
        bmesh.ops.bevel(bm, geom=list(bm.edges), offset=bevel, segments=1, affect="EDGES")
    bm.transform(TO_BLENDER)
    mesh = bpy.data.meshes.new(mat)
    bm.to_mesh(mesh)
    bm.free()
    obj = bpy.data.objects.new(mat, mesh)
    bpy.context.scene.collection.objects.link(obj)
    obj.data.materials.append(material(mat))
    for p in mesh.polygons:
        p.use_smooth = False
    PARTS.append(obj)
    return obj


def box(lo, hi, mat, bevel=0.0, slant_front_top=0.0):
    """A box from corner `lo` to `hi` (engine frame); the top front edge can
    lean back by `slant_front_top` metres (windscreens)."""
    bm = bmesh.new()
    (x0, y0, z0), (x1, y1, z1) = lo, hi
    v = [
        bm.verts.new((x, y, z))
        for x, y, z in [
            (x0, y0, z0), (x1, y0, z0), (x1, y1, z0), (x0, y1, z0),
            (x0, y0, z1), (x1 - slant_front_top, y0, z1), (x1 - slant_front_top, y1, z1), (x0, y1, z1),
        ]
    ]
    for f in [(0, 3, 2, 1), (4, 5, 6, 7), (0, 1, 5, 4), (1, 2, 6, 5), (2, 3, 7, 6), (3, 0, 4, 7)]:
        bm.faces.new([v[i] for i in f])
    bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
    return add(bm, mat, bevel)


def cylinder(centre, axis, radius, length, mat, sides=10):
    """A capped cylinder along engine axis 'x', 'y' or 'z'."""
    bm = bmesh.new()
    bmesh.ops.create_cone(bm, cap_ends=True, segments=sides, radius1=radius, radius2=radius, depth=length)
    rot = {"x": Matrix.Rotation(math.pi / 2, 4, "Y"), "y": Matrix.Rotation(math.pi / 2, 4, "X"), "z": Matrix()}[axis]
    bm.transform(Matrix.Translation(centre) @ rot)
    return add(bm, mat)


def bar(a, b, thickness, mat):
    """A square tube from a to b (engine frame)."""
    a, b = Vector(a), Vector(b)
    d = b - a
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    rot = d.to_track_quat("X", "Z").to_matrix().to_4x4()
    bm.transform(Matrix.Translation((a + b) / 2) @ rot @ Matrix.Diagonal((d.length, thickness, thickness, 1)))
    return add(bm, mat)


def build():
    front_axle = WHEELBASE / 2
    rear_axle = -WHEELBASE / 2
    nose = front_axle + 0.82
    tail = rear_axle - 0.86
    hw = 0.9
    # Lower body, bonnet and the tall cabin with a leaning windscreen.
    box((tail, -hw, 0.58), (nose, hw, 1.12), "paint", bevel=0.05)
    box((0.62, -hw + 0.03, 1.12), (nose - 0.04, hw - 0.03, 1.24), "paint", bevel=0.04)
    box((tail + 0.02, -hw + 0.02, 1.12), (0.7, hw - 0.02, 1.96), "paint", bevel=0.06, slant_front_top=0.22)
    # Glass: windscreen, side windows, rear window.
    # The windscreen sits just proud of the cabin's leaning front face.
    lean = 0.22 / (1.96 - 1.12)
    box((0.42, -hw + 0.12, 1.24), (0.7 - lean * 0.12 + 0.02, hw - 0.12, 1.86), "glass", slant_front_top=lean * 0.62)
    for s in (-1, 1):
        y = s * (hw - 0.005)
        box((tail + 0.25, min(y, y + s * 0.02), 1.3), (0.45, max(y, y + s * 0.02), 1.84), "glass")
    box((tail - 0.005, -hw + 0.18, 1.32), (tail + 0.02, hw - 0.18, 1.84), "glass")
    # Black trim: grille, bumpers, sills, fender flares.
    box((nose - 0.02, -0.55, 0.72), (nose + 0.03, 0.55, 1.08), "trim")
    box((nose - 0.05, -hw - 0.02, 0.46), (nose + 0.16, hw + 0.02, 0.66), "trim", bevel=0.03)
    box((tail - 0.14, -hw - 0.02, 0.46), (tail + 0.05, hw + 0.02, 0.66), "trim", bevel=0.03)
    for s in (-1, 1):
        box((rear_axle + 0.45, s * hw - 0.08, 0.44), (front_axle - 0.45, s * hw + 0.08, 0.52), "trim")
        for axle in (front_axle, rear_axle):
            lo = (axle - 0.56, min(s * hw, s * (hw + 0.1)), 0.86)
            hi = (axle + 0.56, max(s * hw, s * (hw + 0.1)), 0.98)
            box(lo, hi, "trim", bevel=0.03)
    # Beam axles with their diff casings, and a skid plate under the nose.
    for axle in (front_axle, rear_axle):
        bar((axle, -TRACK / 2 + 0.12, WHEEL_R), (axle, TRACK / 2 - 0.12, WHEEL_R), 0.1, "trim")
        box((axle - 0.16, -0.18, 0.26), (axle + 0.16, 0.18, 0.48), "trim", bevel=0.04)
    box((front_axle + 0.1, -0.5, 0.4), (nose, 0.5, 0.5), "metal")
    # Lamps.
    for s in (-1, 1):
        cylinder((nose + 0.02, s * 0.68, 0.94), "x", 0.11, 0.06, "headlight", sides=10)
        box((nose - 0.01, s * 0.86 - 0.04, 0.80), (nose + 0.03, s * 0.86 + 0.04, 0.86), "indicator")
        box((tail - 0.03, s * 0.8 - 0.07, 0.78), (tail + 0.01, s * 0.8 + 0.07, 1.04), "brake_light")
    # Bull bar.
    bx = nose + 0.24
    for s in (-1, 1):
        bar((nose + 0.1, s * 0.62, 0.5), (bx, s * 0.62, 0.6), 0.06, "metal")
        bar((bx, s * 0.62, 0.6), (bx, s * 0.5, 1.02), 0.06, "metal")
    bar((bx, -0.62, 0.6), (bx, 0.62, 0.6), 0.06, "metal")
    bar((bx, -0.5, 1.02), (bx, 0.5, 1.02), 0.06, "metal")
    # Snorkel up the right-hand pillar.
    bar((0.95, -hw - 0.06, 0.95), (0.62, -hw - 0.06, 1.2), 0.1, "trim")
    bar((0.62, -hw - 0.06, 1.2), (0.5, -hw - 0.06, 1.98), 0.1, "trim")
    # Mirrors.
    for s in (-1, 1):
        box((0.48, s * (hw + 0.02), 1.38), (0.6, s * (hw + 0.18), 1.5), "trim")
    # Spare wheel on the back door.
    cylinder((tail - 0.16, 0, 1.14), "x", 0.37, 0.24, "tyre", sides=12)
    cylinder((tail - 0.29, 0, 1.14), "x", 0.2, 0.04, "metal", sides=10)
    # Roof rack: rails on legs, cross bars, and a load.
    top = 1.96
    for s in (-1, 1):
        bar((tail + 0.1, s * 0.78, top + 0.14), (0.55, s * 0.78, top + 0.14), 0.05, "trim")
        for x in (tail + 0.15, -0.6, 0.45):
            bar((x, s * 0.78, top), (x, s * 0.78, top + 0.14), 0.05, "trim")
    for x in (tail + 0.15, -1.2, -0.6, 0.0, 0.45):
        bar((x, -0.78, top + 0.14), (x, 0.78, top + 0.14), 0.04, "trim")
    box((-1.6, -0.6, top + 0.17), (-0.85, 0.15, top + 0.5), "cargo_olive", bevel=0.04)
    box((-0.75, -0.55, top + 0.17), (0.25, 0.55, top + 0.42), "cargo_canvas", bevel=0.06)
    cylinder((-1.2, 0.48, top + 0.31), "x", 0.14, 0.9, "cargo_orange", sides=8)


LICENCE = """// Licence file for offroader.glb (`wp schema asset`). Generated by
// tools/models/blender/offroader.py; free to reuse.
(
  source: "own work (tools/models/blender/offroader.py)",
  author: "Waypoint contributors",
  licence: Cc0,
)
"""


def main():
    args = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    if not args:
        print("usage: blender -b -P offroader.py -- <out dir>")
        sys.exit(1)
    out_dir = args[0]
    os.makedirs(out_dir, exist_ok=True)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    build()
    path = os.path.join(out_dir, "offroader.glb")
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        export_yup=True,
        export_apply=True,
        export_texcoords=False,
        export_materials="EXPORT",
    )
    with open(os.path.join(out_dir, "offroader.asset.ron"), "w") as f:
        f.write(LICENCE)
    tris = sum(len(p.vertices) - 2 for o in PARTS for p in o.data.polygons)
    print(f"wrote {path} ({len(PARTS)} parts, {tris} triangles)")


main()
