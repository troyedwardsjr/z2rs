#!/usr/bin/env python3
"""Editable sheets to paint over, and the pack cut back out of them.

    python3 tools/hd-sheets/paint_sheets.py build --shots DIR... --out SHEETS
        [--rom "$Z2_ROM"] [--scale 4] [--name HEX=NAME]... [--bleed 8] [--max-figure 64]
    python3 tools/hd-sheets/paint_sheets.py cut --sheets SHEETS --out PACK_DIR
        [--rom "$Z2_ROM"] [--name NAME] [--layer SPEC]... [--propagate sprites|all|none]

`make hd-sheets` and `make hd-pack` run both steps for the whole game.

`build` reads the `*.tilemap.tsv` files `article_shots --tilemap` wrote (any
number of capture directories) and lays the game out the way it is drawn
rather than the way a pack stores it:

* **figures** (`sheets/fig-*.png`): every sprite pose seen, whole. Sprites are
  grouped into a figure when they touch and share a palette, so one slot holds
  a whole Link, enemy or NPC in one animation frame, not eight 8x8 fragments.
  Slots carry a `--bleed` margin of empty pixels: paint into it and the art
  reaches past the NES sprite boxes.
* **tilesets** (`sheets/tile-*.png`): every 16x16 metatile of every scene seen,
  four CHR tiles each, most-used first. Painting a metatile paints all four.
  A CHR tile with no pattern at all draws the backdrop colour and is the same
  tile in every empty sky in the game, so blank quads are left out of the
  sheet and never become cells; paint a sky with a back layer instead.

Both are drawn from your ROM's CHR in the colours the game used, so a sheet
opens as a picture of the game. Paint over a slot and `cut` slices it back
into pack cells; a slot left exactly as written is skipped, so a half-painted
sheet gives a half-covered pack and everything else keeps the original art.

`cut` writes a complete pack (`pack.json`, page sheets, a free sheet for
matte-shaped sprites). Sprite cells come out shaped by the painted alpha, with
`bleed` where the paint runs past the 8x8 box, so a hat or a sword may leave
the NES silhouette. The ROM keeps copies of many sprite tiles on several CHR
pages; `--propagate sprites` (the default) registers each painted sprite cell
on every page that holds the same tile, so a character painted in one place
looks the same everywhere.

`--layer` adds a whole image behind or over one scene's background (a sky,
distant hills). SPEC is comma-separated: `file=PATH` and any of
`depth=back|front`, `x=N`, `y=N` (NES pixels), `scroll=0..100` (percent of the
camera movement followed), `repeat_x`, `over=PAGE:TILE+PAGE:TILE` (front
layers only: cover just those tiles) and `when=WORLD:REGION:SCENE`.

Both directories hold ROM-derived art, so keep them outside the repository
(LEGAL.md).
"""

import argparse
import collections
import glob
import hashlib
import json
import os
import shutil
import subprocess
import sys

import numpy as np
from PIL import Image

MAX_BLEED = 8           # NES pixels; `z2_render::MAX_BLEED`
MAX_SHEET_PX = 4096     # largest pack sheet the loader accepts, per side
GUTTER = 6              # NES pixels of ignored space around every slot
BACKDROP = (28, 28, 34)  # sheet background, never sampled


# --------------------------------------------------------------------------
# shared helpers
# --------------------------------------------------------------------------

def in_git_worktree(path):
    probe = os.path.abspath(path)
    while not os.path.isdir(probe):
        probe = os.path.dirname(probe)
    done = subprocess.run(["git", "-C", probe, "rev-parse", "--is-inside-work-tree"],
                          capture_output=True, text=True)
    return done.returncode == 0 and done.stdout.strip() == "true"


def load_chr(rom_path):
    rom = open(rom_path, "rb").read()
    if rom[:4] != b"NES\x1a":
        sys.exit(f"{rom_path}: not an iNES file")
    start = 16 + (512 if rom[6] & 4 else 0) + rom[4] * 16384
    return rom[start:]


def tile_bytes(chr_rom, page, tile):
    off = page * 4096 + tile * 16
    return chr_rom[off:off + 16]


def tile_values(chr_rom, page, tile):
    """8x8 array of 2-bit pattern values."""
    raw = tile_bytes(chr_rom, page, tile)
    if len(raw) < 16:
        return np.zeros((8, 8), np.uint8)
    lo = np.unpackbits(np.frombuffer(raw[:8], np.uint8)[:, None], axis=1)
    hi = np.unpackbits(np.frombuffer(raw[8:], np.uint8)[:, None], axis=1)
    return lo | (hi << 1)


