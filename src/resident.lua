-- resident — how a resident is written: the three tenses, the clauses it listens with, the work
-- it asks for, the files it writes, and the languages it speaks. Built into town and loaded
-- before any of a town's own files, so every town starts from the same words.

-- the three tenses, one constructor each, so no resident hand-types a `tense` string (a typo'd
-- tense is no word). intent = future, fact = present, event = past.
function intent(kind, body) return { kind = kind, tense = "future",  body = body } end
function fact(kind, body)   return { kind = kind, tense = "present", body = body } end
function event(kind, body)  return { kind = kind, tense = "past",    body = body } end

-- languages: how bytes are read as a body's parts, and parts written as bytes, one grammar run both
-- ways, over a stream: what has not all come yet is waited for. In a `seq` or a `choice`, a string
-- is those bytes, and `{ name = g }` names what g reads. A seq is the parts named within it; with
-- none, what its one other piece reads. A string named is kept. A language is said as a word, a
-- past `language` { name, grammar }, which `language(name, grammar)` says; `parts(name, bytes)`
-- reads with one, and `bytes(name, parts)` writes, as does `as` on the files.
-- each piece is marked as one, so `{ name = g }` is a part whatever its name: `text`, `map`, `int`
local piece = {}
local function made(t) return setmetatable(t, piece) end
local function item(g)
  if type(g) == "string" then return made{ lit = g } end
  if getmetatable(g) == piece then return g end
  if type(g) == "table" then
    local name, is = next(g)
    if name ~= nil and next(g, name) == nil then return made{ part = { name, item(is) } } end
  end
  error("not a grammar: a string, a piece, or { name = piece }", 3)
end
local function items(gs)
  local out = {}; for i, g in ipairs(gs) do out[i] = item(g) end; return out
end
local function counted(g, n) if n then return { item(g), n } end; return item(g) end
function language(name, grammar)
  talk(event("language", { name = name, grammar = item(grammar) }))
end
function upto(s)      return made{ upto = s } end             -- text up to s, which is passed over
function but(s)       return made{ but = s } end              -- text of a byte or more, none in s
function seq(gs)      return made{ seq = items(gs) } end      -- each in turn
function choice(gs)   return made{ choice = items(gs) } end   -- the first that reads
function many(g, n)   return made{ many = counted(g, n) } end -- as many as read, or n, in order
function map(g, n)    return made{ map = counted(g, n) } end  -- the same, by `key`: each a `value`
function unless(g)    return made{ unless = item(g) } end     -- nothing, where g would not read
-- a whole number: { size = bytes, signed, order = "big"|"little", from, to, base }, from and to
-- bounding what the bytes hold, and read as that less base
function int(o)       return made{ int = o or {} } end
function float(o)     return made{ float = o or {} } end      -- { size = 2|4|8, order }
-- a count, n, is a number; a piece that reads one; or the name of a part read before it in the
-- same seq, which is then not kept, and is written as the length of what it counts
function text(n)      return made{ text = n } end             -- text of n bytes
function raw(n)       return made{ raw = n } end              -- n bytes; written, a string too
function whole(g)     return made{ whole = item(g) } end      -- what g reads in pieces, as one
-- what g reads, as value: nil for nothing
function be(g, value) return made{ be = { item(g), value } } end
function ref(name)    return made{ ref = name } end           -- the language of that name

-- words: a word on a line, as a shell says one: `[tense] kind key=value …`, a tab between each, its
-- tense present if it is not said. The one language town speaks itself but JSON, for its CLI. What
-- a tab or a line's end is in cannot be said in it.
local pair = seq{ "\t", { key = but("=\t\n") }, "=", { value = choice{ but("\t\n"), "" } } }
language("words", seq{
  choice{ seq{ { tense = choice{ "past", "present", "future" } }, "\t" }, "" },
  { kind = but("\t\n") },
  { body = choice{ map(pair), "" } },
  "\n",
})

-- files a resident keeps: read a whole file (nil if absent), post one so a reader sees the old or
-- the new and never neither, and mend a missing one from make(). With `as`, a file is the parts of
-- that language, not text.
function read(path, as)
  local f = io.open(path, "rb"); if not f then return nil end
  local s = f:read("*a"); f:close()
  if as then return parts(as, s) end
  return s
end
function post(path, body, as)
  if as then body = bytes(as, body) end
  local aside = path .. ".writing"
  local f = io.open(aside, "wb"); if not f then return end
  f:write(body); f:close()
  os.rename(aside, path)
end
function mend(path, make, as)
  if not read(path) then local t = make(); if t then post(path, t, as) end end
end

-- a string as one shell word. A quote inside it would close ours, so it is doubled the way sh
-- wants: close, escape one, reopen.
function quote(s) return "'" .. tostring(s):gsub("'", "'\\''") .. "'" end

