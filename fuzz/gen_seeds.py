#!/usr/bin/env python3
"""Builds the synthetic seed inputs of the fuzz corpora under fuzz/corpus/.

Every input is made here from the layouts documented in docs/ and in the module docs of src/; no game
data is involved. Usage: gen_seeds.py <dump dir> where the dump dir holds the byte strings that the unit
tests of the crate build (one file per string, named <module>.<builder>.<hash>); those feed the targets
that need a header built by a test helper. Without a dump dir only the inputs made here are written.
"""
import os
import struct
import sys

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "corpus")


def u32(*v): return b"".join(struct.pack("<I", x) for x in v)
def u16(*v): return b"".join(struct.pack("<H", x) for x in v)
def f32(*v): return b"".join(struct.pack("<f", x) for x in v)
def s32(s): b = s.encode(); return u32(len(b)) + b


def common(sources, attrs, kind):
    h = b"\xab" * 16 + u32(len(sources))
    for s in sources: h += s32(s)
    h += u32(len(attrs.encode())) + attrs.encode()
    if attrs: h += b"\0"
    return h + u32(kind)


def put(target, name, data):
    d = os.path.join(OUT, target)
    os.makedirs(d, exist_ok=True)
    with open(os.path.join(d, name), "wb") as f: f.write(data)


def parts(*ps):
    """Input layout of the targets that split their input: u16 lengths of all parts but the last."""
    return b"".join(u16(len(p)) for p in ps[:-1]) + b"".join(ps)


# --- cache: block headers, LZF -------------------------------------------------------------------
def block(comp, dec):
    return struct.pack(">II", 0x80000000 | comp << 2, dec << 5 | 1)

lzf = bytes([2, 97, 98, 99, 4 << 5, 2])  # "abc" then 6 bytes copied from 3 back
put("lzf", "literals_and_copy", u16(9) + lzf)
put("lzf", "long_copy", u16(13) + bytes([0, 120, 7 << 5, 3, 0]))
put("lzf", "bad_distance", u16(3) + bytes([1 << 5, 5]))
# a 5-byte entry the .toc declares as 3 GB (it used to allocate that much)
put("cache_decompress", "crash-declared-size", u32(3372932899) + bytes([2, 97, 98, 99]))
put("cache_decompress", "stored", u32(5) + b"hello")
put("cache_decompress", "unframed_lzf", u32(9) + lzf)
put("cache_decompress", "framed", u32(13) + block(4, 4) + b"wxyz" + block(len(lzf), 9) + lzf)
put("cache_decompress", "oodle_garbage", u32(64) + block(6, 64) + bytes([0x8C, 6, 255, 255, 255, 255]))

# --- toc ----------------------------------------------------------------------------------------
def toc_entry(offset, comp, ln, parent, name):
    return (struct.pack("<qqiiii", offset, 1234, comp, ln, 0, parent) + name.encode()).ljust(96, b"\0")

toc = struct.pack("<II", 0x1867C64E, 20)
put("toc_parse", "empty", toc)
put("toc_parse", "tree", toc + toc_entry(-1, 0, 0, 0, "Lotus") + toc_entry(0, 10, 10, 1, "a.txt")
    + toc_entry(-1, 0, 0, 1, "Interface") + toc_entry(10, 5, 20, 2, "b.png") + toc_entry(15, 3, 3, 0, "top"))
put("toc_parse", "bad_parent", toc + toc_entry(0, 1, 1, 5, "x"))

# A chain of 70 directories with 60-byte names: its deepest path is over the 4096-byte limit
put("toc_parse", "crash-deep-directory-chain", toc + b"".join(toc_entry(-1, 0, 0, i, "d" * 60) for i in range(70)))

# --- glob ---------------------------------------------------------------------------------------
put("glob", "crash-many-doublestars", parts(b"**a" * 40 + b"b", b"a" * 200))
put("glob", "star", parts(b"/Lotus/*.png", b"/Lotus/a.png"))
put("glob", "doublestar", parts(b"/Lotus/**/a.png", b"/Lotus/x/y/a.png"))
put("glob", "filter", parts(b"/Tenno/Movement/**_anim.fbx|/Bow/", b"/Lotus/Animations/Tenno/Movement/Sword/Jog_anim.fbx"))

# --- text ---------------------------------------------------------------------------------------
attrs = "Mesh=Chair.fbx\nMeshScale=0.5\nAmbientLight={50,50,50,0}\nName=\"a b\"\nNested={{0.1,0.2},{1,2}}\nEnabled=1\n"
put("ee_attributes", "attrs", attrs.encode())
put("ee_attributes", "block", b"Materials={\n    MyMat\n}\nColor={1,0.5,0,1}\n")
put("material_text", "props", parts(b"Diffuse=/Lotus/a.png\nColor={1,0,0,1}\nTile=2.5\nBlock={\n  a=1\n}\n", b"Other.png"))
put("material_text", "paths", parts(b"/Lotus/Deco/", b"../Other/x.png"))

put("entity_text", "chain", parts(b"Mesh=Base.fbx\nAmbientLight={50,50,50,0}\nPostProcessInfo={Bloom={Enabled=1}}\n",
    b"Diffuse=/Lotus/a.png\nTile=2\nShader0=/EE/Shaders/PostFX/A.hlsl\nPS:Sharpen=0.5\nSIMPLE=1\n",
    b"StartTrim=0.1\nTrimEnd=0.9\nEvents={}\nColor={1,0.5,0,1}\n", b"Mesh=Other.fbx\nDimensions={60.5,41.5}\n"))
