-- reload: opens a resident again as it is saved. It joins `mirror` on each path in
-- `town/residents` but one inside another, which that one's mirror says, and asks `town/open` of
-- each file a mirror says, or says is gone. Town opens again only a file that is not the version
-- its opener opened, so an ask of one as it was, as a mirror begins or town lets a fact go, does
-- nothing. So it asks of every file it is told, its greeting too: a file's kind is
-- `reload<its path>`, whichever mirror said it, and one said before it knew its mirrors, or said
-- again unchanged, is still asked of.
local hears = hears   -- town's rule, as it is now: a library after may name its own
local mirrors = {}    -- each mirror it joined, by its kind
local function inside(path, paths)   -- whether another of the paths contains it
  for _, other in ipairs(paths) do
    if other ~= path and hears("/reload" .. other, "reload" .. path) then return true end
  end
  return false
end
return {
  kinds = { "town/residents", "/reload" },
  listen = function(w)
    if w.kind == "town/residents" then
      mirrors = {}
      local paths = w.body or {}
      for _, path in ipairs(paths) do
        local kind = "reload" .. path
        if not inside(path, paths) and not mirrors[kind] then
          mirrors[kind] = true
          talk(join(kind, { "mirror", path }))
        end
      end
      return
    end
    if w.tense == "present" and w.kind:sub(1, #"reload/") == "reload/" then
      return intent("town/open", { path = w.kind:sub(#"reload" + 1) })
    end
  end,
}
