# Skel2D

A lightweight 2D skeletal animation tool in Rust, built to be driven by AI agents as comfortably as by humans.

## Goals

- **Light and fast**: small Rust binary, CPU-only headless renderer (tiny-skia), no runtime dependencies.
- **Stable**: versioned text format (`skel2d/1`), everything addressed by name, never by index.
- **AI-friendly**:
  - Diff-friendly JSON documents that fit in a context window.
  - One mutation API: atomic JSON edit ops (a batch either fully applies and validates, or nothing changes).
  - Headless PNG rendering (including animation strips) so an agent can look at its work.
  - A plain local HTTP API (curl) instead of MCP, so no tool schemas are loaded into the agent's context.
- Machine-readable errors: failures are JSON (`{"error","op_index"}` or `{"error","issues"}`).

## Workspace layout

```
crates/skel2d-core    model, curves, math, pose evaluation, edit ops, validation, JSON format
crates/skel2d-render  tiny-skia CPU renderer -> PNG (setup pose, frame, frame strip)
crates/skel2d-rig     auto-rigging: image analysis, joint templates, alpha mesh, geodesic weights
crates/skel2d-cli     `skel2d` binary: CLI commands + local HTTP server
crates/skel2d-editor  `skel2d-editor` binary: egui/eframe desktop editor
docs/AI_GUIDE.md      compact reference for AI agents (ops, curves, API)
```

## Build

```
cargo build --release      # binary: target/release/skel2d
powershell -ExecutionPolicy Bypass -File scripts\make_dist.ps1   # Windows zip: dist\Skel2D-<version>-win64.zip
```

`scripts/make_dist.ps1` builds the `dist` profile of `skel2d` and `skel2d-editor`, copies them with `docs/dist/README.txt`, `docs/AI_GUIDE.md`, the manual PDF (`docs/manual/Skel2D_*.pdf`, must exist) and the examples (spineboy, celestial-circus, skins, face, walker, arm, rig; not miya) into `dist\Skel2D-<version>-win64`, checks that every example validates and finds its images, and zips the folder.

## Quick start

Every command prints compact JSON on stdout; errors go to stderr as JSON with exit code 1.

```
skel2d new hero.skel2d.json --name hero            # --force to overwrite

skel2d apply hero.skel2d.json --ops '[
  {"op":"add_bone","name":"root"},
  {"op":"add_bone","name":"arm","parent":"root","x":20,"length":40},
  {"op":"add_slot","name":"arm_slot","bone":"arm","attachment":"arm",
   "attachments":[{"type":"region","name":"arm","x":20,"width":40,"height":10}]},
  {"op":"key_bone","animation":"wave","bone":"arm","time":0,"rotate":0},
  {"op":"key_bone","animation":"wave","bone":"arm","time":0.5,"rotate":45,"curve":"ease_in_out"},
  {"op":"key_bone","animation":"wave","bone":"arm","time":1,"rotate":0}
]'
# also: apply FILE ops.json | apply FILE - (stdin) | --dry-run (validate, don't save)

skel2d info hero.skel2d.json                         # bones, slots, animations
skel2d pose hero.skel2d.json --anim wave --time 0.5  # world-space bone transforms (numbers)
skel2d render hero.skel2d.json -o setup.png          # setup pose
skel2d render hero.skel2d.json -o f.png --anim wave --time 0.5
skel2d render hero.skel2d.json -o strip.png --anim wave --frames 8   # 8 frames side by side
skel2d validate hero.skel2d.json                     # {"valid":true} or all issues
skel2d schema ops                                    # JSON Schema of ops ("document" also)
skel2d serve hero.skel2d.json --port 7878            # HTTP API on 127.0.0.1 (default port 7878)
skel2d mcp hero.skel2d.json                          # MCP server on stdio (for AI agents)
```
Every document has a `root` bone: `skel2d new` starts with it, loading an older file without one adds it, `add_bone` without `parent` puts the bone under root, and root cannot be removed, renamed or given a parent. An `add_bone` named `root` on a document that already has one only overwrites its values (so the quick start above works as written).

Render options: `--width/--height` (default 512, max 4096), `--zoom` (pixels per unit; auto-fit if omitted), `--center X,Y` (world point at the image center, for close-ups with `--zoom`), `--no-bones`, `--frames N` (1..64, needs `--anim` or `--track`), `--track NAME[@SECONDS]` (repeatable, see Face poses and layered animation).

HTTP example:

