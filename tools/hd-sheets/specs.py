#!/usr/bin/env python3
"""Write spec files for `crates/z2-native/examples/article_shots.rs`.

    python3 tools/hd-sheets/specs.py OUTDIR

The specs are plain text, but the frames they produce are derived from the
user's ROM, so keep OUTDIR next to those frames, outside the repository
(LEGAL.md). Each line loads a scene through the game's own loader: it pokes
the region, world, key-area and scene registers and sets the game mode to 0,
so the game runs its normal bank switch, area, enemy and palette load.

Specs written (run each against the named movie):

    scenes.spec   anypct.bk2           bosses, palaces, towns, areas, ending
    special.spec  anypct.bk2           movie grafts, overworld, width variants,
                                       edge-clip and dialogue limits
    hd.spec       anypct.bk2           the frames used for HD pack before/after
    sheets.spec   anypct.bk2           every town, palace, boss and field with a
                                       full move set, 115 captures each: the
                                       coverage `make hd-sheets` builds from
    m100.spec     hundred-percent.bk2  title, name entry, game over
    title.spec    (no movie)           title and intro scroll
    bug.spec      anypct.bk2 --chrbug  the short-header black overworld
"""

import os
import re
import sys

SETTLE = 150  # frames for a scene load to finish before anyone moves


def inventory(stage):
    """RAM pokes for a fresh (0), mid-game (1) or end-game (2) Link."""
    if stage == 0:
        return []
    if stage == 1:
        return ["0777:04", "0778:04", "0779:04", "0783:06", "0784:06", "0773:C0", "0774:C0",
                "0785:01", "0786:01", "0787:01", "0788:01",
                "077B:01", "077C:01", "077D:01", "077E:01", "0796:14", "0793:02"]
    return ["0777:08", "0778:08", "0779:08", "0783:08", "0784:08", "0773:FF", "0774:FF",
            "0785:01", "0786:01", "0787:01", "0788:01", "0789:01", "078A:01", "078B:01", "078C:01",
            "077B:01", "077C:01", "077D:01", "077E:01", "077F:01", "0780:01", "0781:01", "0782:01",
            "0796:14", "0793:03"]


def load(region, world, idx=0, scene=None, page=0, town=0, palace=0, stage=0, extra=()):
    """`poke=` option that makes the game's own loader enter a scene."""
    pokes = [f"0706:{region:02X}", f"0707:{world:02X}", f"0748:{idx:02X}",
             f"056B:{town:02X}", f"056C:{palace:02X}", f"075C:{page:02X}", "0701:00"]
    if scene is not None:
        pokes.append(f"0561:{scene:02X}")
    pokes += inventory(stage) + list(extra) + ["0736:00"]
    return "poke=" + "/".join(pokes)


def town(code, scene, stage=0):
    region, world = (0, 1) if code < 4 else (2, 2)
    return load(region, world, 0x2C + 2 * (code % 4), scene, town=code, stage=stage)


# palace number -> (region, world, palace code, key-area index); 7 = Great Palace
PALACES = {1: (0, 3, 0, 0x34), 2: (0, 3, 1, 0x35), 3: (0, 4, 2, 0x36), 4: (1, 4, 0, 0x34),
           5: (2, 3, 0, 0x34), 6: (2, 4, 1, 0x35), 7: (2, 5, 2, 0x36)}


def palace(number, scene, page=0, stage=1):
    region, world, code, idx = PALACES[number]
    return load(region, world, idx, scene, page, palace=code, stage=stage)


def area(region, idx, stage=0):
    """World-0 key area (cave, bridge, field): the scene comes from `$0748`."""
    return load(region, 0, idx, stage=stage, extra=["0785:01"])


def overworld(region, idx):
    return f"poke=0706:{region:02X}/0707:00/0709:01/0748:{idx:02X}/0736:00"


def line(name, run, p1, p2, *opts, graft=600):
    return f"{name} {graft} {run} {p1} {p2} " + ",".join(o for o in opts if o)


def frames(script):
    return sum(int(re.search(r"(\d+)$", seg).group(1)) for seg in script.split(","))


