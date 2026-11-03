-- cbor — a language a town may speak, written in the grammar of town's resident.lua. Named
-- among a town's programs, before the residents that speak it; a later one of the name takes its
-- place.

-- RFC 8949: each value in its smallest form, of definite length; one of indefinite length,
-- in pieces till a break, is read too. A tag is { tag, value }, read and written; undefined is
-- read as nothing.
local item = ref("cbor")
-- a number in a byte's low bits
local function low(from, to) return int{ from = from, to = to, base = from } end
local function arg(major, negative)   -- the number a head of this major type says
  local b = major * 32
  local n = negative and function(size) return int{ size = size, base = -1, negative = true } end
                     or function(size) return int{ size = size } end
  return choice{
    negative and int{ from = b, to = b + 23, base = b - 1, negative = true } or low(b, b + 23),
    seq{ string.char(b + 24), n(1) }, seq{ string.char(b + 25), n(2) },
    seq{ string.char(b + 26), n(4) }, seq{ string.char(b + 27), n(8) },
  }
end
language("cbor", choice{
  arg(0), arg(1, true),
  be("\xf4", false), be("\xf5", true), be("\xf6", nil), be("\xf7", nil),
  seq{ "\xf9", float{ size = 2 } }, seq{ "\xfa", float{ size = 4 } },
  seq{ "\xfb", float{ size = 8 } },
  text(arg(3)), raw(arg(2)),   -- text first: written, text would be bytes too
  many(item, arg(4)),
  seq{ { tag = arg(6) }, { value = item } },   -- before a map: written, { tag, value } is a tag
  map(seq{ { key = item }, { value = item } }, arg(5)),
  seq{ "\x5f", whole(many(seq{ unless("\xff"), raw(arg(2)) })), "\xff" },
  seq{ "\x7f", whole(many(seq{ unless("\xff"), text(arg(3)) })), "\xff" },
  seq{ "\x9f", many(seq{ unless("\xff"), item }), "\xff" },
  seq{ "\xbf", map(seq{ unless("\xff"), { key = item }, { value = item } }), "\xff" },
})