def tile_rgba(chr_rom, palette, page, tile, colors, flip_h=False, flip_v=False):
    """One CHR tile as 8x8 RGBA in `colors`; pattern value 0 is transparent."""
    v = tile_values(chr_rom, page, tile)
    if flip_h:
        v = v[:, ::-1]
    if flip_v:
        v = v[::-1]
    out = np.zeros((8, 8, 4), np.uint8)
    for value, index in enumerate(colors, start=1):
        m = v == value
        out[m, :3] = palette[index & 0x3F]
        out[m, 3] = 255
    return out


def hash_slot(rgba):
    return hashlib.sha256(np.ascontiguousarray(rgba).tobytes()).hexdigest()[:16]


def shot_paths(dirs):
    out = []
    for d in dirs:
        out += sorted(glob.glob(os.path.join(d, "*.tilemap.tsv")))
    return out


def iter_shots(paths):
    """One capture at a time: a whole run's rows never sit in memory at once."""
    for path in paths:
        with open(path) as handle:
            rows = [line.rstrip("\n").split("\t") for line in handle][1:]
        scene_path = path[: -len(".tilemap.tsv")] + ".scene.json"
        scene = json.load(open(scene_path)) if os.path.exists(scene_path) else None
        yield os.path.basename(path)[: -len(".tilemap.tsv")], rows, scene


def find_palette(dirs):
    for d in dirs:
        p = os.path.join(d, "palette.png")
        if os.path.exists(p):
            return np.asarray(Image.open(p).convert("RGB")).reshape(-1, 3)[:64]
    sys.exit("no palette.png in any --shots directory; re-run article_shots --tilemap")


def colour_tag(colors):
    return "c{:02X}-{:02X}-{:02X}".format(*colors)


# --------------------------------------------------------------------------
# build: figures
# --------------------------------------------------------------------------

def sprite_boxes(rows):
    """`(x, top) -> ((page, tile, colours), flip_h, flip_v)` for one frame."""
    out = collections.OrderedDict()
    for r in rows:
        if r[0] != "spr":
            continue
        y, x = int(r[1]), int(r[2])
        key = (int(r[3]), int(r[4]), (int(r[5]), int(r[6]), int(r[7])))
        out.setdefault((x, y - int(r[9]) % 8), (key, r[10] == "1", r[11] == "1"))
    return out


def group_figures(boxes, limit):
    """Boxes that touch and share a palette are one figure."""
    items = list(boxes.items())
    parent = list(range(len(items)))

    def find(i):
        while parent[i] != i:
            parent[i] = parent[parent[i]]
            i = parent[i]
        return i

    for i, ((ax, ay), (ka, _, _)) in enumerate(items):
        for j in range(i):
            (bx, by), (kb, _, _) = items[j]
            if ka[2] == kb[2] and abs(ax - bx) <= 8 and abs(ay - by) <= 8:
                parent[find(i)] = find(j)
    groups = collections.defaultdict(list)
    for i, item in enumerate(items):
        groups[find(i)].append(item)
    out = []
    for g in groups.values():
        x0 = min(b[0][0] for b in g)
        y0 = min(b[0][1] for b in g)
        w = max(b[0][0] for b in g) + 8 - x0
        h = max(b[0][1] for b in g) + 8 - y0
        # A column of edge-mask sprites is not a figure.
        if w <= limit and h <= limit:
            out.append(((x0, y0, w, h), g))
    return out


def add_figures(rows, limit, poses, seen, name):
    """Fold one capture's figures into `poses` / `seen`."""
    for (x0, y0, w, h), g in group_figures(sprite_boxes(rows), limit):
        cells = tuple(sorted(
            (bx - x0, by - y0, k[0], k[1], k[2], fh, fv)
            for (bx, by), (k, fh, fv) in g))
        seen[cells] += 1
        poses.setdefault(cells, (w, h, g[0][1][0][2], name))


# --------------------------------------------------------------------------
# build: tilesets
# --------------------------------------------------------------------------

