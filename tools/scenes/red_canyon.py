#!/usr/bin/env python3
"""Writes examples/red_canyon/scenery/canyon.scenery.ron: the mesas, boulders
and palms of Red Canyon, placed round the loop road (roads/canyon.road.ron).
Seeded, so it always writes the same file. Roadside rows (rocks, scrub, grass)
are left to the engine's prop_rows.

    python3 tools/scenes/red_canyon.py
"""
import math
import random

# The loop's roads: (from, through, to), drawn by the engine as a Catmull-Rom
# spline through the three points (wp_road::geometry::catmull_rom).
ARCS = [
    ((0, 0), (120, 80), (260, 90)),
    ((260, 90), (390, 70), (470, -40)),
    ((470, -40), (470, -190), (300, -260)),
    ((300, -260), (170, -280), (40, -200)),
    ((40, -200), (-40, -90), (0, 0)),
]
OASIS = (330.0, -200.0)


def sample(a, b, c, per_segment=24):
    """The engine's spline through a, b, c (end points repeated)."""
    ctrl = [a, b, c]
    get = lambda i: ctrl[max(0, min(len(ctrl) - 1, i))]
    pts = []
    for i in range(len(ctrl) - 1):
        p0, p1, p2, p3 = get(i - 1), get(i), get(i + 1), get(i + 2)
        for k in range(per_segment):
            t = k / per_segment
            t2, t3 = t * t, t * t * t
            pts.append(tuple(
                0.5 * (2 * p1[j] + (-p0[j] + p2[j]) * t + (2 * p0[j] - 5 * p1[j] + 4 * p2[j] - p3[j]) * t2
                       + (-p0[j] + 3 * p1[j] - 3 * p2[j] + p3[j]) * t3)
                for j in range(2)
            ))
    pts.append(c)
    return pts


def lane_point(arc, along_m, lane_m=1.75):
    """A point `along_m` along road `arc`, in its right-hand lane, and its heading."""
    pts = sample(*arc, per_segment=200)
    d = 0.0
    for (x0, y0), (x1, y1) in zip(pts, pts[1:]):
        step = math.hypot(x1 - x0, y1 - y0)
        if d + step >= along_m:
            dx, dy = (x1 - x0) / step, (y1 - y0) / step
            return (x0 + dy * lane_m, y0 - dx * lane_m), math.degrees(math.atan2(dy, dx))
        d += step
    raise ValueError("road too short")


ROAD = [p for arc in ARCS for p in sample(*arc)]


def road_distance(p):
    return min(math.hypot(p[0] - q[0], p[1] - q[1]) for q in ROAD)


def main():
    rng = random.Random(7)
    props = []
    placed = []  # (position, radius)

    def place(model, p, radius, scale, solid, heading=None):
        if road_distance(p) < radius + 8:
            return False
        if any(math.hypot(p[0] - q[0], p[1] - q[1]) < radius + r for q, r in placed):
            return False
        placed.append((p, radius))
        h = rng.uniform(0, 360) if heading is None else heading
        props.append(
            f'    (model: "assets/nature/{model}.glb", position: ({p[0]:.1f}, {p[1]:.1f}), '
            f"heading_deg: {h:.0f}, scale: {scale:.2f}, solid_m: {solid:.1f}),"
        )
        return True

    # Canyon walls: mesas along both sides of the road, 30-90 m out.
    mesas = [("mesa_a", 18.0), ("mesa_b", 12.0), ("mesa_c", 25.0)]
    for i in range(0, len(ROAD) - 1, 9):
        (x0, y0), (x1, y1) = ROAD[i], ROAD[i + 1]
        dx, dy = x1 - x0, y1 - y0
        n = math.hypot(dx, dy) or 1.0
        left = (-dy / n, dx / n)
        for side in (1, -1):
            if rng.random() < 0.35:
                continue
            model, r = rng.choice(mesas)
            scale = rng.uniform(0.7, 1.3)
            out = r * scale + rng.uniform(14, 50)
            p = (x0 + left[0] * side * out, y0 + left[1] * side * out)
            # Keep the oasis open.
            if math.hypot(p[0] - OASIS[0], p[1] - OASIS[1]) < 70:
                continue
            place(model, p, r * scale, scale, r * scale * 0.6)
    # The skyline: huge mesas far out round the loop.
    for k in range(18):
        a = 2 * math.pi * k / 18 + rng.uniform(-0.1, 0.1)
        d = rng.uniform(420, 620)
        p = (230 + math.cos(a) * d, -90 + math.sin(a) * d)
        scale = rng.uniform(2.2, 3.4)
        place("mesa_c", p, 25 * scale, scale, 0.0)
    # Boulders scattered in the open between the walls.
    for _ in range(140):
        p = (rng.uniform(-150, 620), rng.uniform(-420, 230))
        model = rng.choice(["rock_sandstone_a", "rock_sandstone_b", "rock_sandstone_c"])
        scale = rng.uniform(0.8, 2.2)
        place(model, p, 1.8 * scale, scale, 0.9 * scale)
    # The oasis: palms and green scrub round a hollow.
    for _ in range(26):
        a = rng.uniform(0, 2 * math.pi)
        d = rng.uniform(6, 34)
        p = (OASIS[0] + math.cos(a) * d, OASIS[1] + math.sin(a) * d)
        if rng.random() < 0.7:
            place("palm_a", p, 1.2, rng.uniform(0.8, 1.25), 0.25)
        else:
            place("bush_green", p, 1.0, rng.uniform(0.9, 1.6), 0.0)

    roads = '["wash_run", "the_narrows", "oasis_bend", "long_flat", "overlook_climb"]'
    text = f"""// Red Canyon's scenery, written by tools/scenes/red_canyon.py (edit that, not this).
// Layered mesas wall the loop road, a skyline of giant mesas rings it, boulders
// lie in the open and palms stand round the oasis. The roadside rows (boulders,
// scrub and dry grass) are generated by the engine along every road.
(
  name: "canyon",
  props: [
{chr(10).join(props)}
  ],
  prop_rows: [
    (models: ["assets/nature/rock_sandstone_a.glb", "assets/nature/rock_sandstone_c.glb"],
     roads: {roads}, spacing_m: 13.0, kerb_m: 4.5, solid_m: 0.8, seed: 1),
    (models: ["assets/nature/rock_sandstone_b.glb"],
     roads: {roads}, spacing_m: 31.0, kerb_m: 10.0, solid_m: 1.5, seed: 2),
    (models: ["assets/nature/bush_dry.glb", "assets/nature/grass_dry.glb"],
     roads: {roads}, spacing_m: 7.0, kerb_m: 2.0, seed: 3),
    (models: ["assets/nature/grass_dry.glb"],
     roads: {roads}, spacing_m: 3.0, kerb_m: 1.0, seed: 4),
  ],
)
"""
    with open("examples/red_canyon/scenery/canyon.scenery.ron", "w") as f:
        f.write(text)
    print(f"wrote {len(props)} props")
    for name, arc, along in [("trailhead", ARCS[0], 20.0), ("overlook", ARCS[4], 25.0), ("oasis", ARCS[3], 30.0)]:
        (x, y), h = lane_point(arc, along)
        print(f'spawn {name}: (name: "{name}", position: ({x:.1f}, {y:.1f}), heading_deg: {h:.1f}),')


main()
