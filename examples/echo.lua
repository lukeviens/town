-- echo: whatever you say, `cat` says back.
--
-- `join` starts a program and keeps talking to it. `cat` speaks `line`: each line it prints
-- comes back as a past word of its kind, here `cat`, and each future `cat` is sent to it as a line.
--
--   printf 'future\tsay\ttext=hi\n' | town '{"speaks":"words"}'
--                                  `cat` says hi back: a past `echoed`

local echo = react{
  on({ kind = "say", tense = "future" }, function(w)
    return intent("cat", w.body.text)
  end),

  on({ kind = "cat", tense = "past" }, function(w)
    return event("echoed", { text = w.body })
  end),
}
talk(join("cat", { "cat" }, { speaks = "line" }))   -- as the town opens
return echo
