-- words: a word a line, `[tense] kind key=value …`, a tab between each, present if no tense is
-- said. A tab or a line's end cannot be said in it.
local pair = seq{ "\t", { key = but("=\t\n") }, "=", { value = choice{ but("\t\n"), "" } } }
local tense = { tense = choice{ "past", "present", "future" } }
return { languages = { words = seq{
  choice{ seq{ tense, "\t", { kind = choice{ but("\t\n"), "" } } }, { kind = but("\t\n") } },
  { body = choice{ seq{ unless("\n"), map(pair) }, be("", nil) } },
  "\n",
} } }