put("entity_text", "empty", parts(b"", b"", b"", b""))

# --- common header / animation / level / texture ------------------------------------------------
put("common_header", "plain", common(["/Lotus/a.png"], "EnableStreaming=1\n", 0x000100A3))
put("common_header", "no_attrs", common([], "", 184))

anim_h = common(["/Lotus/x.fbx"], "Events={}\n", 132) + s32("") + s32("") + b"\0" * 90 + u32(1)
anim_h += s32("Skel") + u32(0) + f32(0, 0, 0, 0, 1, 0, 0, 0, 0) + u32(2)
anim_h += s32("root") + u16(0, 0) + s32("child") + u16(0, 0)
anim_h += u32(1) + b"\0" * 8 + f32(30.0) + b"\0" * 4 + s32("clip") + b"\0" * 68 + u32(0, 0)
anim_h += b"\0" * 26 + u32(0) + b"\0" * 12 + u32(2) + b"\0" * 8 + u32(0)
anim_b = b"".join(f32(0, 0, 0, 1, 0, 0, 0) + b"\0" * 12 for _ in range(2)) + u16(0, 0) + u16(0, 0) + f32(0.0, 1.0)
put("animation", "one_clip", parts(anim_h, anim_b))

level_h = common(["/Lotus/l.level"], "", 212) + u32(1) + s32("/Lotus/a") + u32(0) + u32(1) + s32("/Lotus/x.png")
level_h += u32(0) + u32(0) + u32(2)
for t, scope in (("/EE/Types/Engine/Light", "/Layer1/"), ("/Lotus/Deco/Chair", "")):
    level_h += s32(t) + f32(1, 2, 3) + f32(0, 0, 0, 1) + s32(scope) + u32(7, 0)
level_h += u32(0) + u32(0)
texts = [b"Intensity=2\n", b"Mesh=Chair.fbx\n"]
level_b = u32(*[len(t) for t in texts]) + b"".join(t + b"\0" for t in texts)
put("level", "two_objects", parts(level_h, level_b))
slevel_h = common(["/Lotus/s.static"], "", 30) + u32(0) + u32(0)
put("level", "static_head", parts(slevel_h, b""))


def texture(fmt, w, h, kind=163, body_mips=1, full_mips=0, extra=()):
    return (common(["/Lotus/t.png"], "", kind) + bytes([body_mips, full_mips, 0, fmt]) + u32(len(extra))
            + u32(*extra) + u16(w, h, w, h))

put("texture", "bc1_8x8", parts(texture(0, 8, 8, body_mips=4), b"\0" * (32 + 8 + 8 + 8), b""))
put("texture", "bgra_4x4", parts(texture(10, 4, 4, body_mips=3), bytes(range(64 + 16 + 4)), b""))
put("texture", "bc7_split", parts(texture(34, 8, 8, body_mips=1, full_mips=1), b"\1" * 16, b"\2" * 64))
put("texture", "probe", parts(texture(10, 2, 2, kind=183, body_mips=1), b"\3" * 24, b""))

# --- bc6h / hdr ---------------------------------------------------------------------------------
put("bc6h", "zero", b"\0" * 16)
put("bc6h", "mode1", bytes([0x01]) + bytes(range(15)))
put("bc6h", "ones", b"\xff" * 16)
put("radiance_hdr", "tiny", b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 2 +X 2\n" + bytes([128, 128, 128, 129] * 4))
put("model_vertex", "zero", b"\0" * 32)
put("model_vertex", "ones", b"\xff" * 32)

# --- composed from what the unit tests build ----------------------------------------------------
if len(sys.argv) > 1:
    d = sys.argv[1]
    dump = {}
    for n in sorted(os.listdir(d)):
        mod, fn, _ = n.split(".")
        dump.setdefault((mod, fn), []).append((n, open(os.path.join(d, n), "rb").read()))
    for n, v in dump.get(("toc", "toc"), []): put("toc_parse", n, v)
    for n, v in dump.get(("header", "build"), []): put("common_header", n, v)
    for n, v in dump.get(("packages", "build_with_buffers"), []): put("packages_bin", n, v)
    for n, v in dump.get(("physx", "mesh_blob"), []) + dump.get(("physx", "convex_blob"), []): put("physx", n, v)
    for n, v in dump.get(("bc6h", "blocks"), []):
        for i in range(0, min(len(v), 160), 16): put("bc6h", f"{n}.{i}", v[i:i + 16])
    for n, v in dump.get(("model", "vertex_bytes"), []) + dump.get(("model", "float_vertex"), []): put("model_vertex", n, v)
    verts = b"".join(v for _, v in dump.get(("model", "vertex_bytes"), [])[:3])
    idx = u16(0, 1, 2)
    for n, h in dump.get(("model", "header_bytes"), []) + dump.get(("model", "skinned_header_bytes"), []) + dump.get(("model", "level_mesh_header"), []):
        body = b"\0\0" + verts + idx
        put("model", n, parts(h, body, b""))
        put("model", n + ".split", parts(h, body[:len(body) // 2], body[len(body) // 2:]))
        put("texture", "m." + n, parts(h, body, b""))  # a wrong-kind header: reaches the type check
    for n, b in dump.get(("model", "skinned_body"), []):
        for hn, h in dump.get(("model", "skinned_header_bytes"), []): put("model", n + ".skinned", parts(h, b, b""))
    for hn, h in dump.get(("animtree", "head"), []):
        put("animtree", hn, parts(h, b""))
        for bn, b in dump.get(("animtree", "edges"), []): put("animtree", hn + "." + bn.split(".")[-1], parts(h, b))
