-- clock: ask it the time, and it runs `date` and tells you.
--
-- `work` runs a program, `date` here, without making the town wait. The result comes back later as
-- a past `town/work`, with whatever you sent along, and `by` picks out clock's own. Each time is
-- kept as a fact: `present` gives the last one, `past` them all. If the question has an
-- `id`, the answer goes back as a past `time` with that `id`.
--
--   printf 'future\ttime\n' | town '{"speaks":"words"}'        a new `clock` fact
--   town '{"kinds":["time"]}'                               held open: the answer to a `time`
--                                                           with an `id`, as a past `time`

return react{
  on({ kind = "time", tense = "future" }, function(w)
    return work({ "date", "+%H:%M:%S" }, { about = "clock", id = (w.body or {}).id })
  end),

  on({ kind = "town/work", tense = "past", by = { about = "clock" } }, function(w)
    if w.body.id then talk(event("time", { id = w.body.id, now = w.body.out })) end
    local before = present("clock") or {}
    return fact("clock", { now = w.body.out, before = before.now, times = #past("clock") + 1 })
  end),
}
