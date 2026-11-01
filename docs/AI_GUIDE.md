# Skel2D AI Guide

Terse reference for agents. All output is JSON; errors are JSON on stderr (CLI, exit 1) or HTTP 400 (API).

## Conventions
- **The root bone.** Every document has a bone named `root`: `skel2d new` starts with it, loading an older file without one adds it. `add_bone` / `add_bone_world` without `parent` put the new bone under root. `add_bone` named `root` on a document that already has one only overwrites its values. Root can be moved, rotated, scaled and keyed like any other bone. It cannot be removed (`remove_bone`), renamed, or given a parent, and no other bone can lose its parent (`set_bone` `parent:null` is refused). Index 0 of `info`/`pose` bone lists is therefore root; address bones by name.

- y-up, angles in degrees, counter-clockwise.
- Bone x/y/rotation/scale are relative to the parent. `length` is visual only (along local +x; does not affect children).
- Animation values are offsets from setup: `rotate` and `translate` are ADDED, `scale` is MULTIPLIED (1 = unchanged).
- Everything is addressed by name. Times in seconds, >= 0. Keys within 1e-4 s are the same key (replaced).
- Slots are in draw order: first = back. `add_slot` appends to the end (front) unless `index` is given; `move_slot` index 0 = back.
- Colors: hex string `"#rrggbb"` or `"#rrggbbaa"`.

## Weight operations (M3)

M3 weight editing uses the same `AnyOp` / `skel2d_rig::apply_any` path in the editor,
CLI `apply`, HTTP `POST /ops`, and MCP `skel2d_apply_ops`. Core weight ops need no image.

```json
[
  {"op":"bind_bones","slot":"skin","attachment":"arm","add":["root"],"redistribute":"auto"},
  {"op":"set_bone_weight","slot":"skin","attachment":"arm","vertices":[0,1],"bone":"lower","value":0.35,"mode":"set","locked_bones":["root"]},
  {"op":"smooth_weights","slot":"skin","attachment":"arm","factor":0.5,"iterations":2,"locked_bones":["root"]},
  {"op":"prune_weights","slot":"skin","attachment":"arm","max_bones":4,"threshold":0.01,"locked_bones":["root"]},
  {"op":"swap_weights","slot":"skin","attachment":"arm","a":"upper","b":"lower"},
  {"op":"normalize_weights","slot":"skin","attachment":"arm"},
  {"op":"bind_bones","slot":"skin","attachment":"arm","remove":["root"],"redistribute":"parent"}
]
```

- All six ops accept optional `skin`; omitted means the slot's own attachment.
- `smooth_weights`: omitted `vertices` means all; defaults `factor=0.5`, `iterations=1`.
  Factor must be 0..=1 and iterations 1..=100. Uses simultaneous edge-neighbor averaging per pass.
- `prune_weights`: omitted `vertices` means all; defaults `max_bones=4` (1..=8),
  `threshold=0.01` (0..=1). Responses include `pruned_influences` for the batch and mesh summaries.
  Locks take priority over the count and threshold. A partial locked budget retains at least
  one unlocked recipient, even if that exceeds the requested count.
- `swap_weights`: optional `vertices`; exchanges `a` and `b`. If either bone is locked, the swap does nothing.
- `set_bone_weight`: required `vertices`, `bone`, finite `value`, and `mode` (`set`, `add`, `scale`).
  Add permits negative values. Set/scale require nonnegative values. The result clamps to the
  budget left by locks; other unlocked influences share the remainder. If none exist,
  the slot bone (or its parent when editing the slot bone) receives it; without an eligible
  recipient the last influence keeps the free budget.
- `bind_bones`: optional `add`/`remove` arrays and required `redistribute` (`auto`, `nearest`, `parent`).
  Zero-influence additions persist in `Mesh.bones`. Empty/missing `Mesh.bones` preserves legacy inference
  without rewriting old files. The effective palette combines stored names and effective influences.
  Removing the last bound bone fails atomically. Auto rescales surviving influences proportionally;
  without survivors it uses the nearest remaining bone segment in setup space. Nearest moves each
  removed share to that segment. Parent prefers the nearest bound ancestor of the removed bone,
  falling back to nearest. Names must exist; duplicate names and add/remove overlap are errors.
  Bone renames/deletions update palettes in both default and named skins.
- `normalize_weights`: normalizes every vertex. Evaluation still normalizes legacy stored values.
  Weight operations store four decimal places and correct rounding residuals so the sum is 1.
  An empty influence list means full weight on the slot bone.
- `locked_bones` is a session/request option, never document state. Smooth, prune, numeric, swap,
  distance `auto_weights`, `image_weights`, transfer, mirror, brush and paste preserve locked values.
  Explicit binding removal and explicit normalization intentionally edit the whole target weight set.
  Automatic weights default to an explicit binding palette when one exists, otherwise retain legacy candidate selection.
- For ops with a `vertices` array, omission means all where supported, `[]` changes none,
  and out-of-range or duplicate indices are errors. Unselected vertices and other skins stay unchanged.
  `mirror_weights` operates across its axis on the target half; it has no vertex-selection parameter.
- The editor exposes palette/numeric controls and preview dialogs for pruning, mirroring, transferring,
  and binding removal. Enter applies one Undo step; Escape restores the original. Pose test
  (`G`, enable Pose test, Alt+drag) rotates only a temporary evaluation track and restores setup on release.
  Hold T to paint in the configured test angle; releasing T restores setup without editing bones or keys.
  Painting previews continuously and records one `set_vertices` op when the stroke ends.

## Document (`skel2d schema document` for the full schema)
```
{"format":"skel2d/1","name":"hero",
 "bones":[{"name","parent"?,"x"?,"y"?,"rotation"?,"scale_x"?,"scale_y"?,"length"?}],   // parents before children
 "slots":[{"name","bone","color"?,"attachment"?,"attachments":[
    {"type":"region","name","image"?,"x"?,"y"?,"rotation"?,"scale_x"?,"scale_y"?,"width","height"} |
    {"type":"mesh","name","image"?,"vertices":[{"x","y","u","v","weights"?:[{"bone","weight"}]}],"triangles":[[i,j,k]]}]}],
 "ik":[{"name","bones":[parent?,child],"target","mix"?:1,"softness"?:0,"bend_positive"?:true,"compress"?:false,"stretch"?:false}],   // solved in this order
 "events":{"footstep":{"int"?:0,"float"?:0,"string"?:""}},   // event definitions: default values of their keys
 "physics":[{"name","bone","order"?,"x"?,"y"?,"rotate"?,"scale_x"?,"shear_x"?,"inertia"?:1,"strength"?:100,"damping"?:1,"mass_inverse"?:1,"wind"?,"gravity"?,"mix"?:1,"limit"?:5000,"step"?:1/60,"inertia_global"?,...}],   // shares order with ik
 "physics_globals":{"inertia"?,"strength"?,"damping"?,"mass_inverse"?,"wind"?,"gravity"?,"mix"?},
 "animations":{"walk":{"duration"?,
    "bones":{"bone":{"rotate":[{"time","value","curve"?}],"translate":[... value:[x,y]],"scale":[... value:[x,y]]}},
    "slots":{"slot":{"attachment":[{"time","name":str|null,"reset"?:true}],"color":[{"time","value":"#hex","curve"?}]}},
    "ik":{"constraint":[{"time","value":mix | {"mix"?,"softness"?,"bend_positive"?,"compress"?,"stretch"?},"curve"?,"curves"?:[mix curve,softness curve]}]},
    "events":[{"time","name","int"?,"float"?,"string"?}],   // sorted by time
    "draw_order":[{"time","order"?:[every slot name, back to front]}],   // no order = setup order
    "physics":{"constraint or \"\"":{"inertia"|"strength"|"damping"|"mass"|"wind"|"gravity"|"mix":[{"time","value","curve"?}],"reset":[time,...]}}}}}
```
Defaults are omitted on save. See Meshes and IK below. Region `image` is a path relative to the document file; without it a placeholder rectangle is drawn (unloadable images are listed in `missing_images` by render). Animation duration = `duration` if set, else time of last key. Region width/height must be > 0.

