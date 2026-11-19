#!/usr/bin/env python3
"""Inspect a .wasm binary: custom sections, imports, exports, memory.
Usage: python inspect_wasm.py <file.wasm> [--dump-metadata]
Zero-dependency; equivalent of `wasm-tools print` for the checks we need.
"""
import json
import struct
import sys


def read_u32(data, pos):
    result = 0
    shift = 0
    while True:
        b = data[pos]
        pos += 1
        result |= (b & 0x7F) << shift
        if not (b & 0x80):
            return result, pos
        shift += 7


def read_name(data, pos):
    length, pos = read_u32(data, pos)
    s = data[pos:pos + length].decode("utf-8", "replace")
    return s, pos + length


def parse(path):
    with open(path, "rb") as f:
        data = f.read()
    assert data[:4] == b"\x00asm", "not a wasm binary"
    pos = 8
    custom, imports, exports, mems = [], [], [], []
    while pos < len(data):
        sec_id = data[pos]
        pos += 1
        sec_len, pos = read_u32(data, pos)
        sec_end = pos + sec_len
        if sec_id == 0:  # custom
            name, _ = read_name(data, pos)
            payload = data[pos + (sec_end - pos) - (sec_end - pos):]  # placeholder
            # locate payload: after the name
            _, name_end = read_name(data, pos)
            payload = data[name_end:sec_end]
            custom.append((name, payload))
        elif sec_id == 2:  # import
            count, p = read_u32(data, pos)
            for _ in range(count):
                mod, p = read_name(data, p)
                name, p = read_name(data, p)
                kind = data[p]; p += 1
                # skip descriptor (kind-specific)
                if kind == 0:  # func: typeidx
                    _, p = read_u32(data, p)
                elif kind == 1:  # table: reftype + limits
                    p += 1
                    flags = data[p]; p += 1
                    _, p = read_u32(data, p)
                    if flags & 1:
                        _, p = read_u32(data, p)
                elif kind == 2:  # memory: limits
                    flags = data[p]; p += 1
                    _, p = read_u32(data, p)
                    if flags & 1:
                        _, p = read_u32(data, p)
                elif kind == 3:  # global: valtype + mut
                    p += 2
                elif kind == 4:  # tag
                    p += 1
                    _, p = read_u32(data, p)
                imports.append((mod, name, kind))
        elif sec_id == 7:  # export
            count, p = read_u32(data, pos)
            for _ in range(count):
                name, p = read_name(data, p)
                kind = data[p]; p += 1
                _, p = read_u32(data, p)
                exports.append((name, {0: "func", 1: "table", 2: "memory", 3: "global"}.get(kind, str(kind))))
        elif sec_id == 5:  # memory
            count, p = read_u32(data, pos)
            for _ in range(count):
                flags = data[p]; p += 1
                minv, p = read_u32(data, p)
                maxv = None
                if flags & 1:
                    maxv, p = read_u32(data, p)
                mems.append((minv, maxv))
        pos = sec_end
    return custom, imports, exports, mems


KIND_NAMES = {0: "func", 1: "table", 2: "mem", 3: "global", 4: "tag"}


def main():
    path = sys.argv[1]
    dump_meta = "--dump-metadata" in sys.argv
    custom, imports, exports, mems = parse(path)

    print(f"== {path}")
    print(f"-- memory: {mems}")
    print(f"-- imports ({len(imports)}):")
    mods = {}
    for mod, name, kind in imports:
        mods.setdefault(mod, []).append(f"{name}:{KIND_NAMES.get(kind, kind)}")
    for mod, names in mods.items():
        print(f"   [{mod}] {', '.join(names)}")
    print(f"-- exports ({len(exports)}):")
    print("   " + ", ".join(f"{n}({k})" for n, k in exports))
    print(f"-- custom sections ({len(custom)}):")
    for name, payload in custom:
        note = ""
        if name == "grain.metadata":
            try:
                parsed = json.loads(payload.decode("utf-8"))
                note = f"  json={json.dumps(parsed, ensure_ascii=False)}"
            except Exception as e:  # noqa: BLE001
                note = f"  !! unparseable: {e}"
        preview = payload.decode("utf-8", "replace") if dump_meta and name == "grain.metadata" else ""
        print(f"   {name} ({len(payload)} bytes){note}")
        if preview:
            print(f"   raw: {preview}")


if __name__ == "__main__":
    main()
