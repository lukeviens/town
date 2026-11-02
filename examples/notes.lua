-- notes: a file the town keeps. `heal` makes it if it is missing, `emit` writes it whole, and
-- `watch` says it as a fact each time it changes. `noted` is `own`ed: only notes says it.
--
--   town talk past note text="buy milk"     a line in notes.txt, and the file said again

local file = (os.getenv("TMPDIR") or "/tmp/"):gsub("/?$", "/") .. "notes.txt"
heal(file, function() return "" end)

local notes = react{
  on({ kind = "note", tense = "past" }, function(w)
    emit(file, read(file) .. w.body.text .. "\n")
    return event("noted", { text = w.body.text })
  end),
}
notes.watch = { notes = file }
notes.own = { "noted" }
return notes