def scenes():
    out = []
    w = SETTLE

    def fight(name, opt, walk, dx=56, burst="burst=10x6"):
        p1 = f"_{w},R{walk},_4,B3,_10,B3,_10,DB3,_14,B3,_8"
        p2 = f"_{w},_8,R{walk - 20},_6,A14,RA6,B3,_12,B3,_10,A10,B3,_8"
        out.append(line(name, w + walk + 66, p1, p2, opt, burst, f"dx={dx}"))

    def stroll(name, opt, walk=120):
        p1 = f"_{w},R{walk},_30,L20,_40,R30,_30"
        p2 = f"_{w},_25,R{walk + 40},_20,L30,_30,R10,_20"
        out.append(line(name, w + walk + 150, p1, p2, opt, "burst=8x12", "dx=48"))

    fight("boss1_horsehead", palace(1, 13, stage=0), 290)
    fight("boss2_helmethead", palace(2, 34), 290)
    fight("boss3_rebonack", palace(3, 14, page=1), 190, burst="burst=8x9,both")
    fight("boss4_carock", palace(4, 28), 290)
    fight("boss5_gooma", palace(5, 41, stage=2), 290)
    fight("boss7_thunderbird", palace(7, 53, page=1, stage=2), 150)
    out.append(line("boss8_darklink", w + 560, f"_{w},R230,_250,B3,_10,DB3,_10,B3,_51",
                    f"_{w},R200,_280,A12,B3,_10,B3,_55", palace(7, 54, page=1, stage=2),
                    "burst=12x8", "dx=40"))
    fight("pal1_ironknuckle", palace(1, 4, stage=0), 170)
    fight("pal1_entrance", palace(1, 0, stage=0), 100)
    fight("pal2_entrance", palace(2, 14), 100)
    fight("pal5_entrance", palace(5, 35, stage=2), 100)
    fight("pal7_entrance", palace(7, 0, stage=2), 100)
    fight("pal3_room", palace(3, 5), 170)
    fight("pal6_room", palace(6, 44, stage=2), 170)
    fight("pal7_room", palace(7, 20, stage=2), 170)
    for name, code, scene, stage in (("rauru", 0, 1, 0), ("ruto", 1, 4, 0), ("saria", 2, 7, 1),
                                     ("mido", 3, 10, 1), ("nabooru", 4, 13, 1),
                                     ("darunia", 5, 16, 2), ("newkasuto", 6, 19, 2),
                                     ("oldkasuto", 7, 22, 2)):
        stroll(f"town_{name}", town(code, scene, stage))
    stroll("town_rauru_west", town(0, 0), walk=40)
    fight("area_cave", area(0, 0x0A), 170)
    fight("area_cave2", area(0, 0x0F), 170)
    fight("area_bridge", area(0, 0x14), 170)
    fight("area_forest", area(0, 0x1B), 120)
    fight("area_desert", area(0, 0x1F), 120)
    fight("area_graveyard", area(0, 0x05), 120)
    fight("area_dm_cave", area(1, 0x04, stage=1), 170)
    fight("area_east_field", area(2, 0x00, stage=2), 150)
    out.append(line("start_northpalace", 150, "_150", "_30,R44,_10,L2,_64", "burst=5x20", graft=300))
    # The real ending: Dark Link's HP is set to 1 once he is up, player 1 crouch-stabs.
    stabs = ",".join(["DB3,D5"] * 40)
    hp = "/".join(f"@{w + 520}:{a:04X}:01" for a in range(0xC2, 0xC8))
    out.append(line("ending", w + 3240, f"_{w},R230,_290,{stabs},_2400",
                    f"_{w},R200,_320,{stabs},_2400", palace(7, 54, page=1, stage=2) + "/" + hp,
                    "burst=60x45", "dx=40"))
    return out