## Ops
Single op object or array; tag field is `"op"`. Unknown fields are rejected.
```
{"op":"set_name","name":"hero"}
{"op":"add_bone","name":"arm","parent":"root","x":20,"y":0,"rotation":0,"scale_x":1,"scale_y":1,"length":40}   // parent omitted = under root
{"op":"set_bone","name":"arm","rename":"arm_l","parent":null,"x":1,"y":2,"rotation":10,"scale_x":1,"scale_y":1,"length":30}   // omitted = unchanged; parent: a bone name (null is refused: every bone except root has a parent; root cannot be renamed or moved)
{"op":"remove_bone","name":"arm","recursive":true}      // also removes its slots + timelines; children are an error unless recursive
{"op":"add_slot","name":"s","bone":"arm","color":"#ffffff","attachment":"img","attachments":[{"type":"region","name":"img","width":32,"height":16}],"index":0}
{"op":"set_slot","name":"s","rename":"s2","bone":"arm","color":"#ff0000","attachment":null}   // attachment null = hide in setup pose
{"op":"remove_slot","name":"s"}
{"op":"move_slot","name":"s","index":0}
{"op":"set_attachment","slot":"s","attachment":{"type":"region","name":"img","image":"img.png","x":0,"y":0,"rotation":0,"scale_x":1,"scale_y":1,"width":32,"height":16}}   // add or replace by name
{"op":"remove_attachment","slot":"s","name":"img"}   // if linked children exist: on_parent_removed="freeze" or "delete"; omitted = error
{"op":"add_linked_mesh","slot":"body","name":"shirt_link","parent":"body_mesh","skin":"shirt","image":"shirt.png","timelines":true}   // parent_skin optional (default skin when omitted); geometry, UVs and weights follow the independent parent mesh; timelines defaults true
{"op":"add_skin","name":"red"}   // skins (outfits): per-slot attachments keyed by placeholder name
{"op":"set_skin_attachment","skin":"red","slot":"torso","attachment":{"type":"region","name":"shirt","image":"shirt_red.png","width":40,"height":50}}   // where the slot (or a key) shows "shirt", the active skin's one is drawn
{"op":"remove_skin_attachment","skin":"red","slot":"torso","name":"shirt"}   // parent removal policy also accepts on_parent_removed="freeze"|"delete"
{"op":"set_skin","name":"red","rename":"hero","bones":["cape"],"constraints":["cape_ik"]}   // skin-required bones and skin-owned constraints; old document field "ik" migrates on load
{"op":"remove_skin","name":"red"}   // linked children referencing parents in this skin need on_parent_removed="freeze"|"delete"
// mesh ops (set_vertices, auto_weights, transfer_weights, mirror_weights) take "skin":"red" to edit a mesh inside a skin; render with --skin red [--skin hair] (later wins), MCP/HTTP "skins"
// linked meshes cannot be edited geometrically; edit the parent. A linked mesh with timelines=true reads its parent's deform keys; key_deform/remove_key on the child target that parent path. With false it keeps its own keys. A linked parent cannot itself be linked.
// `pose --skin red` and MCP skel2d_pose / HTTP GET /pose with skins return source_skin and resolved_attachment per slot; info includes resolved_skins for each named skin.
{"op":"add_animation","name":"walk","duration":1.0}     // duration optional; errors if it exists; keying a missing animation also creates it
{"op":"set_animation","name":"walk","rename":"run","duration":null}   // duration null = back to "last key time"
{"op":"remove_animation","name":"walk"}
{"op":"key_bone","animation":"walk","bone":"arm","time":0.5,"rotate":30,"translate":[0,5],"scale":[1,1.2],"curve":"ease_out"}   // >=1 of rotate/translate/scale; curve applies to every key set by this op
{"op":"key_slot","animation":"walk","slot":"s","time":0.5,"attachment":"img2","color":"#ffffff80","curve":"linear"}   // >=1 of attachment/reset/color; attachment null = hide; "reset":true (instead of attachment) = show what lower tracks (or the setup pose) show
{"op":"key_deform","animation":"walk","slot":"s","attachment":"img","time":0.5,"offsets":[[0,0],[3,-2]],"curve":"ease_out"}   // mesh only; one [dx,dy] per vertex (exact count), setup-pose space, added before skinning
{"op":"remove_key","animation":"walk","bone":"arm","timeline":"rotate","time":0.5}   // timeline: rotate|translate|scale (bone), attachment|color|deform (slot); bone XOR slot; "attachment":"img" limits deform removal to one mesh
{"op":"clear_timeline","animation":"walk","bone":"arm","timeline":"rotate"}          // timeline omitted = all timelines of that bone/slot
{"op":"add_ik","name":"reach","bones":["upper","lower"],"target":"goal","mix":1,"softness":0,"bend_positive":true,"compress":false,"stretch":false}   // all but name/bones/target optional
{"op":"set_ik","name":"reach","rename":"r2","bones":["upper","lower"],"target":"goal2","mix":0.5,"softness":10,"bend_positive":false,"compress":false,"stretch":true}   // omitted = unchanged
{"op":"remove_ik","name":"reach"}
{"op":"key_ik","animation":"wave","ik":"reach","time":0.5,"mix":0.8,"softness":12,"bend_positive":false,"compress":false,"stretch":true,"curve":"linear","curves":["linear",{"bezier":[0.3,0,0.7,1]}]}   // >=1 of mix/softness/bend_positive/compress/stretch; fields not given keep what a key at that time holds, else follow setup; curve applies to both channels, curves = [mix, softness] replaces it
{"op":"remove_ik_key","animation":"wave","ik":"reach","time":0.5}
{"op":"key_draw_order","animation":"hit","time":0.5,"order":["s_back","s_front","s_mid"]}   // every slot, back to front; order omitted/null = setup order; replaces a key at that time
{"op":"remove_draw_order_key","animation":"hit","time":0.5}
{"op":"add_event","name":"footstep","int":0,"float":0,"string":"left"}   // defaults of the event's keys; all optional
{"op":"set_event","name":"footstep","rename":"step","int":1,"float":0.5,"string":"x"}   // omitted = unchanged
{"op":"remove_event","name":"footstep"}   // also all its keys in every animation
{"op":"key_event","animation":"walk","name":"footstep","time":0.5,"int":1,"float":0.5,"string":"x"}   // int/float/string optional (omitted = the event's defaults); replaces that event's key at that time
{"op":"remove_event_key","animation":"walk","name":"footstep","time":0.5}
{"op":"add_physics","name":"hair_1_physics","bone":"hair_1","rotate":1,"damping":0.9,"order":1}   // every PhysicsConstraint field; defaults follow the 4.2 JSON convention (x,y,rotate,scale_x,shear_x 0 = that property is not simulated)
{"op":"set_physics","name":"hair_1_physics","rename":"h1","bone":"hair_1","strength":60,"mass_inverse":0.5,"step":0.01667,"damping_global":true}   // omitted = unchanged; order too
{"op":"remove_physics","name":"hair_1_physics"}   // also its keys and skin references
{"op":"key_physics","animation":"idle","constraint":"hair_1_physics","property":"damping","time":0.5,"value":0.8,"curve":"linear"}   // property: inertia|strength|damping|mass|wind|gravity|mix; constraint "" = the global timeline
{"op":"key_physics_reset","animation":"idle","constraint":"hair_1_physics","time":1.0}   // restart the simulation there; constraint omitted = every constraint
{"op":"remove_physics_key","animation":"idle","constraint":"hair_1_physics","property":"damping","time":0.5}   // property omitted = remove a reset key
{"op":"set_physics_globals","wind":10,"gravity":20}   // shared values read by constraints with a *_global flag
{"op":"mesh_from_region","slot":"skin","region":"arm","name":"arm_mesh","cols":8,"rows":2}   // name defaults to region name (replaces it); cols/rows default 4; vertices weighted to slot's bone
{"op":"auto_weights","slot":"skin","attachment":"arm","bones":["upper","lower"],"max_influences":4,"falloff":2}   // bones defaults to all; max_influences default 4; falloff default 2; "vertices":[0,3] = only those
{"op":"transfer_weights","slot":"shirt","attachment":"shirt","from_slot":"body","from_attachment":"body"}   // each vertex takes the closest point's weights on the source mesh (setup pose); optional "vertices"
{"op":"mirror_weights","slot":"body","attachment":"body","axis":0,"source":"positive"}   // copies the x>axis half (or "negative") onto the other half, swapping _l/_r, left/right, L_/R_ bones
{"op":"set_vertices","slot":"skin","attachment":"arm","vertices":[{"index":3,"x":40,"y":12,"u":0.4,"v":0.1,"weights":[{"bone":"upper","weight":1}]}]}   // per vertex all but index optional; weights [] = slot's bone
```
Null semantics: `parent` (set_bone), `attachment` (set_slot, key_slot) and `duration` (set_animation) distinguish absent (unchanged) from explicit `null` (hide / auto duration; `parent:null` is refused because every bone but root has a parent). In `add_bone`/`add_slot`/`add_animation`, omit instead of null.

