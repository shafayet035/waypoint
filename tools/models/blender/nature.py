"""Stylised nature models for Waypoint: faceted rocks, layered cliffs, clumped
trees, pines, palms, bushes and grass tufts, written as .glb with licence files.

Our own work, released as CC0. Run headless from the repo root:

    /Applications/Blender.app/Contents/MacOS/Blender -b -P tools/models/blender/nature.py -- <out dir> [names...]

Every model is built from code with a fixed seed, so running it again gives the
same files. Shapes are low-poly with flat facets (the look comes from light,
haze and colour, not texture); colours are per material (the engine reads one
colour per material). Origins sit on the ground. Blender's -Y is the engine's
forward (+X).
"""

import math
import os
import random
import sys

import bmesh
import bpy
from mathutils import Vector, noise


def srgb(r, g, b):
    def lin(c):
        c /= 255.0
        return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4

    return (lin(r), lin(g), lin(b), 1.0)


PALETTE = {
    "sandstone_top": srgb(238, 168, 118),
    "sandstone": srgb(214, 124, 86),
    "sandstone_dark": srgb(184, 98, 72),
    "granite_top": srgb(214, 214, 208),
    "granite": srgb(170, 172, 176),
    "bark": srgb(96, 66, 46),
    "birch": srgb(232, 228, 216),
    "autumn_orange": srgb(236, 112, 34),
    "autumn_orange_dark": srgb(204, 82, 26),
    "autumn_red": srgb(206, 48, 36),
    "autumn_red_dark": srgb(164, 32, 30),
    "autumn_yellow": srgb(244, 194, 44),
    "autumn_yellow_dark": srgb(222, 160, 30),
    "leaf_green": srgb(112, 154, 62),
    "leaf_green_dark": srgb(80, 124, 48),
    "pine": srgb(44, 94, 62),
    "pine_dark": srgb(32, 74, 52),
    "palm_leaf": srgb(84, 132, 56),
    "palm_leaf_dark": srgb(62, 106, 46),
    "palm_trunk": srgb(138, 104, 72),
    "scrub": srgb(150, 142, 98),
    "scrub_dark": srgb(118, 110, 76),
    "grass": srgb(84, 150, 52),
    "grass_dry": srgb(196, 164, 82),
}


def reset():
    bpy.ops.wm.read_factory_settings(use_empty=True)


def material(name):
    mat = bpy.data.materials.get(name)
    if mat:
        return mat
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = next(n for n in mat.node_tree.nodes if n.type == "BSDF_PRINCIPLED")
    bsdf.inputs["Base Color"].default_value = PALETTE[name]
    bsdf.inputs["Roughness"].default_value = 0.85
    return mat


def new_object(name, bm):
    mesh = bpy.data.meshes.new(name)
    bm.to_mesh(mesh)
    bm.free()
    obj = bpy.data.objects.new(name, mesh)
    bpy.context.scene.collection.objects.link(obj)
    for p in mesh.polygons:
        p.use_smooth = False
    return obj


def assign(obj, names, pick):
    """Give each face one of `names` (materials), chosen by pick(face) → index."""
    for n in names:
        obj.data.materials.append(material(n))
    for p in obj.data.polygons:
        p.material_index = pick(p)


def join(objs, name):
    bpy.ops.object.select_all(action="DESELECT")
    for o in objs:
        o.select_set(True)
    bpy.context.view_layer.objects.active = objs[0]
    bpy.ops.object.join()
    obj = bpy.context.view_layer.objects.active
    obj.name = name
    return obj


def blob(seed, radius, squash=(1.0, 1.0, 1.0), subdiv=2, rough=0.25, flat_bottom=None):
    """A lumpy faceted ball: an icosphere pushed about by noise."""
    rng = random.Random(seed)
    bm = bmesh.new()
    bmesh.ops.create_icosphere(bm, subdivisions=subdiv, radius=radius)
    off = Vector((rng.uniform(-50, 50), rng.uniform(-50, 50), rng.uniform(-50, 50)))
    for v in bm.verts:
        n = noise.noise(v.co * (1.6 / radius) + off)
        v.co *= 1.0 + rough * n
        v.co.x *= squash[0]
        v.co.y *= squash[1]
        v.co.z *= squash[2]
        if flat_bottom is not None and v.co.z < flat_bottom:
            v.co.z = flat_bottom
    return bm