-- work: a command run outside the town, without blocking it — or, with `join`, a line written to a
-- joined program. It comes back as a past `work` carrying `code`, `out`, `ms`, and whatever fields
-- were sent with it — match on those with `on`'s `by`. `where` is the line that asked, for timing.
local acting   -- the clause react is running

-- where work was asked: the line that called it, or, when `return work(…)` (a tail call) took that
-- line away, the clause that was running
local function asked()
  if not debug then return "?" end   -- absent under the test sandbox
  local i, line = debug.getinfo(3, "Sl"), nil
  if debug.getinfo(2, "t").istailcall and acting then
    i = debug.getinfo(acting, "S"); line = i.linedefined
  end
  local s = i and i.short_src
  local src = s and (s:match('%[string "(.-)"%]') or s:match("([^/]+)$") or s)
  return src and (src .. ":" .. (line or i.currentline)) or "?"
end

function work(cmd, body)
  local b = {}   -- a copy: the caller's table may be said again
  for k, v in pairs(body or {}) do b[k] = v end
  b.cmd = cmd
  b.where = asked()
  return intent("work", b)
end

-- after: a wait that blocks nothing. Once `ms` have gone, a past `after` comes back carrying what
-- was sent with it — match on that with `on`'s `by`, as for `work`. A wait is the town's: it is
-- answered across a save, not across town itself starting again.
function after(ms, body)
  if type(ms) ~= "number" or ms ~= ms or ms < 0 or ms == math.huge then   -- nan is not itself
    error("after: ms must be a number, 0 or more", 2)   -- the asker's line, the asker's error
  end
  local b = {}   -- a copy: the caller's table may be said again
  for k, v in pairs(body or {}) do b[k] = v end
  b.ms = ms
  return intent("after", b)
end

-- join: a program town keeps talking to, as `kind`, the way `watch` is a file it keeps reading.
-- As it starts, a bodiless past word of `kind` says so; then each line it says is a past word of
-- `kind`, and each future word of `kind` is a line written to it. When it ends, a past `join` comes
-- back carrying `code` and what was sent with it: to join it again is the resident's to say.
-- Joining a kind again ends the one before; the town opening again ends it too, and a resident
-- joins again as it opens.
function join(kind, argv, body)
  local b = {}   -- a copy: the caller's table may be said again
  for k, v in pairs(body or {}) do b[k] = v end
  b.as, b.argv = kind, argv
  return intent("join", b)
end

-- connect: a socket town keeps talking to, as `kind`, the way `join` is a program: `to` a path, or
-- a host:port. `speaks` in the body names the language it speaks, for a join too: what it says is
-- read in it, as parts, and a future word's body is written in it. It ends as a past `connect`.
function connect(kind, to, body)
  local b = {}
  for k, v in pairs(body or {}) do b[k] = v end
  b.as, b.to = kind, to
  return intent("connect", b)
end

-- on: a clause — match a word by kind, tense, and what it goes by; run act(w). `by = "id"` is a
-- word whose body has that field; `by = { colour = "red" }` a word whose fields are these (`true`:
-- any value).
--   on("note", fn)  ·  on({ kind = "note", tense = "future", by = "id" }, fn)
function on(what, act)
  if type(what) == "string" then what = { kind = what } end
  local by = what.by
  return {
    kind = what.kind,
    match = function(w)
      if what.kind and w.kind ~= what.kind then return false end
      if what.tense and (w.tense or "present") ~= what.tense then return false end
      if by then
        local b = w.body
        if type(b) ~= "table" then return false end
        if type(by) == "string" then return b[by] ~= nil end
        for k, v in pairs(by) do
          if v == true then if b[k] == nil then return false end
          elseif b[k] ~= v then return false end
        end
      end
      return true
    end,
    act = act,
  }
end

-- react: a resident from `on` clauses. Its `kinds` are the clauses' kinds, or every kind ("*") if
-- a clause names none, and `error` too if one names it; its `listen` runs the first matching
-- clause. Add `watch` (or other fields) to the returned table after, if needed.
function react(list)
  local kinds, seen, all = {}, {}, false
  for _, c in ipairs(list) do
    if not c.kind then all = true
    elseif not seen[c.kind] then seen[c.kind] = true; kinds[#kinds + 1] = c.kind end
  end
  if all then kinds = seen.error and { "*", "error" } or { "*" } end
  return {
    kinds = kinds,
    listen = function(w)
      for _, c in ipairs(list) do
        if c.match(w) then
          acting = c.act
          local ok, said = pcall(c.act, w)
          acting = nil
          if not ok then error(said, 0) end
          return said
        end
      end
    end,
  }
end