```
curl localhost:7878/info
curl -X POST localhost:7878/ops -d '{"op":"set_bone","name":"arm","rotation":10}'
curl "localhost:7878/render?anim=wave&frames=6" -o strip.png
curl -X POST localhost:7878/undo
```

MCP (stdio) for agents such as Claude Code:

```
claude mcp add skel2d -- skel2d mcp path/to/hero.skel2d.json
# or in .mcp.json: {"mcpServers":{"skel2d":{"command":"skel2d","args":["mcp","path/to/hero.skel2d.json"]}}}
```

See [docs/AI_GUIDE.md](docs/AI_GUIDE.md) for the full op list and API.

## Meshes and IK

A `mesh` attachment is a triangulated image deformed by weighted bones. Vertex `x`/`y` live in setup-pose skeleton space (where the vertex sits with every bone in its setup pose), `u`/`v` are 0..1 from the image's top-left, `triangles` are index triples, and `weights` (`[{"bone","weight"}]`, normalized when evaluated; empty = the slot's bone) bind vertices to bones. Meshes are bound to the setup pose: changing a bone's setup transform does not move them, only animation and IK do.

```
{"op":"mesh_from_region","slot":"skin","region":"arm","cols":8,"rows":2}
{"op":"auto_weights","slot":"skin","attachment":"arm","bones":["upper","lower"],"max_influences":4}
{"op":"set_vertices","slot":"skin","attachment":"arm","vertices":[{"index":3,"x":40,"y":12}]}
{"op":"add_ik","name":"reach","bones":["upper","lower"],"target":"goal","bend_positive":true}
{"op":"set_ik","name":"reach","mix":0.5}
{"op":"key_ik","animation":"wave","ik":"reach","time":0,"mix":1}
{"op":"remove_ik_key","animation":"wave","ik":"reach","time":0}
{"op":"remove_ik","name":"reach"}
```

IK constraints (`ik` in the document, solved in order after bone animation): one bone aims its +x axis at the target; two bones (a parent and its direct child) bend so the child's tip reaches the target, `bend_positive` = counter-clockwise elbow. `mix` is 0..1 (keyable per animation via `animation.ik`); the target must not be inside the chain. `examples/arm/arm.skel.json` is a meshed two-bone arm reaching for a `goal` bone in the `wave` animation (built by `examples/arm/build.ops.json`). See [docs/AI_GUIDE.md](docs/AI_GUIDE.md) for details.

## Auto-rigging

`skel2d-rig` turns a PNG plus a list of joint positions (image pixels, y down) into a rigged, meshed document. An agent only has to say where the joints are; everything numeric is deterministic:

- The image is placed at 1 world unit per pixel, origin at the bottom center of the visible pixels (feet on y = 0), world y up.
- A grid mesh is traced from the alpha channel and weighted to bones by geodesic distance inside the silhouette (so an arm next to the torso does not bleed into it).
- Templates: `humanoid` (named joints, leg IK with `leg_target_l/r`, optional arm IK), `chain` (given joints or an automatic medial-axis trace), `custom` (explicit bones). `skel2d schema rig` prints the spec schema.
- Bones are created with the core op `add_bone_world` (world-space start/end, local transform computed for you), which is also usable on its own.
- `image-info` reports size, visible bounds and islands, and can write a labeled coordinate grid image (with the spec's joints marked) so vision models can place joints accurately.

```
skel2d image-info examples/rig/character.png --grid grid.png --spec examples/rig/spec.json
skel2d rig examples/rig/character.png --spec examples/rig/spec.json -o character.skel.json
skel2d apply character.skel.json examples/rig/wave.ops.json
skel2d render character.skel.json -o strip.png --anim wave --frames 8
```

Also available over MCP as `skel2d_image_info` and `skel2d_rig`. See [docs/AI_GUIDE.md](docs/AI_GUIDE.md) for the agent workflow.

## Layered characters

`skel2d rig` also accepts a `layers.json` manifest (one PNG per part, back to front) instead of a single PNG. Each layer becomes a slot; `skel2d schema layers` prints the schema.

```
{"canvas":[1000,1500],                       // optional; default = extent of all layers
 "layers":[
  {"name":"body","image":"base.png"},        // x/y = top-left on the canvas (default 0,0)
  {"name":"eyes","image":"parts/eyes_open.png","center":[527,212],"scale":0.85,"bone":"head",
   "variants":[{"name":"eyes_closed","image":"parts/eyes_closed.png","center":[527,220]},
               {"name":"eyes_happy","image":"parts/eyes_happy.png"}]}]}
```

- Paths are relative to the manifest; the document stores them relative to the output file.
- `x`,`y` is the top-left corner; `center` (overrides x/y) puts the scaled image's middle on that canvas pixel; `scale` = image px to canvas px.
- `attachment` names the default attachment (default: the layer name); `hidden` hides the slot in the setup pose.
- `bone` attaches the layer rigidly to that bone; `bones` restricts mesh weights to those bones; `mesh` forces a weighted mesh (true) or a rigid region (false).
- Default without `mesh`/`bone`: a layer covering at least 1.5% of all visible pixels becomes a weighted mesh (weights flow only through its own pixels), smaller ones are regions on the nearest bone.
- `variants` are extra attachments of the same slot (closed eyes, mouth shapes). A variant defaults to the layer's center and scale; it may set `center` or `x`+`y`, and `scale`.

`skel2d psd` splits a layered Photoshop file: every top-level group becomes one part (its layers composited, e.g. line art over flat color), and an ungrouped layer is a part of its own; with `--layers` every layer is a part and groups only organize them (one part per layer); with `--inner-groups` every innermost group is a part. A layer tagged `[origin]` or the first guides set the origin (else the canvas center). Hidden layers are included unless you pass `--skip-hidden`; a clipped layer is painted onto its base, not made a part; a paper background layer (`용지`/`Paper`/`用紙`) is skipped. It writes one cropped PNG per part, `layers.json` (back to front, canvas `x`/`y`) and `composite.png` (all parts, for `image-info --grid` when placing joints). Add `bone`/`bones`/`mesh` to the manifest, then rig it.

```
skel2d psd character.psd -o parts --exclude reference   # --exclude skips a group or layer (repeatable), --layers: one part per layer, --inner-groups: one part per innermost group, --skip-hidden: leave out hidden layers
```

Example: `examples/skins` (one skeleton, three outfits knight / mage / ranger as skins, animations idle, walk and wave; see its README.txt). Example: `examples/miya` (PSD with arms and legs as one weighted mesh each, hands, feet and head rigid on their bones):

```
skel2d psd examples/miya/miya_a_idle02_field.psd -o examples/miya/parts --exclude miya_a_idle02_field
skel2d rig examples/miya/layers.json --spec examples/miya/spec.json -o miya.skel.json --cell 8
```

`skel2d slice` cuts a parts sheet (one PNG, transparent background) into one PNG per part, in reading order (rows top to bottom, then left to right):

```
skel2d slice sheet.png -o parts --prefix part --merge 40 --padding 4 --min-pixels 50
# -> {"pieces":[{"file":"parts/part_01.png","bbox":{...},"size":[w,h]}, ...]}
```

Pieces closer than `--merge` pixels stay together (a pair of eyes is one part); specks under `--min-pixels` are ignored. Existing files are never overwritten. Over MCP: `skel2d_slice`; `skel2d_rig` takes the manifest path as `image`.

Cleaning AI-generated sheets (image generators rarely produce clean transparency):

```
skel2d key sheet.png -o sheet_clean.png --color 00ff00 --tolerance 60   # chroma-key a flat background
skel2d slice sheet.png -o parts --key 00ff00 --defringe 32              # key + drop faint halo/specks, then slice
skel2d crop sheet.png --rects rects.json -o parts --defringe 32         # cut named rects when pieces sit too close to slice
```

`--defringe N` makes alpha below N transparent and erases specks under `--min-pixels`. `rects.json` is `[{"name":"eyes_open","rect":[x0,y0,x1,y1]}]`. Asking the generator for a flat `#00FF00` background and keying it out is far more reliable than asking for transparency.

## Face poses and layered animation

A *face pose* is an ordinary animation keyed at time 0 (one `key_slot` per slot; attachment, `null` = hidden, optional color), played as a layered track:

```
{"op":"key_slot","animation":"face/happy","slot":"eyes","time":0,"attachment":"eyes_happy"}
{"op":"key_slot","animation":"face/happy","slot":"cheek","time":0,"color":"#ff9090"}
```

An attachment key with `"reset":true` (`key_slot ... "reset":true`) shows what the lower tracks (or the setup pose) show, so blink and talk layered above a face pose return to that face's eyes and mouth.

Several animations can play at once (`Pose::compute_with`, used by render and the editor). Tracks apply in order (first is lowest); a track only affects the timelines it keys and replaces what earlier tracks set, blended by its alpha (1 in the CLI and editor). Attachments switch only at alpha >= 0.5. Typical layering: main animation (walk), then `blink`, then `talk`.

```
{"op":"key_slot","animation":"blink","slot":"eyes","time":2.88,"attachment":"eyes_closed"}
{"op":"key_slot","animation":"blink","slot":"eyes","time":3,"reset":true}
{"op":"key_slot","animation":"talk","slot":"mouth","time":0,"attachment":"mouth_a"}
{"op":"key_slot","animation":"talk","slot":"mouth","time":0.15,"reset":true}
```

Blink and mouth animations are ordinary attachment keys, one `key_slot` per image change.

```
skel2d render face.skel.json -o f.png --track face/happy
skel2d render face.skel.json -o f.png --track blink@2.95            # fixed time
skel2d render face.skel.json -o s.png --anim walk --track blink --track talk --frames 12   # tracks loop over their own duration
skel2d render face.skel.json -o close.png --track face/sad --center 0,1250 --zoom 2.5   # close-up
```

`--track NAME@T` fixes the track's time; without `@` it follows the frame time, looping over its own duration. Frames span the main animation (or the longest track).

Example: `examples/clipping` (a potion bottle: the `liquid` and `shine` slots clip to `bottle` and move on their own bones; animations `slosh`, `shine`, `idle`).

Example: `examples/face` (layered face with face poses `face/happy`, `face/angry`, `face/sad` and `blink` / `talk` animations):

```
skel2d rig examples/face/layers.json --spec examples/rig/spec.json -o face.skel.json
skel2d apply face.skel.json examples/face/face.ops.json
skel2d render face.skel.json -o face.png --track face/happy --track blink@2.95
```

## Editor

```
cargo run --release -p skel2d-editor -- path/to/file.skel.json
```

The editor is organized by what you are doing. Five numbered steps run along the top (keys `1` to `5`); the left column shows the tools of the step, the right column the step's list and the panel of the picked item, and a line over the viewport says what you can do right now.

1. **Bones**: draw the skeleton. Tools: Select (B), New bone (N), Move (V), Rotate (C), Scale (X). The list is the bone tree; `root` is always on top (it cannot be deleted, renamed or given a parent, but it can be moved, rotated, scaled and keyed; every other bone hangs under it). New bones go under the selected bone, or under root. Groups in the bone panel add *Follow target (IK)* and *Jiggle (physics)*.
2. **Images**: load PNG/PSD files (button, File menu, or drop them on the viewport). The list is the slots in front-to-back order with thumbnails; drag to reorder. A slot shows one image at a time but can hold many: the slot panel lists them as cards (thumbnail, name, kind, *Showing*), **Add images** adds more PNGs to the slot, and each card has a menu (rename, replace the file, make a mesh, delete). The *Outfits (skin)* group previews and manages skins.
3. **Mesh**: pick an image. A *Picture* shows "not a mesh yet" and **Make mesh** opens the live-preview dialog; a *Shared mesh* offers **Go to parent mesh**; a mesh gets the modes Move / Add / Delete / Fold line (each explained), vertex actions, rebuild and *Make shared mesh*.
4. **Weights**: **Set weights automatically** first, then the bound bones with their percentage and lock, the brush (Add / Remove / Replace, size, strength), a "bend a bone to test" check (Alt+drag), and smooth / copy / paste / clean up / swap / transfer / mirror.
5. **Animation**: pick or create an animation (**New animation**), move the time marker and change bones or press an image card of the selected slot to switch images at that time (blinking, mouth shapes, face poses). Key buttons show state by shape and word: filled diamond *Keyed*, hollow diamond *Key* / *Between*, filled circle *Changed*. The timeline shows *Play together with* for preview animations.

Other: Space play/pause, K key the selected bone, F frame, Ctrl+Z undo, Ctrl+Shift+Z / Ctrl+Y redo, Ctrl+S save, Ctrl+O open, Ctrl+K command palette, F1 help for the step. View menu: UI size 100 / 125 / 150%. Middle or right drag pans, the wheel zooms.

Options: `--anim NAME` (open this animation), `--time SECONDS` (playhead), `--step 1-5` (open in this step), `--lang en|ko` (language for this run), `--select BONE`, `--select-slot SLOT`, `--select-ik NAME`, `--select-physics NAME`, `--tool NAME` (pose, create, weights, mesh, rotate, translate, scale), `--tracks A,B` (preview animations layered in order, first = lowest, e.g. `--tracks face/happy,blink`), `--layers JSON` (ordered `{anim,alpha,enabled}` preview tracks; alternative to `--tracks`), `--skin NAME` (repeatable), `--size WxH`, `--screenshot OUT.png` (save the window and exit), `--wait SECONDS` (delay the capture, with `--screenshot`).

- Steps 1-4 edit the setup pose with `set_bone`, `set_attachment` and friends; step 5 keys values with `key_bone`, `key_slot` and friends at the current time.
- Live reload: the editor polls the file every 500 ms and reloads external edits (CLI, HTTP, MCP). If the editor has unsaved changes, a dialog asks whether to reload or keep yours.
- Bone drawing (step 1, New bone): drag in the viewport to draw a bone (`add_bone_world`) as a child of the selected bone; starting the drag near the selected bone's tip chains from it. The new bone becomes selected, so consecutive drags draw a chain. Ctrl toggles grid snapping.
- Images: a PSD is split like `skel2d psd` (a dialog asks: one part per layer, or per top-level group) into a new `<name>_parts` folder beside it. Each image becomes a slot with a region at 1 unit per pixel, upright, centered on the drop point, attached to the bone under the cursor, else the selected bone, else root. Paths are stored relative to the document. Changing a slot's **Bone** keeps its pictures in place on screen. **Add images** gives the slot new regions (one `set_attachment` per file, sized from the PNG, named after the file, placed where the shown image is), in one undo step.
- Meshes are drawn deformed by animation and IK. **Make mesh** (density low / medium / high) replaces a region with an adaptive mesh over the image's visible pixels; **Rebuild mesh** does the same over a mesh's image and keeps the weights. Mesh edits are `set_attachment` ops; the viewport shows the pure setup pose without IK while the Mesh step is active.
- Weights: painting is one `set_vertices` per stroke. Other bones are rescaled so each vertex totals 1.
- IK: **Add follow target (IK)** creates a `<bone>_target` bone at the bone's tip plus a 1- or 2-bone constraint (`add_ik`). In step 5 the IK mix can be keyed.
- Every edit goes through the same op API as the CLI, HTTP and MCP servers. Blink and mouth animations are made by switching images over time (step 5).

## GIF export

Animations export to looping GIFs from the editor (File > Export GIF..., `Ctrl+E`; `Ctrl+Shift+E` repeats the last export), the CLI, HTTP and MCP. Previewed skins and layered preview animations are included.

```
skel2d gif hero.skel.json -o walk.gif --anim walk --gif-fps 25 --fit 400x400
skel2d sheet hero.skel.json -o walk.png --anim walk --gif-fps 30 --columns 8 --padding 2 --transparent  # PNG plus walk.json (frames array)
skel2d gif hero.skel.json -o out --all-animations --skin knight       # one file per animation, -o is a folder
skel2d gif hero.skel.json -o out --all-skins --anim idle              # one file per skin
skel2d gif hero.skel.json -o f.gif --anim walk --track blink --transparent --background 202020
curl "localhost:7878/gif?anim=walk&skins=mage&options={\"fps\":20,\"fit\":[160,160]}" -o walk.gif   # MCP: skel2d_gif
```

- No `--anim` exports the setup pose. `--gif-fps` is at most 50: GIF delays are hundredths of a second and viewers slow anything below 2.
- Loop frames: leave `--include-last-frame` off, the last frame repeats the first. Also `--no-loop`, `--plays`, `--pause-after`, `--warm-up` (physics starts in motion) and `--range FIRST-LAST`.
- Size: `--scale` percent, or `--fit WxH` with `--enlarge` and `--pad` (exact size), `--crop X,Y,W,H` (world rectangle). Colors: `--colors`, `--dither`, `--quality`, `--background RRGGBB`, `--transparent` with `--alpha-threshold`. GIF pixels are solid or see-through, so soft edges mix with the background color: pick one close to the page color. `--bones`, `--no-physics`.
- The editor runs the export in the background. Its top-right progress window shows the file name, percentage and batch position; Cancel switches to a disabled Cancelling state. Completion, failure and cancellation appear once as bottom-right toasts. The status bar keeps shortcut hints, selection and zoom.

## Roadmap

- ~~GUI editor (egui)~~ done, see Editor
- ~~Mesh attachments and vertex weights~~ done, see Meshes and IK
- ~~IK constraints~~ done
- ~~AI auto-rigging from images~~ done, see Auto-rigging
- ~~Layered characters~~ done, see Layered characters
- ~~GIF export~~ done, see GIF export
- Runtime crate and further export formats