## Curves
`curve` on a key shapes interpolation toward the NEXT key: `"linear"` (default), `"stepped"` (hold), `"ease_in"`, `"ease_out"`, `"ease_in_out"`, `{"bezier":[x1,y1,x2,y2]}` (CSS cubic-bezier; x1,x2 clamped to 0..1). Attachment keys have no curve.

## Meshes
Meshes optionally store `hull` (the first N vertices, clockwise; default 0 infers triangle boundaries) and `edges` (user constrained vertex pairs, default empty). Loading an old file never reorders vertices. Topology ops normalize a single outer boundary to the front, remapping triangles, constraints and all deform keys. Hole boundaries are preserved in triangles. Legacy meshes with disconnected islands retain `hull:0`, since a single boundary cannot describe them; retriangulation preserves every island and gap.

```json
{"op":"add_mesh_vertices","slot":"body","attachment":"cloth","points":[[30,20]],"on_edge_tolerance":2}
{"op":"remove_mesh_vertices","slot":"body","attachment":"cloth","indices":[4]}
{"op":"set_mesh_edges","slot":"body","attachment":"cloth","edges":[[0,2]]}
{"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","count":8}
{"op":"generate_mesh_vertices","slot":"body","attachment":"cloth","spacing":12}
{"op":"reset_mesh","slot":"body","attachment":"cloth","mode":"vertices_to_uvs"}
{"op":"reset_mesh","slot":"body","attachment":"cloth","mode":"uvs_to_vertices"}
```

These five op types accept optional `skin` (omission means the default mesh). Indices refer to the document immediately before that op: read it again after a topology change. `on_edge_tolerance` is a nonnegative world-space distance (default 0). Inserted vertices interpolate UVs, bone weights and each deform key using the same factors. Removed vertices drop their offsets and incident constraints; surviving indices are remapped. Collinear vertices split constrained edges into segments. Crossing constraints, points outside the mesh and boundary collapse fail atomically. Constrained Delaunay triangulation clips concave outlines and holes.

Generation accepts either `count` (1..2000; default 8) or positive `spacing` (at most 2000 grid candidates). Count mode repeatedly inserts the largest triangle's centroid. Spacing mode places interior grid points while retaining existing vertices. `vertices_to_uvs` restores vertex positions using an affine UV-to-setup fit; `uvs_to_vertices` fits UVs to current positions. Both are pure geometry, retain affine placement, weights and deform offsets, and reject a degenerate fit. They do not read image dimensions.

The editor's mesh step (step 3) has the modes **Move / Add / Delete / Fold line** (internally Modify / Create / Delete / Edges), cycled with a viewport right-click. Create clicks insert vertices; Delete clicks remove them. Ctrl+click, double-click and Delete retain removal shortcuts. In Edges mode, drag vertex to vertex to add a constraint, or select an orange constraint and press Delete. Cyan lines show boundaries; gray lines show ordinary triangle edges. Generate, delete, reset and constraint actions use the same core ops as CLI/HTTP/MCP and are available through Ctrl+K. Topology controls are disabled in the animation step (5); vertex drags there still key deformation.

Deformation is stored at `animations[animation].deform[skin][slot][attachment]`; `default` means the slot's own attachments and is reserved (not a user skin name). Existing `slots[slot].deform` files migrate to `deform.default` on load. Active skins use only the resolved attachment owner's keys; fallback to a default attachment uses default keys. Offset counts must match that specific mesh, even when two skins share a placeholder.

`key_deform`, deform `remove_key`, and `clear_timeline` accept optional `skin`; omission targets default deformation. Other slot keys remain shared. Clearing a slot without `timeline` clears attachment/color and the selected skin's deform keys. Use `timeline:"deform"` to clear deformation alone.

```json
{"op":"rename_attachment","slot":"body","from":"cloth","to":"cloth_new","skin":"armor"}
{"op":"rename_placeholder","slot":"body","from":"cloth","to":"cloth_new"}
```

`rename_attachment` changes only the selected skin's attachment and deform path (omit `skin` for default). `rename_placeholder` changes every matching attachment and deform path across skins. Both update the shared slot setup attachment and animation attachment keys; a local rename leaves other skins' old placeholders intact. Name collisions fail atomically. Replacing a mesh fits only its owner's deform keys. Removing/renaming skins, slots or attachments cleans up the corresponding paths; a removed placeholder's shared references survive while another skin still supplies it. These ops use the same CLI `apply`, HTTP `POST /ops`, and MCP `skel2d_apply_ops` API.

