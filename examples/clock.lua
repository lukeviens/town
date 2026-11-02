-- clock: `work` runs a command without stopping the town. Its answer comes back as a past
-- `work`, carrying what was sent with it, so `by` knows which answer is clock's. What clock
-- learns, it keeps as a fact: `present` is the last one, `past` all of them lately. Asked, it
-- answers too: a past `time` with the asker's `id`.
--
--   town talk future time       a `clock` fact
--   town ask future time        the time, answered

return react{
  on({ kind = "time", tense = "future" }, function(w)
    return work("date " .. quote("+%H:%M:%S"), { about = "clock", id = (w.body or {}).id })
  end),

  on({ kind = "work", tense = "past", by = { about = "clock" } }, function(w)
    if w.body.id then talk(event("time", { id = w.body.id, now = w.body.out })) end
    local before = present("clock") or {}
    return fact("clock", { now = w.body.out, before = before.now, times = #past("clock") + 1 })
  end),
}
