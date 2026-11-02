-- echo: `join` keeps a program running beside the town. Each line it says is a past word of its
-- kind, and `work(line, { join = kind })` hands it one. Here it is `cat`, a perfect parrot.
--
--   town talk future say text=hi     `cat` says hi back: a past `echoed`

local echo = react{
  on({ kind = "say", tense = "future" }, function(w)
    return work(w.body.text, { join = "cat" })
  end),

  on({ kind = "cat", tense = "past" }, function(w)
    if w.body then return event("echoed", { text = w.body }) end   -- bodiless: cat just woke
  end),
}
echo.join = { cat = { "cat" } }
return echo