def rock(name, seed, size, kind="sandstone"):
    rng = random.Random(seed)
    squash = (rng.uniform(0.8, 1.3), rng.uniform(0.7, 1.1), rng.uniform(0.55, 0.85))
    bm = blob(seed, size * 0.5, squash, subdiv=2, rough=0.35, flat_bottom=-size * 0.12)
    obj = new_object(name, bm)
    obj.location.z = size * 0.1
    top, side = (f"{kind}_top", kind)
    assign(obj, [top, side], lambda p: 0 if p.normal.z > 0.55 else 1)
    return obj


def cliff(name, seed, width, depth, height, layers=5):
    """Stacked sandstone slabs, each a little smaller, edges softened: a mesa."""
    rng = random.Random(seed)
    parts = []
    z = 0.0
    for i in range(layers):
        h = height / layers * rng.uniform(0.75, 1.25)
        shrink = 1.0 - 0.1 * i * rng.uniform(0.6, 1.2)
        bm = bmesh.new()
        n = 11
        ring = []
        for k in range(n):
            a = 2 * math.pi * k / n + rng.uniform(-0.12, 0.12)
            r = rng.uniform(0.82, 1.08)
            ring.append(
                bm.verts.new((math.cos(a) * width * 0.5 * r * shrink, math.sin(a) * depth * 0.5 * r * shrink, z))
            )
        face = bm.faces.new(ring)
        ext = bmesh.ops.extrude_face_region(bm, geom=[face])
        top = [e for e in ext["geom"] if isinstance(e, bmesh.types.BMVert)]
        for v in top:
            v.co.z += h
        bmesh.ops.recalc_face_normals(bm, faces=bm.faces)
        # Soften the top edge so slabs read as worn rock, not cut stone.
        top_edges = [e for e in bm.edges if all(v.co.z > z + h - 1e-3 for v in e.verts)]
        bmesh.ops.bevel(bm, geom=top_edges, offset=min(h * 0.25, 1.2), segments=1, affect="EDGES")
        obj = new_object(f"{name}_{i}", bm)
        shade = ["sandstone", "sandstone_dark"][i % 2]
        assign(obj, ["sandstone_top", shade], lambda p: 0 if p.normal.z > 0.6 else 1)
        parts.append(obj)
        z += h * 0.98
    return join(parts, name)


def trunk(name, height, radius, mat, sides=6, bend=0.0, seed=0):
    rng = random.Random(seed)
    bm = bmesh.new()
    segments = 4
    rings = []
    for s in range(segments + 1):
        t = s / segments
        r = radius * (1.0 - 0.45 * t)
        x = bend * t * t
        ring = [
            bm.verts.new((x + math.cos(2 * math.pi * k / sides) * r, math.sin(2 * math.pi * k / sides) * r, height * t))
            for k in range(sides)
        ]
        rings.append(ring)
    for a, b in zip(rings, rings[1:]):
        for k in range(sides):
            bm.faces.new((a[k], a[(k + 1) % sides], b[(k + 1) % sides], b[k]))
    obj = new_object(name, bm)
    obj.rotation_euler.z = rng.uniform(0, math.pi)
    assign(obj, [mat], lambda p: 0)
    return obj


