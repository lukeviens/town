-- hi: say hello, and it says hi back, then bye a moment later.
--
-- `on` picks out a kind of word, and `react` answers with the first `on` that matches. `talk`
-- says more besides the answer. `after` waits without making the town wait: when the time is up,
-- a past `after` comes back with whatever you sent along.
--
--   town talk hello from=me     a past `hi`, a question to clock.lua, and a `bye` half a second
--                               later

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
