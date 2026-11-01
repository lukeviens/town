-- resident — how a resident is written: the three tenses, the clauses it listens with, the work
-- it asks for, and the files it writes. Compiled into the VM and loaded before any town's own
-- lib.lua, so every town starts from the same grammar.

-- the three tenses, one constructor each, so no resident hand-types a `tense` string (a typo'd
-- tense silently deserializes to nothing). intent = future, fact = present, event = past.
function intent(kind, body) return { kind = kind, tense = "future",  body = body } end
function fact(kind, body)   return { kind = kind, tense = "present", body = body } end
function event(kind, body)  return { kind = kind, tense = "past",    body = body } end

-- files a resident keeps: read a whole file (nil if absent), emit one so a reader sees the old or
-- the new and never neither, and heal a missing one from make().
function read(path)
  local f = io.open(path, "r"); if not f then return nil end
  local s = f:read("*a"); f:close(); return s
end
function emit(path, text)
  local aside = path .. ".writing"
  local f = io.open(aside, "w"); if not f then return end
  f:write(text); f:close()
  os.rename(aside, path)
end
function heal(path, make)
  if not read(path) then local t = make(); if t then emit(path, t) end end
end

-- a string as one shell word. A quote inside it would close ours, so it is doubled the way sh
-- wants: close, escape one, reopen.
function quote(s) return "'" .. tostring(s):gsub("'", "'\\''") .. "'" end

-- work: a command run outside the town, without blocking it — or, with `to`, a line written to a
-- joined program. It comes back as a past `work` carrying `code`, `out`, `ms`, and whatever fields
-- were sent with it — match on those with `on`'s `by`. `where` is the line that asked, for timing.
function work(cmd, body)
  local b = body or {}
  b.cmd = cmd
  local i = debug and debug.getinfo(2, "Sl")   -- absent under the test sandbox
  local src = i and (i.short_src:match('%[string "(.-)"%]') or i.short_src:match("([^/]+)$") or i.short_src)
  b.where = src and (src .. ":" .. i.currentline) or "?"
  return intent("work", b)
end

-- on: a clause — match a word by kind, tense, and what it goes by; run act(w). `by = "id"` is a
-- word whose body has that field; `by = { colour = "red" }` a word whose fields are these (`true`:
-- any value).
--   on("note", fn)  ·  on({ kind = "note", tense = "future", by = "id" }, fn)
function on(spec, act)
  if type(spec) == "string" then spec = { kind = spec } end
  local by = spec.by
  return {
    kind = spec.kind,
    match = function(w)
      if spec.kind and w.kind ~= spec.kind then return false end
      if spec.tense and (w.tense or "present") ~= spec.tense then return false end
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

-- react: a resident from `on` clauses. Its `kinds` are the clauses' kinds; its `listen` runs the
-- first matching clause. Add `watch` (or other fields) to the returned table after, if needed.
function react(list)
  local kinds, seen = {}, {}
  for _, c in ipairs(list) do
    if c.kind and not seen[c.kind] then seen[c.kind] = true; kinds[#kinds + 1] = c.kind end
  end
  return {
    kinds = kinds,
    listen = function(w)
      for _, c in ipairs(list) do if c.match(w) then return c.act(w) end end
    end,
  }
end
