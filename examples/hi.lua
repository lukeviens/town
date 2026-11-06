-- hi: say hello, and it says hi back, then bye a moment later.
--
-- `on` picks out a kind of word, and `react` answers with the first `on` that matches. `talk` says
-- more besides the answer. A wait is a program, `sleep`, that `work` runs without making the town
-- wait: as it ends, a past `town/work` comes back with whatever you sent along.
--
--   printf 'past\thello\tfrom=me\n' | town '{"speaks":"words"}'
--                                  a past `hi`, a question to clock.lua, and a `bye` half a
--                                  second later

return react{
  on("hello", function(w)
    talk(intent("time"))
    talk(work({ "sleep", "0.5" }, { about = "bye", to = w.body.from }))
    return event("hi", { to = w.body.from })
  end),

  on({ kind = "town/work", tense = "past", by = { about = "bye" } }, function(w)
    return event("bye", { to = w.body.to })
  end),
}