def broadleaf(name, seed, height, leaves, birch=False):
    """A trunk and a crown of 4-6 lumpy clumps in two tones of `leaves`."""
    rng = random.Random(seed)
    parts = [trunk(f"{name}_trunk", height * 0.62, height * 0.035, "birch" if birch else "bark", seed=seed)]
    clumps = rng.randint(4, 6)
    crown_z = height * 0.66
    for i in range(clumps):
        r = height * rng.uniform(0.16, 0.24)
        a = 2 * math.pi * i / clumps + rng.uniform(-0.3, 0.3)
        d = height * rng.uniform(0.08, 0.16)
        bm = blob(seed * 31 + i, r, (1.0, 1.0, rng.uniform(0.85, 1.1)), subdiv=2, rough=0.3)
        obj = new_object(f"{name}_clump{i}", bm)
        obj.location = (math.cos(a) * d, math.sin(a) * d, crown_z + rng.uniform(-0.12, 0.18) * height)
        tone = leaves if i % 3 else leaves + "_dark"
        assign(obj, [tone], lambda p: 0)
        parts.append(obj)
    top = blob(seed * 7, height * 0.2, subdiv=2, rough=0.3)
    obj = new_object(f"{name}_top", top)
    obj.location = (0, 0, height - height * 0.18)
    assign(obj, [leaves], lambda p: 0)
    parts.append(obj)
    return join(parts, name)


def pine(name, seed, height):
    rng = random.Random(seed)
    parts = [trunk(f"{name}_trunk", height * 0.3, height * 0.03, "bark", seed=seed)]
    tiers = 4
    for i in range(tiers):
        t = i / tiers
        r = height * (0.24 - 0.045 * i) * rng.uniform(0.9, 1.1)
        h = height * 0.34
        bm = bmesh.new()
        bmesh.ops.create_cone(bm, cap_ends=True, segments=7, radius1=r, radius2=0.0, depth=h)
        obj = new_object(f"{name}_tier{i}", bm)
        obj.location = (0, 0, height * (0.22 + 0.19 * i) + h * 0.5)
        obj.rotation_euler.z = rng.uniform(0, 1)
        assign(obj, ["pine" if i % 2 else "pine_dark"], lambda p: 0)
        parts.append(obj)
    return join(parts, name)


def palm(name, seed, height):
    rng = random.Random(seed)
    lean = height * rng.uniform(0.08, 0.16)
    parts = [trunk(f"{name}_trunk", height, height * 0.035, "palm_trunk", sides=6, bend=lean, seed=seed)]
    fronds = 8
    for i in range(fronds):
        a = 2 * math.pi * i / fronds + rng.uniform(-0.15, 0.15)
        length = height * rng.uniform(0.38, 0.48)
        bm = bmesh.new()
        segs = 5
        left, right, mid = [], [], []
        for s in range(segs + 1):
            t = s / segs
            w = length * 0.11 * math.sin(math.pi * min(t * 1.15, 1.0))
            droop = -length * 0.55 * t * t
            mid.append(bm.verts.new((length * t, 0, droop + length * 0.08 * t)))
            left.append(bm.verts.new((length * t, w, droop)))
            right.append(bm.verts.new((length * t, -w, droop)))
        for s in range(segs):
            bm.faces.new((mid[s], left[s], left[s + 1], mid[s + 1]))
            bm.faces.new((mid[s], mid[s + 1], right[s + 1], right[s]))
        obj = new_object(f"{name}_frond{i}", bm)
        obj.location = (lean, 0, height)
        obj.rotation_euler.z = a
        assign(obj, ["palm_leaf" if i % 2 else "palm_leaf_dark"], lambda p: 0)
        parts.append(obj)
    obj = join(parts, name)
    # Fronds are single sheets: show both sides.
    for m in obj.data.materials:
        m.use_backface_culling = False
    return obj


def bush(name, seed, size, leaves):
    rng = random.Random(seed)
    parts = []
    for i in range(3):
        bm = blob(seed * 13 + i, size * rng.uniform(0.32, 0.45), (1, 1, 0.75), subdiv=2, rough=0.3, flat_bottom=-0.05)
        obj = new_object(f"{name}_{i}", bm)
        a = 2 * math.pi * i / 3
        obj.location = (math.cos(a) * size * 0.22, math.sin(a) * size * 0.22, size * 0.25)
        assign(obj, [leaves if i % 2 else leaves + "_dark"], lambda p: 0)
        parts.append(obj)
    return join(parts, name)