def add_metatiles(rows, scene, scenes):
    """Fold one capture's background into `scenes` (level coordinates)."""
    if scene is None:
        return
    key = (scene["world"], scene["region"], scene["scene"])
    margin, cam = scene["margin_px"], scene["camera_x"]
    grid = {}
    for r in rows:
        if r[0] != "bg":
            continue
        y, x, page, tile = int(r[1]), int(r[2]), int(r[3]), int(r[4])
        colors = (int(r[5]), int(r[6]), int(r[7]))
        prow, col0, length = int(r[8]), int(r[9]), int(r[10])
        ox, oy = x - col0, y - prow
        if (oy % 8) or oy < 32 or length + col0 < 8:
            continue                           # partial run or a HUD row
        wx = cam + ox - margin
        if wx % 8:
            continue
        grid[(wx // 8, oy // 8)] = (page, tile, colors)
    for (tx, ty), cell in grid.items():
        mx, my = tx // 2, (ty - 4) // 2
        if my < 0:
            continue
        quad = (tx & 1) * 2 + ((ty - 4) & 1)    # column-major, as the ROM stores it
        scenes[key].setdefault((mx, my), [None] * 4)[quad] = cell


def whole_metatiles(scenes):
    """Only the metatiles every quadrant of which was seen."""
    out = {}
    for key, cells in scenes.items():
        whole = {pos: quads for pos, quads in cells.items() if all(q is not None for q in quads)}
        if whole:
            out[key] = whole
    return out


def dedupe_metatiles(cells, blank):
    """Distinct metatiles, most common first, with how often each was drawn.
    A metatile with nothing but blank CHR tiles is not worth a slot."""
    count = collections.Counter(tuple(quads) for quads in cells.values())
    return [(quads, n) for quads, n in count.most_common()
            if any(not blank(q) for q in quads)]


# --------------------------------------------------------------------------
# build: sheet layout
# --------------------------------------------------------------------------

def lay_out(slots, scale, per_row_px=1024):
    """Place `(w, h)` slots left to right, rows as tall as their tallest slot."""
    pitch = GUTTER
    places, x, y, row_h = [], pitch, pitch, 0
    limit = max(per_row_px // scale, max((w for w, _ in slots), default=8) + 2 * pitch)
    for w, h in slots:
        if x + w + pitch > limit and row_h:
            x, y, row_h = pitch, y + row_h + pitch, 0
        places.append((x, y))
        x += w + pitch
        row_h = max(row_h, h)
    return places, limit, y + row_h + pitch


def write_sheet(path, slots, draw, scale):
    """`slots` is `[(w, h)]`; `draw(i, canvas, x, y)` paints slot `i`."""
    places, w, h = lay_out(slots, scale)
    canvas = np.zeros((h * scale, w * scale, 4), np.uint8)
    canvas[..., :3] = BACKDROP
    canvas[..., 3] = 255
    for i, (x, y) in enumerate(places):
        draw(i, canvas, x * scale, y * scale)
    Image.fromarray(canvas, "RGBA").save(path)
    return places, (w, h)


def blit(canvas, rgba, x, y, scale):
    """Nearest-upscale `rgba` by `scale` into `canvas` at pixel `(x, y)`."""
    big = np.repeat(np.repeat(rgba, scale, 0), scale, 1)
    h, w = big.shape[:2]
    view = canvas[y:y + h, x:x + w]
    m = big[..., 3] > 0
    view[m] = big[m]
    view[~m] = 0                       # transparent: paintable


def build(args):
    chr_rom = load_chr(args.rom)
    blank_cache = {}

    def blank(cell):
        key = (cell[0], cell[1])
        if key not in blank_cache:
            blank_cache[key] = not any(tile_bytes(chr_rom, *key))
        return blank_cache[key]

    palette = find_palette(args.shots)
    paths = shot_paths(args.shots)
    if not paths:
        sys.exit("no *.tilemap.tsv found; run article_shots --tilemap first")
    poses_map, pose_seen = {}, collections.Counter()
    scene_cells = collections.defaultdict(dict)
    for i, (name, rows, scene) in enumerate(iter_shots(paths), start=1):
        add_figures(rows, args.max_figure, poses_map, pose_seen, name)
        add_metatiles(rows, scene, scene_cells)
        if i % 500 == 0:
            print(f"  read {i}/{len(paths)} captures", flush=True)
    names = dict(n.split("=", 1) for n in args.name)
    os.makedirs(os.path.join(args.out, "sheets"), exist_ok=True)
    manifest = {"scale": args.scale, "bleed": args.bleed, "gutter": GUTTER,
                "figures": [], "tilesets": []}
    report = []

    # ---- figures, one sheet per palette --------------------------------------
    poses = [(cells, *poses_map[cells], pose_seen[cells]) for cells in poses_map]
    by_pal = collections.defaultdict(list)
    for pose in poses:
        by_pal[pose[3]].append(pose)
    for colors, group in sorted(by_pal.items()):
        group.sort(key=lambda p: (-p[5], -p[2], -p[1]))
        b = args.bleed
        slots = [(p[1] + 2 * b, p[2] + 2 * b) for p in group]
        tag = names.get(colour_tag(colors), colour_tag(colors))
        name = f"sheets/fig-{tag}.png"

        def draw(i, canvas, x, y, group=group, b=b, slots=slots):
            # The whole slot, bleed margin included, is paintable: transparent.
            sw, sh = slots[i]
            canvas[y:y + sh * args.scale, x:x + sw * args.scale] = 0
            for dx, dy, page, tile, cols, fh, fv in group[i][0]:
                art = tile_rgba(chr_rom, palette, page, tile, cols, fh, fv)
                blit(canvas, art, x + (dx + b) * args.scale, y + (dy + b) * args.scale, args.scale)

        path = os.path.join(args.out, name)
        places, size = write_sheet(path, slots, draw, args.scale)
        sheet = np.asarray(Image.open(path).convert("RGBA"))
        entries = []
        for (x, y), pose in zip(places, group):
            cells, w, h = pose[0], pose[1], pose[2]
            sw, sh = (w + 2 * b) * args.scale, (h + 2 * b) * args.scale
            entries.append({
                "x": x * args.scale, "y": y * args.scale, "w": sw, "h": sh,
                "core_x": b * args.scale, "core_y": b * args.scale,
                "seen": pose[5],
                "hash": hash_slot(sheet[y * args.scale:y * args.scale + sh,
                                        x * args.scale:x * args.scale + sw]),
                "cells": [{"dx": c[0], "dy": c[1], "page": c[2], "tile": c[3],
                           "colors": list(c[4]), "flip_h": c[5], "flip_v": c[6]}
                          for c in cells],
            })
        manifest["figures"].append({"file": name, "colors": list(colors), "slots": entries})
        report.append(f"{name}: {len(entries)} pose(s), {size[0]}x{size[1]} NES px")

    # ---- tilesets, one sheet per scene ---------------------------------------
    for key, cells in sorted(whole_metatiles(scene_cells).items()):
        world, region, scene = key
        metas = dedupe_metatiles(cells, blank)
        slots = [(16 + 2 * GUTTER, 16 + 2 * GUTTER)] * len(metas)
        tag = f"w{world}-r{region}-s{scene:02d}"
        name = f"sheets/tile-{names.get(tag, tag)}.png"

        def draw(i, canvas, x, y, metas=metas):
            inner = (GUTTER * args.scale, (GUTTER + 16) * args.scale)
            canvas[y + inner[0]:y + inner[1], x + inner[0]:x + inner[1]] = 0
            for quad, cell in enumerate(metas[i][0]):
                if blank(cell):
                    continue
                page, tile, cols = cell
                art = tile_rgba(chr_rom, palette, page, tile, cols)
                qx, qy = (quad // 2) * 8, (quad % 2) * 8
                blit(canvas, art, x + (GUTTER + qx) * args.scale,
                     y + (GUTTER + qy) * args.scale, args.scale)

        path = os.path.join(args.out, name)
        places, size = write_sheet(path, slots, draw, args.scale)
        sheet = np.asarray(Image.open(path).convert("RGBA"))
        entries = []
        for (x, y), (quads, n) in zip(places, metas):
            sx, sy = (x + GUTTER) * args.scale, (y + GUTTER) * args.scale
            side = 16 * args.scale
            entries.append({
                "x": sx, "y": sy, "w": side, "h": side, "core_x": 0, "core_y": 0, "seen": n,
                "hash": hash_slot(sheet[sy:sy + side, sx:sx + side]),
                "cells": [{"dx": (q // 2) * 8, "dy": (q % 2) * 8, "page": c[0], "tile": c[1],
                           "colors": list(c[2]), "flip_h": False, "flip_v": False}
                          for q, c in enumerate(quads) if not blank(c)],
            })
        manifest["tilesets"].append({"file": name, "scene": {"world": world, "region": region,
                                                             "scene": scene}, "slots": entries})
        report.append(f"{name}: {len(entries)} metatile(s) of scene "
                      f"world {world} region {region} scene {scene}")

    with open(os.path.join(args.out, "sheets.json"), "w") as handle:
        json.dump(manifest, handle, indent=1)
        handle.write("\n")
    with open(os.path.join(args.out, "SHEETS-ROM-DERIVED.txt"), "w") as handle:
        handle.write("These sheets are drawn from the CHR graphics in your ROM. Keep them, and "
                     "anything cut from them, outside the repository (LEGAL.md).\n")
    with open(os.path.join(args.out, "build-report.txt"), "w") as handle:
        handle.write("\n".join(report) + "\n")
    figs = sum(len(f["slots"]) for f in manifest["figures"])
    tiles = sum(len(t["slots"]) for t in manifest["tilesets"])
    print(f"{args.out}: {len(manifest['figures'])} figure sheet(s) with {figs} pose(s), "
          f"{len(manifest['tilesets'])} tileset sheet(s) with {tiles} metatile(s), from "
          f"{len(paths)} capture(s)")


# --------------------------------------------------------------------------
# cut
# --------------------------------------------------------------------------

def nearest_owner(alpha, cores, reach):
    """Each painted pixel goes to the nearest core box within `reach` pixels."""
    owner = np.full(alpha.shape, -1, np.int32)
    dist = np.full(alpha.shape, np.inf, np.float32)
    yy, xx = np.mgrid[0:alpha.shape[0], 0:alpha.shape[1]]
    for i, (x0, y0, x1, y1) in enumerate(cores):
        d = np.hypot(np.maximum(np.maximum(x0 - xx, xx - (x1 - 1)), 0),
                     np.maximum(np.maximum(y0 - yy, yy - (y1 - 1)), 0))
        closer = d < dist
        owner[closer], dist[closer] = i, d[closer]
    owner[~alpha | (dist > reach)] = -1
    return owner


def cut(args):
    man = json.load(open(os.path.join(args.sheets, "sheets.json")))
    scale, bleed = man["scale"], man["bleed"]
    cell_px = 8 * scale
    if in_git_worktree(args.out):
        sys.exit(f"{args.out}: inside a git work tree; keep a pack cut from ROM-derived sheets "
                 "outside the repository (LEGAL.md)")

    chr_rom = load_chr(args.rom)
    bg_cells, spr_cells, report = {}, {}, []
    painted_slots = skipped = 0
    for kind in ("tilesets", "figures"):
        for sheet in man[kind]:
            path = os.path.join(args.sheets, sheet["file"])
            if not os.path.exists(path):
                report.append(f"skip {sheet['file']}: not in the sheets directory")
                continue
            img = np.asarray(Image.open(path).convert("RGBA"))
            for slot in sheet["slots"]:
                x, y, w, h = slot["x"], slot["y"], slot["w"], slot["h"]
                box = img[y:y + h, x:x + w]
                if hash_slot(box) == slot["hash"]:
                    skipped += 1
                    continue                       # untouched: keep the original art
                painted_slots += 1
                alpha = box[..., 3] >= 128
                cores = [(slot["core_x"] + c["dx"] * scale, slot["core_y"] + c["dy"] * scale,
                          slot["core_x"] + (c["dx"] + 8) * scale,
                          slot["core_y"] + (c["dy"] + 8) * scale) for c in slot["cells"]]
                owner = (nearest_owner(alpha, cores, bleed * scale) if kind == "figures"
                         else np.where(alpha, 0, -1))
                for i, c in enumerate(slot["cells"]):
                    key = (c["page"], c["tile"], tuple(c["colors"]))
                    cx0, cy0, cx1, cy1 = cores[i]
                    if kind == "tilesets":
                        art = box[cy0:cy1, cx0:cx1].copy()
                        art[art[..., 3] < 128] = 0
                        art[..., 3] = np.where(art[..., 3] >= 128, 255, 0)
                        if not art[..., 3].any():
                            continue
                        keep = int((art[..., 3] > 0).sum())
                        if bg_cells.get(key, (None, -1))[1] < keep:
                            bg_cells[key] = (art, keep)
                        continue
                    mine = owner == i
                    if not mine.any():
                        continue
                    ys, xs = np.nonzero(mine)
                    b = [max(0, -(-(cx0 - xs.min()) // scale)),
                         max(0, -(-(cy0 - ys.min()) // scale)),
                         max(0, -(-(xs.max() + 1 - cx1) // scale)),
                         max(0, -(-(ys.max() + 1 - cy1) // scale))]
                    b = [int(min(v, MAX_BLEED)) for v in b]
                    slot_px = (8 + 2 * MAX_BLEED) * scale
                    core0 = MAX_BLEED * scale
                    art = np.zeros((slot_px, slot_px, 4), np.uint8)
                    sx0, sy0 = cx0 - core0, cy0 - core0
                    dx0, dy0 = max(0, -sx0), max(0, -sy0)
                    ax0, ay0 = max(sx0, 0), max(sy0, 0)
                    ax1, ay1 = min(sx0 + slot_px, w), min(sy0 + slot_px, h)
                    view = art[dy0:dy0 + (ay1 - ay0), dx0:dx0 + (ax1 - ax0)]
                    view[:] = box[ay0:ay1, ax0:ax1]
                    view[..., 3] = mine[ay0:ay1, ax0:ax1] * 255
                    art[art[..., 3] == 0] = 0
                    if c["flip_h"]:
                        art, b = art[:, ::-1], [b[2], b[1], b[0], b[3]]
                    if c["flip_v"]:
                        art, b = art[::-1], [b[0], b[3], b[2], b[1]]
                    keep = int((art[..., 3] > 0).sum())
                    if spr_cells.get(key, (None, None, -1))[2] < keep:
                        spr_cells[key] = (art, b, keep)

    # ---- write the pack --------------------------------------------------------
    shutil.rmtree(os.path.join(args.out, "sheets"), ignore_errors=True)
    shutil.rmtree(os.path.join(args.out, "layers"), ignore_errors=True)
    os.makedirs(os.path.join(args.out, "sheets"), exist_ok=True)
    out = {"format": "z2rs-hdpack", "version": 1, "name": args.name, "scale": scale, "sheets": []}
    pages = collections.defaultdict(dict)
    for (page, tile, colors), (art, _) in bg_cells.items():
        pages[(page, colors)][tile] = art
    for (page, colors), tiles in sorted(pages.items()):
        sheet = np.zeros((128 * scale, 128 * scale, 4), np.uint8)
        for tile, art in tiles.items():
            cx, cy = (tile % 16) * cell_px, (tile // 16) * cell_px
            sheet[cy:cy + cell_px, cx:cx + cell_px] = art
        name = "sheets/page{:02}.{}.png".format(page, colour_tag(colors))
        Image.fromarray(sheet, "RGBA").save(os.path.join(args.out, name))
        out["sheets"].append({"file": name, "page": page, "colors": list(colors)})

    tiles_out = []
    if spr_cells:
        slot_px = (8 + 2 * MAX_BLEED) * scale
        core0 = MAX_BLEED * scale
        # The loader caps a sheet at 4096x4096: fill rows to that width and
        # start another sheet when one is full.
        per_row = MAX_SHEET_PX // slot_px
        per_sheet = per_row * per_row
        items = sorted(spr_cells.items())
        for first in range(0, len(items), per_sheet):
            chunk = items[first:first + per_sheet]
            sheet = np.zeros((-(-len(chunk) // per_row) * slot_px,
                              min(len(chunk), per_row) * slot_px, 4), np.uint8)
            name = "sheets/sprites.png" if first == 0 else \
                "sheets/sprites{}.png".format(first // per_sheet)
            for i, ((page, tile, colors), (art, b, _)) in enumerate(chunk):
                ox, oy = (i % per_row) * slot_px, (i // per_row) * slot_px
                sheet[oy:oy + slot_px, ox:ox + slot_px] = art
                entry = {"page": page, "tile": tile, "sheet": name,
                         "x": ox + core0, "y": oy + core0, "colors": list(colors)}
                if any(b):
                    entry["bleed"] = b
                tiles_out.append(entry)
            Image.fromarray(sheet, "RGBA").save(os.path.join(args.out, name))
            out["sheets"].append({"file": name, "layout": "free"})
        out["sprite_alpha"] = "art"

    # The ROM repeats sprite tiles across CHR pages; a cell must follow.
    if args.propagate != "none":
        twins = collections.defaultdict(list)
        for page in range(len(chr_rom) // 4096):
            for tile in range(256):
                raw = tile_bytes(chr_rom, page, tile)
                if any(raw):
                    twins[raw].append((page, tile))
        have = {(t["page"], t["tile"], tuple(t["colors"])) for t in tiles_out}
        have |= set(bg_cells)
        extra = []
        source = list(spr_cells) if args.propagate == "sprites" else list(spr_cells) + list(bg_cells)
        for key in source:
            page, tile, colors = key
            base = next((t for t in tiles_out
                         if (t["page"], t["tile"], tuple(t["colors"])) == key), None)
            for page2, tile2 in twins.get(tile_bytes(chr_rom, page, tile), ()):
                if (page2, tile2, colors) in have:
                    continue
                have.add((page2, tile2, colors))
                if base is not None:
                    e = dict(base, page=page2, tile=tile2)
                else:
                    src = next(s for s in out["sheets"]
                               if s.get("page") == page and s.get("colors") == list(colors))
                    e = {"page": page2, "tile": tile2, "sheet": src["file"],
                         "x": (tile % 16) * cell_px, "y": (tile // 16) * cell_px,
                         "colors": list(colors)}
                extra.append(e)
        tiles_out += extra
        report.append(f"note {len(extra)} cells repeated on other CHR pages with the same bytes")
    if tiles_out:
        out["tiles"] = tiles_out

    for spec in args.layer:
        fields = dict(part.partition("=")[::2] for part in spec.split(","))
        if "file" not in fields:
            sys.exit(f"--layer {spec}: needs file=PATH")
        os.makedirs(os.path.join(args.out, "layers"), exist_ok=True)
        rel = "layers/" + os.path.basename(fields["file"])
        shutil.copyfile(fields["file"], os.path.join(args.out, rel))
        entry = {"file": rel, "x": int(fields.get("x", 0)), "y": int(fields.get("y", 0))}
        if fields.get("depth"):
            entry["depth"] = fields["depth"]
        if fields.get("scroll"):
            entry["scroll"] = int(fields["scroll"])
        if "repeat_x" in fields:
            entry["repeat_x"] = True
        if fields.get("over"):
            entry["over_tiles"] = [{"page": int(p), "tile": int(t)}
                                   for p, t in (q.split(":") for q in fields["over"].split("+"))]
        if fields.get("when"):
            w, r, s = (int(v) for v in fields["when"].split(":"))
            entry["when"] = {"world": w, "region": r, "scene": s}
        out.setdefault("layers", []).append(entry)

    with open(os.path.join(args.out, "pack.json"), "w") as handle:
        json.dump(out, handle, indent=1)
        handle.write("\n")
    with open(os.path.join(args.out, "cut-report.txt"), "w") as handle:
        handle.write("\n".join(report) + "\n")
    print(f"{args.out}: {painted_slots} painted slot(s) ({skipped} left untouched) -> "
          f"{len(bg_cells)} background + {len(spr_cells)} sprite cells, "
          f"{len(tiles_out)} tiles[] entries")


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawTextHelpFormatter)
    sub = ap.add_subparsers(dest="cmd", required=True)
    b = sub.add_parser("build", help="write editable sheets from captures")
    b.add_argument("--shots", nargs="+", required=True)
    b.add_argument("--out", required=True)
    b.add_argument("--rom", default=os.environ.get("Z2_ROM"))
    b.add_argument("--scale", type=int, default=4)
    b.add_argument("--bleed", type=int, default=MAX_BLEED)
    b.add_argument("--max-figure", type=int, default=64)
    b.add_argument("--name", action="append", default=[], metavar="TAG=NAME")
    b.set_defaults(func=build)
    c = sub.add_parser("cut", help="slice painted sheets into a pack")
    c.add_argument("--sheets", required=True)
    c.add_argument("--out", required=True)
    c.add_argument("--rom", default=os.environ.get("Z2_ROM"))
    c.add_argument("--name", default="Paint-over pack")
    c.add_argument("--layer", action="append", default=[])
    c.add_argument("--propagate", choices=("sprites", "all", "none"), default="sprites")
    c.set_defaults(func=cut)
    args = ap.parse_args()
    if not args.rom:
        sys.exit("--rom or $Z2_ROM")
    if args.cmd == "build" and in_git_worktree(args.out):
        sys.exit(f"{args.out}: inside a git work tree; sheets drawn from ROM CHR belong "
                 "outside the repository (LEGAL.md)")
    args.func(args)


if __name__ == "__main__":
    main()