def special():
    out = []
    w = SETTLE
    for name, graft, delay in (("tas_rauru", 1300, 25), ("tas_rauru2", 1900, 25),
                               ("tas_bridge", 3650, 20), ("tas_dmcave", 6200, 20)):
        out.append(line(name, 200, "t", f"m{delay}", "burst=4x20", "both", graft=graft))
    for name, graft in (("tas_pause_glitch", 17400), ("tas_gameover", 20100),
                        ("tas_continue", 20400)):
        out.append(line(name, 0, "-", "-", "solo", graft=graft))

    def ow(name, region, idx, walk, extra=""):
        out.append(line(name, 240 + frames(walk), f"_240,{walk}", "-", overworld(region, idx),
                        "burst=6x50", "solo", extra))

    ow("ow_start", 0, 0x00, "R48,_60,R48,_60,D32,_60", "v=noclip+native")
    ow("ow_center", 0, 0x14, "_60,L32,_90,U32,_90,R32")
    ow("ow_saria", 0, 0x30, "_60,L32,_90,D32,_90")
    ow("ow_westedge", 0, 0x0B, "_60,L32,_90,L32,_90")
    ow("ow_dm", 1, 0x10, "_60,R32,_90,D32,_90")
    ow("ow_maze", 1, 0x34, "_60,L32,_90,D32,_90")
    ow("ow_east_nabooru", 2, 0x2D, "_60,R32,_90,D32,_90")
    ow("ow_east_darunia", 2, 0x2E, "_60,L32,_90,D32,_90")
    ow("ow_east_p5", 2, 0x34, "_60,L32,_90,U32,_90", "v=noclip")
    p1 = f"_{w},R120,_30,L20,_40,R30,_30"
    p2 = f"_{w},_25,R160,_20,L30,_30,R10,_20"
    out.append(line("cmp_saria", w + 270, p1, p2, town(2, 7, 1), "both", "dx=48",
                    "v=native+w1610+w16"))
    out.append(line("cmp_thunderbird", w + 190, f"_{w},R150,_4,B3,_10,B3,_20",
                    f"_{w},_8,R130,_6,A14,RA6,B3,_20", palace(7, 53, page=1, stage=2), "both",
                    "dx=56", "v=native+w1610+w16"))
    out.append(line("edge_eastfield", w + 900, f"_{w},R100,_800", f"_{w},R80,_820",
                    area(2, 0x00, stage=2), "find=edge", "dx=48"))
    out.append(line("dialog_wiseman", w + 750, f"_{w},R330,_420", f"_{w},_20,R200,_530",
                    town(0, 36), "burst=3x120", "dx=40"))
    return out


def hd():
    w = SETTLE
    p1 = f"_{w},R120,_30,L20,_40,R30,_30"
    p2 = f"_{w},_25,R160,_20,L30,_30,R10,_20"
    return [
        line("hd_rauru", w + 270, p1, p2, town(0, 1), "both", "dx=48"),
        line("hd_thunderbird", w + 190, f"_{w},R150,_4,B3,_10,B3,_20",
             f"_{w},_8,R130,_6,A14,RA6,B3,_20", palace(7, 53, page=1, stage=2), "both", "dx=56"),
        line("hd_overworld", 300, "_240,R48,_12", "-", overworld(0, 0x00), "solo"),
    ]


# Walk, stab, crouch-stab, jump, up- and down-thrust, turn: every pose Link has
# outside magic, plus whatever the scene's enemies and townspeople do meanwhile.
MOVES = ("_150,R40,_6,B4,_12,B4,_12,DB4,_14,D14,DB4,_14,A18,_6,B4,_16,A18,UB4,_18,A18,DB4,"
         "_18,L40,_8,B4,_14,R26,_8,B4,_10,U14,_10,R40,_10,B4,_12,D10,B4,_16,A20,_10,R30,_10,"
         "B4,_20")
P2_MOVES = "_150,_25,R160,_20,L30,_30,R10,_20"


def sheets():
    """Every scene of scenes.spec replayed with MOVES and a dense burst.

    Paint sheets are only as complete as the frames they are built from, so
    this spec trades file count for coverage: 115 captures a scene."""
    out = []
    for spec in scenes():
        name, _, _, _, _, opts = spec.split(" ", 5)
        if not name.startswith(("town_", "pal", "boss", "area_")):
            continue
        poke = next(o for o in opts.split(",") if o.startswith("poke="))
        out.append(line(f"{name}_moves", 460, MOVES, P2_MOVES, poke, "dx=48", "burst=115x4"))
    return out


STATIC = {
    "m100.spec": ["tas100_gameover 206100 0 - - solo", "tas100_title 220000 0 - - solo",
                  "tas100_register 221400 0 - - solo", "tas100_register_end 225900 0 - - solo",
                  "tas100_rauru_crowd 211350 200 t m25 burst=4x20,both"],
    "title.spec": ["title_0900 900 0 - - solo", "title_3300 3300 0 - - solo"],
    "bug.spec": ["bug_palace 300 0 - - solo", "bug_overworld 620 0 - - solo",
                 "bug_overworld2 700 0 - - solo"],
}


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = sys.argv[1]
    os.makedirs(out, exist_ok=True)
    files = {"scenes.spec": scenes(), "special.spec": special(), "hd.spec": hd(),
             "sheets.spec": sheets(), **STATIC}
    for name, lines in files.items():
        with open(os.path.join(out, name), "w") as handle:
            handle.write("\n".join(lines) + "\n")
        print(f"{name}: {len(lines)} shots")


if __name__ == "__main__":
    main()