def grass(name, seed, height, colour):
    """A tuft of blades: thin triangles leaning out from the centre."""
    rng = random.Random(seed)
    bm = bmesh.new()
    for i in range(14):
        a = rng.uniform(0, 2 * math.pi)
        r = rng.uniform(0.0, 0.18)
        lean = rng.uniform(0.15, 0.4)
        h = height * rng.uniform(0.6, 1.0)
        base = Vector((math.cos(a) * r, math.sin(a) * r, 0))
        side = Vector((-math.sin(a), math.cos(a), 0)) * 0.035
        tip = base + Vector((math.cos(a) * lean * h, math.sin(a) * lean * h, h))
        bm.faces.new((bm.verts.new(base - side), bm.verts.new(base + side), bm.verts.new(tip)))
    obj = new_object(name, bm)
    assign(obj, [colour], lambda p: 0)
    for m in obj.data.materials:
        m.use_backface_culling = False
    return obj


MODELS = {
    "rock_sandstone_a": lambda: rock("rock_sandstone_a", 1, 2.2),
    "rock_sandstone_b": lambda: rock("rock_sandstone_b", 2, 3.4),
    "rock_sandstone_c": lambda: rock("rock_sandstone_c", 3, 1.2),
    "rock_granite_a": lambda: rock("rock_granite_a", 4, 2.6, "granite"),
    "rock_granite_b": lambda: rock("rock_granite_b", 5, 4.0, "granite"),
    "mesa_a": lambda: cliff("mesa_a", 11, 34.0, 22.0, 28.0, layers=6),
    "mesa_b": lambda: cliff("mesa_b", 12, 22.0, 16.0, 18.0, layers=4),
    "mesa_c": lambda: cliff("mesa_c", 13, 48.0, 30.0, 40.0, layers=7),
    "tree_autumn_orange": lambda: broadleaf("tree_autumn_orange", 21, 9.0, "autumn_orange"),
    "tree_autumn_red": lambda: broadleaf("tree_autumn_red", 22, 8.0, "autumn_red"),
    "tree_autumn_yellow": lambda: broadleaf("tree_autumn_yellow", 23, 10.0, "autumn_yellow", birch=True),
    "tree_green": lambda: broadleaf("tree_green", 24, 9.0, "leaf_green"),
    "tree_birch": lambda: broadleaf("tree_birch", 25, 11.0, "autumn_yellow", birch=True),
    "pine_a": lambda: pine("pine_a", 31, 12.0),
    "palm_a": lambda: palm("palm_a", 41, 9.0),
    "bush_green": lambda: bush("bush_green", 51, 1.4, "leaf_green"),
    "bush_autumn": lambda: bush("bush_autumn", 52, 1.2, "autumn_orange"),
    "bush_dry": lambda: bush("bush_dry", 53, 1.1, "scrub"),
    "grass_tuft": lambda: grass("grass_tuft", 61, 0.7, "grass"),
    "grass_dry": lambda: grass("grass_dry", 62, 0.6, "grass_dry"),
}

LICENCE = """// Licence file for {name}.glb (`wp schema asset`). Generated by
// tools/models/blender/nature.py; free to reuse.
(
  source: "own work (tools/models/blender/nature.py)",
  author: "Waypoint contributors",
  licence: Cc0,
)
"""


def export(name, out_dir):
    reset()
    MODELS[name]()
    path = os.path.join(out_dir, name + ".glb")
    bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        export_yup=True,
        export_apply=True,
        export_normals=True,
        export_texcoords=False,
        export_materials="EXPORT",
    )
    with open(os.path.join(out_dir, name + ".asset.ron"), "w") as f:
        f.write(LICENCE.format(name=name))
    tris = sum(len(p.vertices) - 2 for o in bpy.data.objects if o.type == "MESH" for p in o.data.polygons)
    print(f"wrote {path} ({tris} triangles)")


def main():
    args = sys.argv[sys.argv.index("--") + 1 :] if "--" in sys.argv else []
    if not args:
        print("usage: blender -b -P nature.py -- <out dir> [names...]")
        sys.exit(1)
    out_dir, names = args[0], args[1:] or list(MODELS)
    os.makedirs(out_dir, exist_ok=True)
    for name in names:
        export(name, out_dir)


main()
