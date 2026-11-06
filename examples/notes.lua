-- notes: each note you add becomes a line in a file.
--
-- `mend` makes the file if it is missing, and `post` writes it whole, so no one reads half of it.
-- `mirror`, a program joined by its name, says the file as a fact whenever it changes. `own` keeps
-- `noted` for notes alone: no one else may say it.
--
--   printf 'past\tnote\ttext=buy milk\n' | town '{"speaks":"words"}'
--                                  a line added to notes.txt, and the file said again

local file = (os.getenv("TMPDIR") or "/tmp/"):gsub("/?$", "/") .. "notes.txt"
mend(file, function() return "" end)
talk(join("notes", { "mirror", file }))

local notes = react{
  on({ kind = "note", tense = "past" }, function(w)
    post(file, read(file) .. w.body.text .. "\n")
    return event("noted", { text = w.body.text })
  end),
}
notes.own = { "noted" }
return notes
