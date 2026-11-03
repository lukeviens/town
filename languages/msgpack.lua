-- msgpack — a language a town may speak, written in the grammar of town's resident.lua. Named
-- among a town's programs, before the residents that speak it; a later one of the name takes its
-- place.

-- msgpack.org: each value in its smallest form. A number as the bytes say it, a string as
-- text, bin as bytes, an ext as { ext, data }: parts of just those, bytes for data, are an ext.
local value = ref("msgpack")
-- a count in a byte's low bits; in the bytes after b
local function low(from, to) return int{ from = from, to = to, base = from } end
local function after(b, size) return seq{ b, int{ size = size } } end
-- an ext of a fixed size; of the size after b
local kind = { ext = int{ signed = true } }
local function ext(b, n) return seq{ b, kind, { data = raw(n) } } end
local function exts(b, size)
  return seq{ b, { size = int{ size = size } }, kind, { data = raw("size") } }
end
language("msgpack", choice{
  int{ to = 0x7f }, int{ signed = true, from = -32, to = -1 },
  ext("\xd4", 1), ext("\xd5", 2), ext("\xd6", 4), ext("\xd7", 8), ext("\xd8", 16),
  exts("\xc7", 1), exts("\xc8", 2), exts("\xc9", 4),
  be("\xc0", nil), be("\xc2", false), be("\xc3", true),
  after("\xcc", 1), after("\xcd", 2), after("\xce", 4), after("\xcf", 8),
  seq{ "\xd0", int{ size = 1, signed = true } }, seq{ "\xd1", int{ size = 2, signed = true } },
  seq{ "\xd2", int{ size = 4, signed = true } }, seq{ "\xd3", int{ size = 8, signed = true } },
  seq{ "\xca", float{ size = 4 } }, seq{ "\xcb", float{ size = 8 } },
  text(choice{ low(0xa0, 0xbf), after("\xd9", 1), after("\xda", 2), after("\xdb", 4) }),
  raw(choice{ after("\xc4", 1), after("\xc5", 2), after("\xc6", 4) }),
  many(value, choice{ low(0x90, 0x9f), after("\xdc", 2), after("\xdd", 4) }),
  map(seq{ { key = value }, { value = value } },
      choice{ low(0x80, 0x8f), after("\xde", 2), after("\xdf", 4) }),
})