Editor: select an attachment row and press F2, or use the image card's **More > Rename** menu in step 2. The dialog's **Rename matching placeholders in all skins** checkbox chooses `rename_placeholder`; otherwise it emits `rename_attachment`. Ctrl+K **Rename** uses the selected row. Deform drags, dope-sheet retiming, copy/paste, curves and deletion preserve the owning skin.

A `mesh` attachment is a triangulated image deformed by bone weights (linear blend skinning).
- Vertex `x`/`y` are in setup-pose skeleton (world) space: where the vertex sits when every bone is in its setup pose. Overlay a mesh on a setup-pose render directly.
- `u`/`v` are 0..1 with (0,0) at the image's top-left. `triangles` are vertex index triples. Without `image` the triangles draw as a placeholder.
- `weights` are normalized on evaluation; empty = the slot's bone at 100%. Bones are named, missing bones are ignored.
- Bound to the setup pose: changing a bone's setup transform (`set_bone`) does NOT move the mesh; only animation and IK deform it. Re-bind by editing vertices (`set_vertices`) or `auto_weights`.
- Typical flow: `add_slot` with a region -> `mesh_from_region` -> `auto_weights` (-> `set_vertices` to refine). For PNG silhouettes use `trace_mesh` followed by `image_weights`.

### Image-dependent ops (M2)

CLI `apply`, HTTP `POST /ops`, MCP `skel2d_apply_ops`, and the editor all use `skel2d_rig::apply_any`. One JSON array can mix core and asset ops. Asset expansion reads the current staged document in input order; the complete batch validates atomically before saving. `schema ops` includes both tag namespaces. The core never reads files.

```json
[
  {"op":"trace_mesh","slot":"skin","attachment":"arm","alpha_threshold":0.05,"padding":2,"detail":0.5,"interior":"auto","keep_weights":true},
  {"op":"image_weights","slot":"skin","attachment":"arm","bones":["upper","lower"],"max_influences":4,"falloff":3}
]
```

- Both accept optional `skin`; omitted means the default attachment owner, without preview-skin fallback. Images resolve relative to the document's directory.
- `trace_mesh` replaces a region or retraces a mesh. Optional `name` renames that owner's attachment and its references. Defaults: `alpha_threshold=0.05`, `padding=2`, `detail=0.5`, `interior="auto"`, `keep_weights=true`. Alpha must exceed the threshold. Alpha/detail are in `[0,1]`; padding is `[0,64]` source pixels, rounded down to whole pixels with a square dilation and clipped to the image. Optional `concavity` is outline simplification tolerance in pixels `[0,64]`; omitted derives it from detail. Optional `spacing>=2` controls interior spacing in source pixels; omitted derives it from image size and detail. `interior` accepts `none` (outline only), `grid` (regular points inside the outline), or `auto` (adaptive refinement and bone joint hints). Higher detail adds resolution. Density presets map to detail 0/0.5/1 with automatic spacing. Traces reject empty silhouettes and singular placements. Holes and disconnected parts remain empty; they use `hull=0` when a single hull cannot describe them.
- Retracing transfers painted weights when `keep_weights=true`, and resamples all deform keys of the exact owner with their original curves. Constrained edge endpoints project to the new surface and remain constrained; collapsed or crossing constraints reject the batch. `keep_weights=false` restores implicit slot-bone binding. This preservation and live preview are intentional.
- `image_weights` accepts optional `bones` and `vertices`. Defaults: `max_influences=4` (1–8), `falloff=3` (0.1–16). Unselected vertices, topology and deform keys stay unchanged. Candidate bones must exist and be unique; by default IK targets are skipped and bones crossing visible pixels are preferred. Distances follow visible pixels instead of crossing empty gaps. Disconnected pixels retain the existing geodesic fallback behavior.
- Apply responses include `applied` (input count), `expanded` (core op count), and `meshes`: per-asset-op summaries with `slot`, `attachment`, optional `skin`, `vertices`, `triangles`, `hull`, and sorted `weight_bones` (including implicit slot-bone binding). `dry_run` computes the same report without saving.
- Rust callers can use `expand(doc, base_dir, &[AssetOp]) -> Result<Vec<Op>>`. The editor retains these materialized ops and snapshots for Undo/Redo, so history never rereads changed image files. Trace dialog parameter changes preview immediately; Cancel restores the original document without adding history.

Reproducible arm fixture: copy `examples/arm/arm.skel.json` into the same directory under a new filename, then apply `examples/arm/trace.ops.json` to that copy. `trace.png` has a faint alpha border for comparing thresholds 0.05 and 0.5. The batch replaces the placeholder mesh with an image region, traces it, and weights it to upper/lower. The original example stays unchanged.

## IK
Bone transforms support setup `shear_x`/`shear_y` in degrees (default 0) and `inherit` (default `normal`): `normal`, `only_translation`, `no_rotation_or_reflection`, `no_scale`, `no_scale_or_reflection`. Use `add_bone` / `set_bone` for these fields. Parent position always affects child position; inheritance controls the linear axes. Root bones ignore inheritance. Definitions follow the 4.2 axis angles; unchanged default transforms retain the old TRS calculation.
- `key_bone {animation,bone,time,shear:[x,y],curve?}` adds angular offsets to setup shear. Duration, key removal/clearing (`timeline:"shear"`), copying, retiming, layering, name changes and deletion include shear. Inheritance is a setup setting; its stepped animation timeline is reserved for P6.
- `pose` / HTTP / MCP bone reports include evaluated local `shear_x`, `shear_y`, `inherit`, and the full `world` affine matrix `{a,b,c,d,tx,ty}` so Y-axis shear is not lost in rotation/scale summaries. Mesh skinning, regions, and render use the same world calculation. Fields are backwards compatible; no file migration is required.
- Editor: setup Shear X/Y, an Inheritance group with descriptions, and animated Shear X/Y with a key button. Scale tool + Alt+drag shears a bone (X handle = shear X, Y/free = shear Y), using the ordinary setup/key and auto-key-off paths. F1 documents this shortcut; Ctrl+K offers reset shear and setup inheritance modes. Reparenting preserves the complete sheared world matrix; singular parents report an error instead of discarding shear.

`duplicate_animation {name,new_name}` copies an entire animation (all channels, keys, curves and duration). The source stays independent; missing sources, duplicate destinations and invalid names fail atomically. This op is available through CLI `apply`, HTTP and MCP `skel2d_apply_ops` and generated op schemas.

