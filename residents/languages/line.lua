-- line: a line of text. Read, a past word at the kind joined, its body the line; written, a
-- future word's body.
local either = choice{ be("", "past"), be("", "future") }   -- read as past; written either way
return { languages = { line = seq{ { tense = either }, { body = upto("\n") } } } }
