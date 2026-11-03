-- kv — a language a town may speak, written in the grammar of town's resident.lua. Named
-- among a town's programs, before the residents that speak it; a later one of the name takes its
-- place.

-- lines of key=value; a line that is not one is passed over
language("kv", map(choice{ seq{ { key = but("=\n") }, "=", { value = upto("\n") } }, upto("\n") }))