`ik` constraints are solved after bone animation by ascending `order` (i32, default 0), preserving document array order on ties (inside pose evaluation, so `pose` and `render` show the result).
- `add_ik` and `set_ik` accept `order`. `set_constraint_order {names:[...]}` lists every constraint exactly once and assigns priorities 0,1,2,...; unknown, repeated or missing names fail atomically. Available through CLI `apply`, HTTP and MCP `skel2d_apply_ops`, with generated schemas.
- PSD origin: a layer with `[origin]` in its name marks the origin with its center (hidden or not; never a part), else the first vertical and horizontal guides, else the canvas center. The editor puts a marked origin on the target bone, so root lands on it. `skel2d psd` reports it as `origin` and `origin_source`; `layers.json` keeps canvas positions. Photoshop's ruler origin is not read.
- `reseat_bone {name,x,y}` moves a bone's setup position (in parent space) while everything on it stays put: direct children and regions, including skin regions, are compensated and meshes keep their setup-space vertices. Use it to fix a joint or re-pivot `root` after import. The editor does this on Alt+drag in the setup pose. A bone with zero scale is refused.
- `redraw_bone {name,start:[x,y],end:[x,y]}` draws a bone again in world space: position, rotation and length change, its scale is kept, and direct children, regions (skin regions too) and meshes stay put. A region has no shear, so under a non-uniform bone scale it keeps position and size but not skew. The editor does this on Alt+drag with the New bone tool.
- `set_slot {name,clip}` shows a slot only inside another slot's rendered image, like a Photoshop clipping layer: `clip` names the base slot, `null` removes it (`add_slot` takes it too). The two slots keep moving on their own, and the clipped slot shows nothing where the base shows nothing. One level only: a base cannot be clipped itself. Renaming the base follows through and removing it clears the clip. The editor's PSD import can keep clipped layers this way (Each layer, keep clipping).
- 1 bone `[b]`: rotates `b` so its +x axis aims at the target's world origin.
- 2 bones `[parent, child]`: child must be a direct child of parent; parent+child rotate so the child's tip (`length` along +x) reaches the target (clamped to reach). `bend_positive` true = counter-clockwise elbow, false = clockwise.
- `softness` (>= 0, default 0): the world distance before full reach at which the chain starts to ease, so the limb does not snap straight. The easing covers target distances reach-softness..reach+softness. Two-bone chains only.
- `compress` (one bone only): shortens the bone along x when the target is closer than its length. `stretch` (one or two bones): lengthens the bones along x when the target is out of reach.
- `mix` 0..1 blends from the animated pose (0) to the solved pose (1).
- `animations.X.ik.{name}` is a list of `{time,value,curve?,curves?}`. `value` is a plain number (mix only) or an object with optional `mix, softness, bend_positive, compress, stretch`. An omitted field follows the setup value (not the previous key). `mix` and `softness` interpolate (curve channels 0 and 1; `curves:[mix curve, softness curve]` gives each its own); the three switches hold until the next key. Layered tracks: a switch takes the top track's value once its alpha >= 0.5.
- `target` must not be inside the chain. Animate the target bone (`key_bone`) to move the limb. Example: `examples/arm` (mesh arm, 2-bone IK, `wave` animation).
- `set_ik` validates bones/target. `remove_bone` also removes constraints that use the bone (and their keys).

## Draw order
`animations.X.draw_order` is a list of `{time, order?}` keys: from `time` until the next key the slots draw in `order` (back to front, every slot exactly once); a key without `order` returns the setup order. Before the first key the setup order shows. A slot added after the key was made sits directly above its setup neighbour; renaming and removing slots is tracked in keys. Layered tracks: the top track with alpha >= 0.5 and draw order keys wins. `pose` reports `draw_order` (slot names back to front) only when it differs from the setup order. `render` draws it. Ops: `key_draw_order`, `remove_draw_order_key`.

