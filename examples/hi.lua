-- hi: a resident made of clauses. `on` hears a kind of word, and `react` answers with the
-- first clause that matches. `talk` says something more besides, and `after` asks town to wait:
-- its answer comes back when the time has gone, carrying what was sent.
--
--   town talk hello from=me     a past `hi` back, the time asked of clock.lua, and a `bye` a
--                               moment later

return react{
  on("hello", function(w)
    talk(intent("time"))
    talk(after(500, { about = "bye", to = w.body.from }))
    return event("hi", { to = w.body.from })
  end),

  on({ kind = "after", tense = "past", by = { about = "bye" } }, function(w)
    return event("bye", { to = w.body.to })
  end),
}
