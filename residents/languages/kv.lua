-- kv: lines of key=value; a line that is not one is passed over.
local pair = seq{ { key = but("=\n") }, "=", { value = upto("\n") } }
return { languages = { kv = map(choice{ pair, upto("\n") }) } }
