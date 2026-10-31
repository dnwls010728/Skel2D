"""Converts an exported Spine sample (JSON) into a Skel2D document.

Usage: py -3 convert_spine.py <sample folder> <project> <output folder> <document name>

Reads <sample folder>/export/<project>.json and the unpacked images folder, and writes
<output folder>/<document name>.skel.json with its images and license. What Skel2D
cannot express is reported on stdout instead of being dropped silently.
"""
import json
import math
import shutil
import sys
from pathlib import Path

RESAMPLE_STEP = 1 / 60
PHYSICS_FIELDS = {
    "x": "x", "y": "y", "rotate": "rotate", "scaleX": "scale_x", "shearX": "shear_x", "limit": "limit",
    "step": "step", "inertia": "inertia", "strength": "strength", "damping": "damping", "wind": "wind",
    "gravity": "gravity", "mix": "mix", "inertiaGlobal": "inertia_global", "strengthGlobal": "strength_global",
    "dampingGlobal": "damping_global", "massGlobal": "mass_global", "windGlobal": "wind_global",
    "gravityGlobal": "gravity_global", "mixGlobal": "mix_global",
}
OMITTED_INERTIA = 0.5
OMITTED_DAMPING = 0.85
# The sample's wind and gravity are per 100 world units: unscaled they would not move anything.
FORCE_SCALE = 100
PHYSICS_KEYS = {"inertia", "strength", "damping", "mass", "wind", "gravity", "mix"}


def num(v):
    return round(v, 5)


def mul(p, m):
    pa, pc, ptx, pd, pe, pty = p
    a, c, x, d, e, y = m
    return (pa * a + pc * d, pa * c + pc * e, pa * x + pc * y + ptx,
            pd * a + pe * d, pd * c + pe * e, pd * x + pe * y + pty)


def world_of(bones, stats):
    worlds = {}
    for b in bones:
        r = math.radians(b.get("rotation", 0))
        sx, sy = b.get("scaleX", 1), b.get("scaleY", 1)
        a, c, d, e = math.cos(r) * sx, -math.sin(r) * sy, math.sin(r) * sx, math.cos(r) * sy
        x, y = b.get("x", 0), b.get("y", 0)
        local = (a, c, x, d, e, y)
        p = worlds.get(b.get("parent"))
        inherit = b.get("inherit", "normal")
        if p is None:
            worlds[b["name"]] = local
        elif inherit == "normal":
            worlds[b["name"]] = mul(p, local)
        elif inherit == "noRotationOrReflection":
            # The parent's stretch without its rotation, as Skel2D evaluates it.
            along = math.hypot(p[0], p[3])
            det = abs(p[0] * p[4] - p[1] * p[3])
            t = math.atan2(p[3], p[0])
            spin = lambda v: (math.cos(v), -math.sin(v), 0, math.sin(v), math.cos(v), 0)
            linear = mul(mul(mul(spin(t), (along, 0, 0, 0, det / along, 0)), spin(-t)), local)
            tx = p[0] * x + p[1] * y + p[2]
            ty = p[3] * x + p[4] * y + p[5]
            worlds[b["name"]] = (linear[0], linear[1], tx, linear[3], linear[4], ty)
        else:
            stats["dropped"].append(f"setup world of inherit {inherit}")
            worlds[b["name"]] = mul(p, local)
    return worlds


def apply(m, x, y):
    return (m[0] * x + m[1] * y + m[2], m[3] * x + m[4] * y + m[5])


