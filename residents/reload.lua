-- reload: opens a resident again as it is saved. It joins `town-watch` on each path in
-- `town/residents` but one inside another, which that one's watch holds, and asks `town/open` of
-- each file a watch says changed, or is gone: town opens each once the opens of one save stop, if
-- the file is a resident, or was one. What a watch says as it begins, before its `now`, is every
-- file there: of those, only one changed since reload began, as the town read it, is asked.
local hears = hears   -- town's rule, as it is now: a library after may name its own
local watching = {}   -- each watch, by its kind: whether it has said `now`
local began = (os.time() - 1) * 1e9   -- in ns, as a watch says when a file changed
local function inside(path, paths)   -- whether another of the paths holds it
  for _, other in ipairs(paths) do
    if other ~= path and hears("/reload" .. other, "reload" .. path) then return true end
  end
  return false
end
return {
  kinds = { "town/residents", "/reload" },
  listen = function(w)
    if w.kind == "town/residents" then
      watching = {}
      local paths = w.body or {}
      for _, path in ipairs(paths) do
        local kind = "reload" .. path
        if not inside(path, paths) and watching[kind] == nil then
          watching[kind] = false
          talk(join(kind, { "town-watch", path }))
        end
      end
      return
    end
    for at, now in pairs(watching) do
      if w.kind == at and w.tense == "past" and w.body == nil then
        watching[at] = true   -- its `now`: what it says from here on changed as it watched
        return
      end
      local changed = type(w.body) == "table" and w.body.changed or 0
      if w.tense == "present" and hears("/" .. at, w.kind) and (now or changed >= began) then
        return intent("town/open", { path = w.kind:sub(#"reload" + 1) })
      end
    end
  end,
}