## Events
Top-level `events` defines names with default `int`, `float`, `string`; `animations.X.events` lists `{time,name,int?,float?,string?}` keys sorted by time (omitted values use the definition's). They fire when playback passes a key. `pose --anim A --time T --since S` (HTTP `since=S`, MCP `since`) reports `events` fired in (S, T] with resolved values; without `since` it reports everything from the start up to T. A game loop passes the previous clock. `info` lists `events` (definitions) and per animation `event_keys` and `draw_order_keys`. `render` does not draw events. Ops: `add_event`, `set_event`, `remove_event` (also removes its keys), `key_event`, `remove_event_key`.

Editor: the dope sheet has DRAW ORDER and EVENTS rows, and step 5 has groups for them.

## Physics
Physics constraints (4.2 style): a bone lags behind its animated pose on a damped spring. They are evaluated in the same order as IK (`order`, ties keep array order); constraint names are unique across both kinds. `set_constraint_order {names}` lists every IK and physics constraint exactly once.
- Influence `x`, `y`, `rotate`, `scale_x`, `shear_x` (0..1, default 0): how much the simulation moves that property. A constraint with all five at 0 does nothing.
- Values: `inertia` (1), `strength` (100), `damping` (1; velocity kept per 1/60 s, so keep it below 1 to settle), `mass_inverse` (1; heavier = smaller), `wind` (world +x), `gravity` (world -y, positive sags), `mix` (1), `limit` (5000, speed limit), `step` (seconds, default 1/60; the editor shows it as Hz).
- `*_global` flags (`inertia_global`, `strength_global`, `damping_global`, `mass_global`, `wind_global`, `gravity_global`, `mix_global`) make the constraint read the value from `physics_globals` (`set_physics_globals`) instead of its own.
- Animation timelines: per constraint one key list per property plus `reset` (a plain list of times where the simulation restarts from the pose). **Mass keys hold `mass_inverse`**, not mass.
- Timeline name `""` is the global timeline: its `reset` times restart every constraint; its property keys only affect constraints whose matching `*_global` flag is on. A constraint's own timeline always applies on top.
- Skin-owned physics: `set_skin {constraints:[names]}` lists IK and physics constraints together; a listed constraint only runs while that skin is active.
- Rename/removal of bones and constraints keeps references (timelines, skins) in sync; `remove_bone` removes physics constraints on the bone.
- Presets (hair, cloth, tail, chest, accessory) are an editor convenience only: a bundle of values written as one `set_physics`.

Simulation in `pose` and `render`: with an animation (or `--track`/`layers`) physics is simulated from time 0 and is deterministic (same input = identical bits; `render --frames` is one continuous run). Without an animation nothing moves. Options (same names in the CLI, HTTP query and MCP args):
- `--physics none|simulate` (HTTP/MCP `physics`): `simulate` is the default; `none` shows the plain animated pose.
- `--fps N` (`fps`, 1..1000, default 60): simulation frames per second.
- `--move "x,y@t;x,y@t"` (`move`): world path of the whole skeleton (times increasing, linear between points) that the physics reacts to, e.g. `--move "0,0@0;100,0@1"`.

`skel2d_pose` / `pose` / `GET /pose` add `"physics":[{"name","x","y","rotate","scale","vx","vy","vrotate","vscale"}]` per constraint (omitted without physics): the current offset (world units; rotation in degrees; `scale` = scale change along the bone) and its velocity (`vrotate` in degrees/s). Use it as numbers instead of looking at images.

Tuning procedure:
1. `add_physics` with `rotate` 1 and `damping` 0.9; `pose --anim A --time T` at a few times after a fast movement (`--move` or an animated parent) and read `physics[].rotate`/`x`/`y`.
2. Too stiff (offsets about 0): lower `strength`, raise `inertia`, lower `mass_inverse` (heavier). Never settles or jitters: lower `damping` (toward 0.8) or raise `strength`. Sags: set `gravity`.
3. Apply the change with `set_physics` (one op, any number of fields) and `pose` again; `render --frames 8` to check the look.
4. Key `mix`/`wind`/etc. with `key_physics`, restart the simulation after a cut with `key_physics_reset`.

Other behaviors worth knowing:
- A linked mesh with `timelines`: true ignores its own deform keys (it reads the parent's); creating or switching it to true drops keys it had.
- `rename_attachment` keeps the shared slot setup and attachment keys while another owner (skin) still fills the old name; use `rename_placeholder` to rename everywhere.

Editor: step 1 lists IK and physics together in evaluation order under "Constraint order" (drag to reorder, F2 rename, Del delete). The bone panel's "Jiggle (physics)" group prefills the selected bone; with a bone chain selected "Create one per bone" makes one constraint per bone in one undo step. The inspector has influence sliders, presets, Motion/Environment/Advanced groups with a "G" toggle for shared values, a Shake button and, in animate mode, key buttons (values key instead of changing the setup) plus Key reset; the dope sheet shows PHYSICS rows ("All physics" for the `""` timeline). Over the viewport: Pause, Reset (Shift+R), Off and, in steps 1-4, Physics preview. None of these change the document; each edit is one of the ops above (`skel2d-editor FILE --select-physics NAME` selects a constraint).

## Layered characters (`skel2d rig layers.json --spec SPEC`, `skel2d schema layers`)
One PNG per part instead of one image. Manifest (layers back to front, canvas pixels, y down):
```
{"canvas":[w,h]?,"layers":[{"name","image","x"?,"y"?,"center"?:[cx,cy],"scale"?:1,"attachment"?,"bone"?,"bones"?:[..],"mesh"?:bool,"hidden"?:bool,
   "variants"?:[{"name","image","center"?|"x"+"y"?,"scale"?}]}]}
```
- `image` paths are relative to the manifest; stored in the document relative to the output file. `x`,`y` = top-left (default 0,0); `center` = where the scaled image's middle lands (overrides x/y); `scale` multiplies image px to canvas px (default 1; variants inherit it).
- Each layer is a slot named `name` whose default attachment is `attachment` (default `name`); `hidden` hides it in the setup pose. Variants are extra attachments of that slot; without position they are centered on the layer's image.
- Mesh vs region: `mesh` wins if set. Otherwise a layer with `bone` is a rigid region on that bone; a layer without `bone` becomes a weighted mesh if it covers >= 1.5% of all visible pixels (weights only through its own pixels, limited to `bones` if given), else a rigid region on the nearest bone. Put small face parts on `head` with `bone`.
- The spec (humanoid/chain/custom) builds the skeleton from the composite silhouette exactly as for a single PNG; coordinates are canvas pixels.

`skel2d slice SHEET.png -o DIR [--prefix part] [--merge 40] [--padding 4] [--min-pixels 50]` cuts a parts sheet into `DIR/PREFIX_NN.png` (reading order, 1-based) and prints `{"pieces":[{"file","bbox","size"}]}`. Pieces closer than `merge` px stay one part (eye pairs); smaller than `min_pixels` is ignored; refuses to overwrite.
`skel2d psd FILE.psd -o DIR [--exclude NAME]... [--layers | --inner-groups] [--skip-hidden]` splits a PSD: one cropped PNG per top-level group (layers composited back to front, clipped layers painted onto their base, hidden layers included unless `--skip-hidden`) or ungrouped layer (with `--layers`: one per layer, groups only organize them), plus `DIR/layers.json` (back to front, `x`/`y`, ready for `bone`/`bones`/`mesh`) and `DIR/composite.png` (place joints on it with `image-info --grid`). Prints `{"canvas","manifest","composite","parts":[{"name","psd_name","file","x","y","size","psd_layers"}]}`; refuses to overwrite. `--exclude` drops a layer or group (e.g. a flattened reference). A paper background layer (`용지`/`Paper`/`用紙`) is always skipped.
AI-generated sheets: request a flat #00FF00 background, then `--key 00ff00` (chroma key, despill) and `--defringe 32` (alpha < 32 -> transparent, specks < min_pixels erased); `skel2d key IMG -o OUT [--color 00ff00] [--tolerance 60]` does the key alone. When pieces are too close to slice (eye pairs wider apart than the gap between items), cut by hand: `skel2d crop SHEET --rects rects.json -o DIR [--key ...] [--defringe N]`, rects = `[{"name","rect":[x0,y0,x1,y1]}]`. MCP `skel2d_slice` takes `key`, `key_tolerance`, `defringe`.

## Face poses and layering
- A face pose is an ordinary animation keyed at time 0, e.g. `{"op":"key_slot","animation":"face/happy","slot":"eyes","time":0,"attachment":"eyes_happy"}` (one per slot; `color` and `"attachment":null` work too). Play it as a layered track: CLI `--track face/happy --track blink@2.95`, editor `--tracks face/happy,blink`. `reset:true` keys in `blink`/`talk` show what lower tracks (or the setup pose) show, so they return to the active face pose.
- Layering: the main animation, then extra tracks in order. A track only affects timelines it keys (a `blink` track that keys only the eye slot leaves the rest alone) and replaces what earlier tracks set, blended by alpha; attachments switch when alpha >= 0.5; `reset` shows what lower tracks (or setup) show; tracks apply in order, first = lowest. CLI `--track NAME` follows the frame time looping over its own duration; `--track NAME@T` is fixed at T seconds.
- Ordered preview configuration: `[{"anim":"face/happy","alpha":1,"enabled":true},{"anim":"blink","alpha":0.5}]`. `alpha` defaults to 1 (finite, 0..1), `enabled` defaults to true. Animation names must exist and occur once, including disabled entries. Disabled/zero-alpha tracks are skipped. Each enabled track loops independently; zero-duration poses sample time 0. A preview entry matching the main animation is dormant, avoiding double application until another main is selected. Configurations are runtime parameters, not saved document fields or ops.
- Use CLI render/pose `--layers JSON`, MCP `skel2d_render {layers:[...]}` / `skel2d_pose {layers:[...]}`, HTTP `/render?layers=URL_ENCODED_JSON` / `/pose?layers=URL_ENCODED_JSON`, or editor `--layers JSON`. CLI/HTTP/MCP render apply structured layers after legacy `--track`/`tracks` entries; editor uses either `--tracks A,B` or `--layers JSON`. Schema: CLI `schema tracks`, MCP `skel2d_schema {kind:"tracks"}`, HTTP `/schema/tracks`. This schema is distinct from `schema layers` (the rigging image manifest).
- Face example (`examples/face`): `skel2d rig examples/face/layers.json --spec examples/rig/spec.json -o face.skel.json` then `skel2d apply face.skel.json examples/face/face.ops.json` (animations face/happy, face/angry, face/sad, blink and talk), then `skel2d render face.skel.json -o f.png --track face/happy --track blink@2.95`.

## Atomicity and errors
A batch runs on a copy: if any op fails or the result is invalid, nothing is saved (no partial apply). Bones are re-sorted parent-first automatically.
- Op failure: `{"error":"bone \"zz\" does not exist","op_index":0}` (JSON parse errors of an op also carry its index).
- Invalid result: `{"error":"document is invalid","issues":[{"path":"slots[s].bone","message":"..."}]}`.
- `--dry-run` / `?dry_run=1` checks without saving.

## Workflow
1. `info` -> names of bones/slots/animations.
2. One `apply` with a batch of ops (cheaper than many calls). `schema ops` if unsure.
3. `pose --anim A --time T` to verify numbers (world x/y/rotation/scale, bone `tip`, slot attachment/color). Cheap; prefer over images.
4. `render -o out.png --anim A --frames 8` to look at the motion; `--no-bones` hides the overlay.
5. `validate` if in doubt.

## CLI
```
skel2d new F [--name N] [--force] | validate F | info F | schema [ops|document]
skel2d apply F (OPSFILE | - | --ops JSON) [--dry-run]
skel2d pose F [--anim A] [--time T] [--since S] [--physics none|simulate] [--fps N] [--move "x,y@t;..."]
skel2d gif F -o OUT.gif [--anim A] [--track N]... [--skin S]... [--all-animations|--all-skins] [--gif-fps N] [--fit WxH] [--transparent] ... (see GIF export)
skel2d render F -o OUT.png [--anim A] [--time T] [--frames N(1..64, needs --anim or --track)] [--width W] [--height H] [--zoom Z] [--center X,Y] [--no-bones] [--track NAME[@T]]... [--physics none|simulate] [--fps N] [--move "x,y@t;..."]
skel2d serve F [--port 7878]
skel2d mcp F                      # MCP stdio server
skel2d image-info IMG.png [--grid OUT.png] [--step 50] [--spec SPEC]
skel2d rig IMG.png|layers.json (--spec FILE | --spec-json JSON) -o OUT [--cell N] [--grid-mesh] [--no-mesh] [--max-influences N] [--force]
skel2d slice SHEET.png -o DIR [--prefix part] [--merge 40] [--padding 4] [--min-pixels 50] [--key RRGGBB] [--defringe N]
skel2d psd FILE.psd -o DIR [--exclude NAME] [--layers | --inner-groups] [--skip-hidden]
skel2d crop SHEET.png --rects RECTS.json -o DIR [--key RRGGBB] [--defringe N]
skel2d key IMG.png -o OUT.png [--color 00ff00] [--tolerance 60]
skel2d schema [ops|document|rig|layers]
```

## HTTP API (`skel2d serve F`, 127.0.0.1 only, default port 7878)
```
GET  /doc | /info | /validate | /schema/ops | /schema/document
GET  /pose?anim=A&time=T[&since=S&physics=none|simulate&fps=N&move=x,y@t;x,y@t]
GET  /render?anim=A&time=T&frames=N&width=W&height=H&zoom=Z&bones=0[&physics=&fps=&move=]     -> image/png
GET  /gif?anim=A&skins=a,b&layers=JSON&options={json}   -> image/gif (see GIF export)
POST /ops[?dry_run=1]    body: op or array       -> {"applied":n,"saved":bool}
POST /undo               reverts last successful /ops (max 100) -> {"undone":true,"remaining":n}; 409 if none
```
```
curl localhost:7878/info
curl -X POST localhost:7878/ops -d '[{"op":"key_bone","animation":"a","bone":"arm","time":1,"rotate":20}]'
curl "localhost:7878/pose?anim=a&time=1"
curl "localhost:7878/render?anim=a&frames=8" -o strip.png
```
Errors: HTTP 400 with the JSON shapes above; unknown route 404. Edits to the file by other tools are picked up before each request (clears undo history). Body max 8 MiB.

## MCP (`skel2d mcp F`)
Stdio MCP server for one document (no port; the client spawns it). Tools: `skel2d_info`, `skel2d_get_document`, `skel2d_apply_ops {ops, dry_run?}`, `skel2d_pose {anim?, time?, physics?, fps?, move?}`, `skel2d_render {anim?, time?, physics?, fps?, move?, frames?, width?, height?, zoom?, center?:[x,y], bones?, tracks?:["NAME"|"NAME@T"]}` (default 384x384, returns an image), `skel2d_schema {kind: ops|document}`, `skel2d_undo`, `skel2d_image_info`, `skel2d_rig` (PNG or layers.json as `image`; see Auto-rig workflow), `skel2d_slice {sheet, out_dir, prefix?, merge?, padding?, min_pixels?}`. Failures are tool results with `isError:true` and the same error JSON as the CLI. Shares session logic with `serve` (external edits picked up per call, atomic saves, undo up to 100).
```
claude mcp add skel2d -- skel2d mcp path/to/file.skel.json
# or .mcp.json: {"mcpServers":{"skel2d":{"command":"skel2d","args":["mcp","path/to/file.skel.json"]}}}
```

## GIF export
`skel2d gif FILE -o OUT [--anim A] [--track NAME]... [--skin S]... [--all-animations] [--all-skins] [options]`; MCP `skel2d_gif {out, anim?, layers?, skins?, all_animations?, all_skins?, options?}`; HTTP `GET /gif?anim=A&skins=a,b&layers=JSON&options={json}` -> `image/gif` (one file; no anim = setup pose). With `--all-animations` / `--all-skins` (also both) `out` is a folder and files are `anim-skin.gif`. CLI/MCP reply `{"files":[{"out","bytes","stats":{"width","height","frames","seconds","colors","missing_images"}}]}`.
`skel2d sheet FILE -o OUT.png [--columns N] [--padding PX]` takes the same frame options as `gif` (`--gif-fps` up to 120) and writes one PNG holding every frame of one play plus `OUT.json`: a `frames` array of `{filename, frame:{x,y,w,h}, duration}` (milliseconds) and a `meta` object. With `--all-animations`/`--all-skins`, `-o` is a folder.
`options` (MCP/HTTP JSON, all optional = `GifOptions` in `crates/skel2d-render/src/gif.rs`; CLI flag in brackets):
```
fps 50 (max 50, [--gif-fps])   colors 256 [--colors]   dither false   quality 5 (1..10)   transparent false   alpha_threshold 128
background [255,255,255] [--background RRGGBB]   forever true [--no-loop]   include_last_frame false   plays 1   pause_after 0 (s)
warm_up 0 (plays simulated first)   bones false   physics true [--no-physics]   scale 100 (%, ignored with fit)
fit [w,h] [--fit WxH]   enlarge false   pad false (exact fit size)   crop [x,y,w,h] (world rect)   range [first,last] (frame numbers from 0)
```
- GIF delays are hundredths of a second, so fps is capped at 50 (anything lower than 2/100 s is slowed by viewers). Identical consecutive frames are merged, so `frames` can be lower than fps x seconds.
- Leave `include_last_frame` off for loops (the last frame repeats the first). GIF pixels are solid or see-through only: with `transparent`, soft edges are flattened over `background`, so choose a color near the page color.
- Layers and skins work like `render`; physics simulates unless `physics:false`. The editor exports with the same options (File > Export GIF..., Ctrl+E; Ctrl+Shift+E repeats).

## Editor + agent workflow
- The editor works in five steps (1 Bones, 2 Images, 3 Mesh, 4 Weights = setup pose; 5 Animation = keys). Adding an image to a slot (step 2, **Add images**) is one `set_attachment` per PNG with a `region` sized from the file, in one undo step.
- The user keeps `skel2d-editor FILE` open on the same file; the agent edits via CLI/HTTP/MCP. The editor reloads external changes live (polls every 500 ms; if the user has unsaved edits they get a reload/keep dialog).
- See what the editor shows: `skel2d-editor FILE --anim A --time T --select BONE [--select-slot SLOT] [--select-ik NAME] --screenshot out.png` (saves the window and exits; `--select-slot` shows a mesh slot's wireframe and vertex weights for the selected bone). `--select-ik NAME` / `--select-physics NAME` show the constraint's inspector (step 1). `--step N` (1-5) opens a step (1 Bones, 2 Images, 3 Mesh, 4 Weights, 5 Animation) and `--lang en|ko` picks the UI language; screenshot runs do not save settings. `--wait S` delays the capture by S seconds. `--tracks a,b` layers those animations in order, first = lowest (with `--time` as the clock, e.g. `--tracks face/happy,blink --time 2.95`).
- The editor draws meshes (deformed by animation/IK), shows vertex weights of the selected bone on the selected mesh slot, and drags vertices (Select tool, setup pose) via `set_vertices`. Its tree has an IK list (+ IK creates a target bone and a constraint); the dope sheet shows `ik: NAME` rows for mix keys.
- Editor dialogs: new/duplicate animation (duration and preview FPS), new skin (optionally merge previewed skins), new IK (bone, optional parent, existing/new target), rename with reference counts, deletion impact, mesh generation and all editor preferences (`Ctrl+,`). Enter applies, Escape cancels; empty/duplicate names disable Apply. These document edits use the same core/asset ops; settings and preview FPS are editor preferences. Up/Down navigate visible list rows; F2/Del act on the selected item (never on root). Mesh trace includes alpha/padding/detail/interior controls and live viewport preview. Additional constraint/export formats remain in P3–P4/P7/P9.
- Editor preview layers: the timeline shows a compact list with the highest track at the top and the editing animation pinned as track 0 below it. Drag a name or use arrow buttons to reorder, edit opacity, toggle enabled, or remove the track; each operation is also reachable through Ctrl+K (opacity presets 25/50/75/100%). Full configuration is scriptable with `--layers`. Setup mode also shows the list when preview tracks exist. Onion skins use the same shared track schedule as the viewport. Renames preserve order/opacity/enabled state; deleting an animation removes its preview entry.

## Auto-rig workflow for agents
Turn a single PNG into a rigged mesh. You state joint positions in image pixels; `skel2d-rig` does the math.
1. `skel2d image-info IMG.png --grid grid.png` -> size, `bbox` of visible pixels, islands. LOOK at `grid.png` (labeled pixel coordinates; via MCP the labels are still in ORIGINAL pixels although the overlay is downscaled to <=1024 px).
2. Write a spec. Pixel coords: x right, y down, origin top-left. Humanoid joints (required: `pelvis`, `neck`, `head_top`; optional `chest`):
   `pelvis chest neck head_top`, and per side (`_l`/`_r`, just be consistent): `shoulder elbow wrist hand`, `hip knee ankle toe`. A limb needs shoulder+elbow+wrist (arm) or hip+knee+ankle (leg), else it is skipped with a warning.
   ```
   {"template":"humanoid","joints":{"head_top":[525,15],"neck":[530,315],"pelvis":[530,740],
     "shoulder_l":[640,360],"elbow_l":[750,570],"wrist_l":[845,705],
     "hip_l":[600,770],"knee_l":[640,1040],"ankle_l":[690,1300],"toe_l":[780,1380], "...":"_r too"},
    "leg_ik":true,"arm_ik":false}
   ```
   Other templates: `{"template":"chain","joints":[[x,y],...]}` or `{"template":"chain","segments":4,"root":"bottom"}` (auto medial axis; root = top|bottom|left|right; bones `chain1..`, `prefix` option), `{"template":"custom","bones":[{"name","parent"?,"start":[x,y],"end"?:[x,y]}]}` (parents first). `skel2d schema rig` for the schema (example: `examples/rig/spec.json`).
3. `skel2d image-info IMG.png --grid grid_spec.png --spec spec.json` marks your joints as magenta crosses; look, fix wrong ones, repeat.
4. `skel2d rig IMG.png --spec spec.json -o hero.skel.json` (`--spec-json '{...}'` inline; `--cell N` mesh density px (outline vertex spacing, or grid cell with `--grid-mesh`), `--no-mesh` single rigid region, `--max-influences N`, `--force`). Prints `{bones, ik, vertices, triangles, warnings, origin_px}`; read `warnings`.
5. Verify: `skel2d render hero.skel.json -o s.png --anim A --frames 8` (e.g. apply `examples/rig/wave.ops.json` first).
6. Refine with ops (`set_vertices`, `auto_weights`, `key_bone`, ...).

Coordinates: 1 world unit per pixel; origin = bottom-center of the visible pixels (`origin_px` in the report), world y up. So world = `(px_x - origin_x, origin_y - px_y)`; feet stand on y = 0.
Humanoid bones: `root` (at pelvis x, y=0), `hip`, `chest`, `head`, `upper_arm_*`, `forearm_*`, `hand_*`, `thigh_*`, `shin_*`, `foot_*`. Leg IK (default on) uses `leg_ik_l/r` with target bones `leg_target_l/r`; `arm_ik: true` adds `arm_ik_l/r` + `arm_target_l/r`. Animate a target bone with `key_bone translate` (offset from its setup position) to move the limb. Vertex weights skip `root` and IK targets.

`add_bone_world`: add a bone by world-space positions; the local transform is computed. `end` omitted = length 0, world rotation 0; `parent` omitted = root.
```
{"op":"add_bone_world","name":"tail2","parent":"tail1","start":[40,120],"end":[70,100]}
```
## Face workflow (layered character with face poses)
1. `skel2d slice sheet.png -o parts` (MCP `skel2d_slice`) -> `parts/part_NN.png` + bbox. LOOK at the pieces (or the sheet) and rename them to meaningful names (`eyes_open`, `eyes_closed`, `mouth_a`, ...). Raise `--merge` if a pair was split, lower it if parts were joined.
2. `skel2d image-info base.png --grid grid.png`; LOOK at the grid to read where the eyes, brows and mouth belong (canvas pixels, y down).
3. Write `layers.json`: body first, then face parts with `center` (+ `scale` if too large) and `"bone":"head"`; put alternative images (closed eyes, mouth shapes, emotes) in `variants`. Mouth shapes named `..._a/_i/_u/_e/_o` make mapping easy.
4. `skel2d rig layers.json --spec spec.json -o char.skel.json`; render the setup pose, adjust `center`/`scale`, rig again.
5. Ops: one animation per emotion (`key_slot` at time 0, e.g. `face/happy`), attachment keys (`key_slot`) into `blink` and `talk`.
6. Check with close-ups: `skel2d render char.skel.json -o c.png --track face/happy --track blink@2.95 --center X,Y --zoom 2` (X,Y = world point on the face: world = (px_x - origin_x, origin_y - px_y), origin from the rig report).

MCP: `skel2d_image_info {image, grid?, step?}` returns the info JSON plus the grid image; `skel2d_rig {image, spec, cell?, grid_mesh?, no_mesh?, max_influences?}` REPLACES the session document (saved; `skel2d_undo` restores the previous one) and returns the report. Relative image paths resolve against the document's directory; the stored image path is relative to the document.

Editor keying: with Auto Key off every animate-mode value edit (bone, deform, slot attachment/tint, IK, copy constraint, physics) stays an unkeyed change (orange) until `K` (Key edited: only what changed; with no change it keys the active tool's property or the slot's attachment) or a key button writes it as ordinary `key_*` ops in one undo step; `Ctrl+Shift+L` keys every dope-sheet row shown. The same keys are reachable from the Ctrl+K palette. The dope sheet addresses single properties (rotate, translate, scale, shear, attachment, tint, deform per skin), so retime/copy/delete act per property, and the overview row acts on everything.
