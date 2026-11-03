-- echo: whatever you say, `cat` says back.
--
-- `join` starts a program and keeps talking to it. Each line the program prints comes back as a
-- past word of its kind, here `cat`; each future `cat` is sent to it as a line.
--
--   town talk future say text=hi     `cat` says hi back: a past `echoed`

local echo = react{
  on({ kind = "say", tense = "future" }, function(w)
    return intent("cat", w.body.text)
  end),

  on({ kind = "cat", tense = "past" }, function(w)
    if w.body then return event("echoed", { text = w.body }) end   -- no body: cat has just started
  end),
}
talk(join("cat", { "cat" }))   -- as the town opens
return echo