def convert_mesh(name, src, slot_bone, bone_names, worlds, stats):
    uvs, raw = src["uvs"], src["vertices"]
    count = len(uvs) // 2
    vertices, palette = [], []
    weighted = len(raw) != len(uvs)
    i = 0
    for v in range(count):
        out = {"u": num(uvs[v * 2]), "v": num(uvs[v * 2 + 1])}
        if weighted:
            n = int(raw[i])
            i += 1
            placed, weights = [], []
            for _ in range(n):
                bone = bone_names[int(raw[i])]
                placed.append((apply(worlds[bone], raw[i + 1], raw[i + 2]), raw[i + 3]))
                weights.append({"bone": bone, "weight": num(raw[i + 3])})
                if bone not in palette:
                    palette.append(bone)
                i += 4
            x = sum(p[0] * w for p, w in placed)
            y = sum(p[1] * w for p, w in placed)
            # A bone can hold the vertex away from the blended position.
            for (p, _), weight in zip(placed, weights):
                drift = math.hypot(p[0] - x, p[1] - y)
                stats["bind_drift"] = max(stats["bind_drift"], drift)
                if drift >= 0.001:
                    weight["offset"] = [num(p[0] - x), num(p[1] - y)]
            out["weights"] = weights
        else:
            x, y = apply(worlds[slot_bone], raw[v * 2], raw[v * 2 + 1])
        out["x"], out["y"] = num(x), num(y)
        vertices.append(out)
    hull = src["hull"]
    boundary = {tuple(sorted((j, (j + 1) % hull))) for j in range(hull)}
    pairs = [(src["edges"][j] // 2, src["edges"][j + 1] // 2) for j in range(0, len(src["edges"]), 2)]
    edges = [list(p) for p in pairs if tuple(sorted(p)) not in boundary]
    area = sum(vertices[j]["x"] * vertices[(j + 1) % hull]["y"] - vertices[(j + 1) % hull]["x"] * vertices[j]["y"] for j in range(hull))
    triangles = [src["triangles"][j:j + 3] for j in range(0, len(src["triangles"]), 3)]
    if area > 0:
        # Skel2D wants a clockwise hull: reverse the hull vertices and renumber.
        stats["ccw_hulls"] += 1
        flip = lambda j: hull - 1 - j if j < hull else j
        vertices[:hull] = vertices[:hull][::-1]
        triangles = [[flip(j) for j in t] for t in triangles]
        edges = [[flip(j) for j in e] for e in edges]
    mesh = {"type": "mesh", "name": name, "image": f"images/{src.get('path', name)}.png", "hull": hull,
            "vertices": vertices, "triangles": triangles}
    if palette:
        mesh["bones"] = palette
    if edges:
        mesh["edges"] = edges
    return mesh


def bezier_value(c, t0, v0, t1, v1, time):
    """The value of one absolute-coordinates curve channel at `time`."""
    lo, hi = 0.0, 1.0
    for _ in range(40):
        s = (lo + hi) / 2
        u = 1 - s
        x = u * u * u * t0 + 3 * u * u * s * c[0] + 3 * u * s * s * c[2] + s * s * s * t1
        if x < time:
            lo = s
        else:
            hi = s
    s = (lo + hi) / 2
    u = 1 - s
    return u * u * u * v0 + 3 * u * u * s * c[1] + 3 * u * s * s * c[3] + s * s * s * v1


def normalized(c, t0, v0, t1, v1):
    """A channel's curve as 0..1 control points; `flat` when it never moves,
    None when it moves between two equal values, which needs resampling."""
    if abs(v1 - v0) < 1e-6:
        return "flat" if abs(c[1] - v0) < 1e-6 and abs(c[3] - v0) < 1e-6 else None
    dt, dv = t1 - t0, v1 - v0
    return [(c[0] - t0) / dt, (c[1] - v0) / dv, (c[2] - t0) / dt, (c[3] - v0) / dv]


def convert_keys(keys, fields, defaults, stats, hold_setup, channels_ok=True):
    """`fields` names the value channels. Returns [(time, values, curve)], where
    curve is "linear", "stepped", {"bezier": ...} or a per-channel list of those."""
    def values(k):
        return [k.get(f, d) for f, d in zip(fields, defaults)]

    out = []
    if hold_setup and keys and keys[0].get("time", 0) > 0:
        out.append((0.0, list(defaults), "stepped"))
    for i, key in enumerate(keys):
        t0, v0 = key.get("time", 0), values(key)
        curve = key.get("curve")
        if i + 1 == len(keys) or curve is None:
            out.append((t0, v0, "linear"))
            continue
        if curve == "stepped":
            out.append((t0, v0, "stepped"))
            continue
        t1, v1 = keys[i + 1].get("time", 0), values(keys[i + 1])
        channels = [curve[j * 4:j * 4 + 4] for j in range(len(fields))]
        shapes = [normalized(c, t0, a, t1, b) for c, a, b in zip(channels, v0, v1)]
        moving = [s for s in shapes if s != "flat"]
        expressible = None not in moving
        same = expressible and all(max(abs(a - b) for a, b in zip(moving[0], s)) < 1e-3 for s in moving)
        if not moving:
            out.append((t0, v0, "linear"))
        elif same:
            out.append((t0, v0, {"bezier": [num(v) for v in moving[0]]}))
        elif expressible and channels_ok:
            stats["channel_curves"] += 1
            out.append((t0, v0, ["linear" if s == "flat" else {"bezier": [num(v) for v in s]} for s in shapes]))
        else:
            # A channel leaves and returns to the same value, which a 0..1 curve cannot hold.
            stats["resampled_segments"] += 1
            steps = max(2, math.ceil((t1 - t0) / RESAMPLE_STEP))
            for s in range(steps):
                time = t0 + (t1 - t0) * s / steps
                out.append((time, [bezier_value(c, t0, a, t1, b, time) for c, a, b in zip(channels, v0, v1)], "linear"))
    return out


def write_keys(converted, scalar):
    result = []
    for time, values, curve in converted:
        key = {"time": num(time), "value": num(values[0]) if scalar else [num(v) for v in values]}
        if isinstance(curve, list):
            key["curves"] = curve
        elif curve != "linear":
            key["curve"] = curve
        result.append(key)
    return result


def color_parts(hex_color):
    return [int(hex_color[i:i + 2], 16) / 255 for i in (0, 2, 4, 6)] if len(hex_color) == 8 else [int(hex_color[i:i + 2], 16) / 255 for i in (0, 2, 4)] + [1.0]


def write_color_keys(converted):
    result = []
    for time, values, curve in converted:
        hex_color = "#" + "".join(f"{min(255, max(0, round(v * 255))):02x}" for v in values)
        key = {"time": num(time), "value": hex_color}
        if isinstance(curve, list):
            key["curves"] = curve
        elif curve != "linear":
            key["curve"] = curve
        result.append(key)
    return result


def convert_slot_color(keys, setup, stats):
    # Skel2D multiplies keys with the setup color; Spine keys replace it.
    base = color_parts(setup)
    prepared = []
    for k in keys:
        parts = color_parts(k["color"])
        key = {"time": k.get("time", 0), **{f: (v / b if b else v) for f, v, b in zip("rgba", parts, base)}}
        curve = k.get("curve")
        if isinstance(curve, list):
            # Curve values are absolute: divide them the same way.
            curve = [v / base[i // 4] if i % 2 and base[i // 4] else v for i, v in enumerate(curve)]
        if curve is not None:
            key["curve"] = curve
        prepared.append(key)
    return write_color_keys(convert_keys(prepared, list("rgba"), [1.0] * 4, stats, True))


def convert_ik_keys(keys, setup, stats):
    """`setup` holds the constraint's mix, softness and switches. An omitted Spine
    key field means mix 1, softness 0, bendPositive true, compress and stretch
    false; Skel2D omits a field that equals the setup value."""
    switches = ("bendPositive", "compress", "stretch")
    spine_default = {"bendPositive": True, "compress": False, "stretch": False}
    prepared = [{**k, "mix": k.get("mix", 1), "softness": k.get("softness", 0)} for k in keys]
    converted = convert_keys(prepared, ["mix", "softness"], [setup["mix"], setup["softness"]], stats, True)
    stats["ik_keys"] += len(keys)
    result = []
    for time, values, curve in converted:
        # Hold keys and resampled keys carry the switches of the Spine key in effect.
        held = [k for k in keys if k.get("time", 0) <= time + 1e-9]
        state = {f: (held[-1].get(f, spine_default[f]) if held else setup[f]) for f in switches}
        mix, softness = num(values[0]), num(values[1])
        value = {"mix": mix}
        if abs(softness - setup["softness"]) > 1e-6:
            value["softness"] = softness
        for f, name in zip(switches, ("bend_positive", "compress", "stretch")):
            if state[f] != setup[f]:
                value[name] = state[f]
        key = {"time": num(time), "value": value["mix"] if len(value) == 1 else value}
        if isinstance(curve, list):
            key["curves"] = curve
        elif curve != "linear":
            key["curve"] = curve
        result.append(key)
    return result


def convert_draw_order(keys, slot_names, stats):
    """Each listed slot moves from its setup index by `offset`; the others fill the
    free positions in setup order. Skel2D wants the complete order of every key."""
    result = []
    for k in keys:
        key = {"time": num(k.get("time", 0))}
        offsets = k.get("offsets", [])
        if offsets:
            order = [None] * len(slot_names)
            for o in offsets:
                order[slot_names.index(o["slot"]) + o["offset"]] = o["slot"]
            moved = {o["slot"] for o in offsets}
            rest = iter(n for n in slot_names if n not in moved)
            key["order"] = [n if n is not None else next(rest) for n in order]
        result.append(key)
    stats["draw_order_keys"] += len(result)
    return result


def convert_event_keys(keys, stats):
    result = []
    for k in keys:
        key = {"time": num(k.get("time", 0)), "name": k["name"]}
        for field in ("int", "float", "string"):
            if field in k:
                key[field] = k[field]
        for field in ("volume", "balance"):
            if field in k:
                stats["dropped"].append(f"event key {field}")
        result.append(key)
    stats["events"] += len(result)
    return result


TRANSFORM_MIX = (("mixRotate", "rotate"), ("mixX", "x"), ("mixY", "y"), ("mixScaleX", "scale_x"), ("mixScaleY", "scale_y"), ("mixShearY", "shear_y"))


def convert_transform_keys(keys, constraint, stats):
    # A key's omitted mix is 1, its Y mixes copy the X ones; unmapped parts stay at 0.
    allowed = {src for src, dst in TRANSFORM_MIX if constraint["mix_" + dst] != 0 or dst in constraint["_mapped"]}
    fields = [src for src, _ in TRANSFORM_MIX if src in allowed]
    prepared = []
    for k in keys:
        key = dict(k)
        x, sx = k.get("mixX", 1), k.get("mixScaleX", 1)
        key.update({"mixRotate": k.get("mixRotate", 1), "mixX": x, "mixY": k.get("mixY", x), "mixScaleX": sx, "mixScaleY": k.get("mixScaleY", sx), "mixShearY": k.get("mixShearY", 1)})
        prepared.append(key)
    setup = {src: constraint["mix_" + dst] for src, dst in TRANSFORM_MIX}
    converted = convert_keys(prepared, fields, [setup[f] for f in fields], stats, True, channels_ok=False)
    result = []
    for time, values, curve in converted:
        by_field = dict(zip(fields, values))
        mix = {dst: num(by_field.get(src, 0.0)) for src, dst in TRANSFORM_MIX}
        key = {"time": num(time), "value": mix}
        if curve != "linear":
            key["curve"] = curve
        result.append(key)
    return result


def vertex_offsets(key, mesh_src, slot_bone, bone_names, worlds):
    """A Spine deform key as one setup-pose offset per vertex: each bone's local
    offset is turned into skeleton space and blended by the weights."""
    uvs, raw = mesh_src["uvs"], mesh_src["vertices"]
    count = len(uvs) // 2
    weighted = len(raw) != len(uvs)
    if weighted:
        i, size = 0, 0
        while i < len(raw):
            n = int(raw[i])
            size += n * 2
            i += 1 + n * 4
    else:
        size = len(raw)
    flat = [0.0] * size
    for j, v in enumerate(key.get("vertices", [])):
        flat[key.get("offset", 0) + j] = v
    out = []
    if not weighted:
        m = worlds[slot_bone]
        for v in range(count):
            dx, dy = flat[v * 2], flat[v * 2 + 1]
            out.append((m[0] * dx + m[1] * dy, m[3] * dx + m[4] * dy))
        return out
    i = f = 0
    for v in range(count):
        n = int(raw[i])
        i += 1
        x = y = 0.0
        for _ in range(n):
            m = worlds[bone_names[int(raw[i])]]
            w = raw[i + 3]
            dx, dy = flat[f], flat[f + 1]
            x += w * (m[0] * dx + m[1] * dy)
            y += w * (m[3] * dx + m[4] * dy)
            f += 2
            i += 4
        out.append((x, y))
    return out


def convert_deform(src, ctx, stats):
    out = {}
    durations = [0.0]
    for skin, slots in src.items():
        for slot, atts in slots.items():
            for name, timelines in atts.items():
                for kind, keys in timelines.items():
                    if kind != "deform":
                        stats["dropped"].append(f"attachment timeline {kind}")
                        continue
                    mesh_src = ctx["attachments"][slot][name]
                    flip = ctx["flips"].get((slot, name))
                    written = []
                    if keys[0].get("time", 0) > 0:
                        written.append({"time": 0.0, "value": None, "curve": "stepped"})
                    for i, key in enumerate(keys):
                        offsets = vertex_offsets(key, mesh_src, ctx["bones_of_slot"][slot], ctx["bone_names"], ctx["worlds"])
                        value = [[num(a), num(b)] for a, b in offsets]
                        if flip:
                            value[:flip] = value[:flip][::-1]
                        k = {"time": num(key.get("time", 0)), "value": value}
                        curve = key.get("curve")
                        if curve == "stepped":
                            k["curve"] = "stepped"
                        elif isinstance(curve, list) and i + 1 < len(keys):
                            t0, t1 = key.get("time", 0), keys[i + 1].get("time", 0)
                            k["curve"] = {"bezier": [num(v) for v in normalized(curve, t0, 0.0, t1, 1.0)]}
                        written.append(k)
                    for k in written:
                        if k["value"] is None:
                            k["value"] = [[0, 0] for _ in written[1]["value"]]
                    durations.append(keys[-1].get("time", 0))
                    out.setdefault(skin, {}).setdefault(slot, {})[name] = written
    return out, durations


def convert_animation(src, ctx, stats):
    out = {}
    durations = [0.0]
    bones = {}
    for bone, timelines in src.get("bones", {}).items():
        target = {}
        for kind, keys in timelines.items():
            durations.append(keys[-1].get("time", 0))
            if kind == "rotate":
                target["rotate"] = write_keys(convert_keys(keys, ["value"], [0.0], stats, True), True)
            elif kind == "translate":
                target["translate"] = write_keys(convert_keys(keys, ["x", "y"], [0.0, 0.0], stats, True), False)
            elif kind == "scale":
                target["scale"] = write_keys(convert_keys(keys, ["x", "y"], [1.0, 1.0], stats, True), False)
            elif kind == "shear":
                target["shear"] = write_keys(convert_keys(keys, ["x", "y"], [0.0, 0.0], stats, True), False)
            else:
                stats["dropped"].append(f"bone timeline {kind}")
        bones[bone] = target
    if bones:
        out["bones"] = bones
    slots = {}
    for slot, timelines in src.get("slots", {}).items():
        for kind, keys in timelines.items():
            durations.append(keys[-1].get("time", 0))
            if kind == "attachment":
                slots.setdefault(slot, {})["attachment"] = [{"time": num(k.get("time", 0)), "name": k.get("name") if k.get("name") in ctx["kept"][slot] else None} for k in keys]
            elif kind == "rgba":
                slots.setdefault(slot, {})["color"] = convert_slot_color(keys, ctx["slot_colors"].get(slot, "ffffffff"), stats)
            else:
                stats["dropped"].append(f"slot timeline {kind}")
    if slots:
        out["slots"] = slots
    physics = {}
    for name, timelines in src.get("physics", {}).items():
        target = {}
        for kind, keys in timelines.items():
            durations.append(keys[-1].get("time", 0))
            if kind == "reset":
                target["reset"] = [num(k.get("time", 0)) for k in keys]
            elif kind in PHYSICS_KEYS:
                written = write_keys(convert_keys(keys, ["value"], [0.0], stats, False), True)
                if kind in ("wind", "gravity"):
                    written = [{**k, "value": num(k["value"] * FORCE_SCALE)} for k in written]
                target[kind] = written
            else:
                stats["dropped"].append(f"physics timeline {kind}")
        physics[name] = target
    if physics:
        out["physics"] = physics
    ik = {}
    for name, keys in src.get("ik", {}).items():
        durations.append(keys[-1].get("time", 0))
        ik[name] = convert_ik_keys(keys, ctx["ik_setup"][name], stats)
    if ik:
        out["ik"] = ik
    transforms = {}
    for name, keys in src.get("transform", {}).items():
        durations.append(keys[-1].get("time", 0))
        constraint = ctx["transforms"][name]
        transforms[constraint["name"]] = convert_transform_keys(keys, constraint, stats)
    if transforms:
        out["transforms"] = transforms
    if "attachments" in src:
        deform, times = convert_deform(src["attachments"], ctx, stats)
        durations.extend(times)
        out["deform"] = deform
    if src.get("events"):
        durations.append(src["events"][-1].get("time", 0))
        out["events"] = convert_event_keys(src["events"], stats)
    if src.get("drawOrder"):
        durations.append(src["drawOrder"][-1].get("time", 0))
        out["draw_order"] = convert_draw_order(src["drawOrder"], ctx["slot_names"], stats)
    for section in src:
        if section not in ("bones", "slots", "physics", "ik", "transform", "attachments", "events", "drawOrder"):
            stats["dropped"].append(f"animation section {section}")
    out["duration"] = num(max(durations))
    return out


def convert_transform(c, order, taken, stats):
    props = c.get("properties", {})
    known = {"rotate": ["mixRotate"], "x": ["mixX"], "y": ["mixY"], "scaleX": ["mixScaleX"], "scaleY": ["mixScaleY"], "shearY": ["mixShearY"]}
    for key, p in props.items():
        if key not in known or set(p["to"]) != {key} or set(p) - {"to"}:
            stats["dropped"].append(f"transform mapping {key} on {c['name']}")
    for key in c:
        if key not in ("type", "name", "source", "bones", "properties", "x", "y", "rotation", "scaleX", "scaleY", "shearY",
                       "mixRotate", "mixX", "mixY", "mixScaleX", "mixScaleY", "mixShearY"):
            stats["dropped"].append(f"transform field {key} on {c['name']}")
    mix = lambda field, part, default: c.get(field, default) if part in props else 0
    mix_x, mix_scale = c.get("mixX", 1), c.get("mixScaleX", 1)
    # Skel2D keeps constraint names unique across kinds.
    name = f"{c['name']}-transform" if c["name"] in taken else c["name"]
    out = {"name": name, "order": order, "bones": c["bones"], "source": c["source"],
           "mix_rotate": mix("mixRotate", "rotate", 1), "mix_x": mix("mixX", "x", 1), "mix_y": mix("mixY", "y", mix_x),
           "mix_scale_x": mix("mixScaleX", "scaleX", 1), "mix_scale_y": mix("mixScaleY", "scaleY", mix_scale), "mix_shear_y": mix("mixShearY", "shearY", 1)}
    for src, dst in (("rotation", "offset_rotation"), ("x", "offset_x"), ("y", "offset_y"), ("scaleX", "offset_scale_x"), ("scaleY", "offset_scale_y"), ("shearY", "offset_shear_y")):
        if src in c:
            out[dst] = c[src]
    return out


def convert_ik(c, order, stats):
    for key in c:
        if key not in ("type", "name", "target", "bones", "mix", "bendPositive", "softness", "compress", "stretch"):
            stats["dropped"].append(f"IK field {key} on {c['name']}")
    out = {"name": c["name"], "order": order, "bones": c["bones"], "target": c["target"]}
    if "mix" in c:
        out["mix"] = c["mix"]
    if "bendPositive" in c:
        out["bend_positive"] = c["bendPositive"]
    for src, dst in (("softness", "softness"), ("compress", "compress"), ("stretch", "stretch")):
        if src in c:
            out[dst] = c[src]
    return out


INHERIT = {"onlyTranslation": "only_translation", "noRotationOrReflection": "no_rotation_or_reflection", "noScale": "no_scale", "noScaleOrReflection": "no_scale_or_reflection"}


def main(sample, project, out, doc_name):
    src = json.loads((sample / "export" / f"{project}.json").read_text(encoding="utf8"))
    stats = {"bind_drift": 0.0, "ccw_hulls": 0, "channel_curves": 0, "resampled_segments": 0, "ik_keys": 0, "events": 0, "draw_order_keys": 0, "dropped": []}
    bones_src = src["bones"]
    bone_names = [b["name"] for b in bones_src]
    worlds = world_of(bones_src, stats)
    physics_names = {c["name"] for c in src["constraints"] if c["type"] == "physics"}
    transforms = [convert_transform(c, order, physics_names, stats) for order, c in enumerate(src["constraints"]) if c["type"] == "transform"]
    transform_by_source_name = {}
    for c, t in zip([c for c in src["constraints"] if c["type"] == "transform"], transforms):
        t2 = dict(t)
        t2["_mapped"] = {{"rotate": "rotate", "x": "x", "y": "y", "scaleX": "scale_x", "scaleY": "scale_y", "shearY": "shear_y"}[k] for k in c.get("properties", {})}
        transform_by_source_name[c["name"]] = t2
    iks = [convert_ik(c, order, stats) for order, c in enumerate(src["constraints"]) if c["type"] == "ik"]
    for c in src["constraints"]:
        if c["type"] not in ("physics", "transform", "ik"):
            stats["dropped"].append(f"{c['type']} constraint")

    bones = []
    for b in bones_src:
        bone = {"name": b["name"]}
        if b.get("parent"):
            bone["parent"] = b["parent"]
        for key in ("x", "y", "rotation", "length"):
            if key in b:
                bone[key] = b[key]
        if "color" in b:
            bone["color"] = "#" + b["color"]
        if b.get("scaleX", 1) != 1:
            bone["scale_x"] = b["scaleX"]
        if b.get("scaleY", 1) != 1:
            bone["scale_y"] = b["scaleY"]
        if b.get("inherit", "normal") != "normal":
            bone["inherit"] = INHERIT[b["inherit"]]
        bones.append(bone)

    images = set()
    attachments = src["skins"][0]["attachments"]
    if len(src["skins"]) > 1:
        stats["dropped"].append("skins other than the first")
    flips = {}
    slots = []
    for s in src["slots"]:
        slot = {"name": s["name"], "bone": s["bone"]}
        if s.get("attachment"):
            slot["attachment"] = s["attachment"]
        if s.get("color"):
            slot["color"] = "#" + s["color"]
        if s.get("blend"):
            slot["blend"] = s["blend"]
        converted = []
        for name, a in attachments.get(s["name"], {}).items():
            kind = a.get("type", "region")
            if kind not in ("region", "mesh"):
                stats["dropped"].append(f"{kind} attachment {s['name']}/{name}")
                # The slot's keys may still name it: it just shows nothing.
                continue
            images.add(a.get("path", name))
            if kind == "mesh":
                before = stats["ccw_hulls"]
                converted.append(convert_mesh(name, a, s["bone"], bone_names, worlds, stats))
                if stats["ccw_hulls"] != before:
                    flips[(s["name"], name)] = a["hull"]
            else:
                region = {"type": "region", "name": name, "image": f"images/{a.get('path', name)}.png"}
                for key, field in (("x", "x"), ("y", "y"), ("rotation", "rotation"), ("scaleX", "scale_x"), ("scaleY", "scale_y")):
                    if key in a:
                        region[field] = a[key]
                region["width"], region["height"] = a["width"], a["height"]
                converted.append(region)
        slot["attachments"] = converted
        if slot.get("attachment") and not any(c["name"] == slot["attachment"] for c in converted):
            del slot["attachment"]
        slots.append(slot)

    physics = []
    for order, c in enumerate(src["constraints"]):
        if c["type"] != "physics":
            continue
        # Values the sample omits. Neither is in the published format page: they
        # are inferred from the file (no explicit 0.5 or 0.85 anywhere, and the
        # explicit values cluster around them) and confirmed by how it then moves.
        p = {"name": c["name"], "bone": c["bone"], "order": order, "inertia": OMITTED_INERTIA, "damping": OMITTED_DAMPING}
        for key, value in c.items():
            if key in PHYSICS_FIELDS:
                p[PHYSICS_FIELDS[key]] = value
            elif key == "mass":
                p["mass_inverse"] = num(1 / value)
            elif key not in ("type", "name", "bone"):
                stats["dropped"].append(f"physics field {key}")
        physics.append(p)

    ctx = {"kept": {s["name"]: {a["name"] for a in s["attachments"]} for s in slots}, "attachments": attachments, "flips": flips, "bone_names": bone_names, "worlds": worlds,
           "bones_of_slot": {s["name"]: s["bone"] for s in src["slots"]},
           "slot_colors": {s["name"]: s.get("color", "ffffffff") for s in src["slots"]},
           "ik_setup": {c["name"]: {"mix": c.get("mix", 1), "softness": c.get("softness", 0), "bendPositive": c.get("bendPositive", True),
                                    "compress": c.get("compress", False), "stretch": c.get("stretch", False)}
                        for c in src["constraints"] if c["type"] == "ik"},
           "slot_names": [s["name"] for s in src["slots"]],
           "transforms": transform_by_source_name}
    doc = {"format": "skel2d/1", "name": doc_name, "bones": bones, "slots": slots, "physics": physics, "transforms": transforms}
    if iks:
        doc["ik"] = iks
    doc["animations"] = {n: convert_animation(a, ctx, stats) for n, a in src["animations"].items()}
    if src.get("events"):
        doc["events"] = {}
        for name, e in src["events"].items():
            doc["events"][name] = {f: e[f] for f in ("int", "float", "string") if f in e}
            for f in e:
                if f in ("audio", "volume", "balance"):
                    stats["dropped"].append(f"event {f}")

    (out / "images").mkdir(parents=True, exist_ok=True)
    for image in sorted(images):
        shutil.copy(sample / "images" / f"{image}.png", out / "images" / f"{image}.png")
    shutil.copy(sample / "license.txt", out / "license.txt")
    (out / f"{doc_name}.skel.json").write_text(json.dumps(doc, ensure_ascii=False, indent=1), encoding="utf8")

    print(f"bones {len(bones)}, slots {len(slots)}, physics {len(physics)}, images {len(images)}, animations {len(doc['animations'])}")
    print("transform constraints:", [c["name"] for c in transforms])
    for key in ("bind_drift", "ccw_hulls", "channel_curves", "resampled_segments", "ik_keys", "events", "draw_order_keys"):
        print(key, stats[key])
    print("not representable:", sorted(set(stats["dropped"])))


if __name__ == "__main__":
    main(Path(sys.argv[1]), sys.argv[2], Path(sys.argv[3]), sys.argv[4])
