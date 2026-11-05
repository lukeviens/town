# lua

A Lua file cannot run itself, so `lua`, a program, runs it and speaks for it: a speaker, as town's
README says one is made, for files of Lua. A speaker for any other language is made the same.

- **found**: `lua` hears the paths town opens with, and takes each `.lua` file in them that cannot
  run itself, in town's order.
- **spoken for**: each is a resident under `lua`'s place, `town/residents/<lua's path>/<its path>`,
  and goes as `lua` goes. Its words name it, `by`.
- **hears**: what it names, which `lua` passes it from what town tells `lua`.
- **reads**: any kind's present or past, which `lua` reads from town (`town/read`), the one that
  knows; the file waits for it.
- **a library**: a file that returns nothing is what the files after it are written with, each its
  own copy: words, not a place to share things. What it talks as it is read is said once, by
  `lua`.
- **parts**: a table is parts by name unless it is a list; `list{}` makes one a list, so an empty
  one is said `[]`, not `{}`; and a nothing in a list is `null`, so the list keeps its places.

A Lua resident is a table: what it is, and `listen(w)`, which answers with a word, or nil for
none.

```lua
-- ~/.config/town/hi.lua
return react{
  on("hello", function(w) return event("hi", { to = w.body.from }) end),
}
```

```
town talk past hello from=me   # town listen hears a hi
```

What a file talks as it is read is said once it has opened, in order: one that fails to open has
done nothing.

On a save, the resident saved goes and comes again, alone:

- **across a save**: it keeps nothing but what the present holds.
- **the others**: they stay as they are.
- **what it joined**: goes with it, as all it speaks for does, and is joined again as it comes.
- **what it asked**: a `town/work` or `town/after` is still answered, though not once town itself
  starts again.
- **a library**: a save of one opens every Lua file again, in a new Lua, since the files after it
  are written with it; as does a save of a Lua file `lua` has not opened: one new, or one that
  failed.

A resident is written with these, the same in every town, from [`resident`](resident):

```
fact(kind, body)  event(kind, body)  intent(kind, body)    a word in each tense
on(what, act)                 a clause: a kind, or { kind, tense, by }; act(w) answers
react{ on…, on… }             a resident from clauses: the first that matches answers; beside
                              them, by name, `own` and `languages`
present(kind)  past(kind)     a kind's fact now; its facts before, oldest first; for `kind/`,
                              `/kind` or `/`, those of each kind it names
talk(word)                    say a word, besides answering
work(cmd, body)               run a command without blocking: a future `town/work`, answered past
after(ms, body)               wait without blocking: a future `town/after`, answered past
join(kind, argv, body)        talk with a program, as kind: it says words there, and is told them,
                              in the language `speaks` names: a future `town/join`, answered past
                              as it ends
connect(kind, to, body)       talk with a socket, a path or a host:port, as join does a program:
                              a future `town/connect`, answered past as it ends
TOWN                          town itself, for a program to call: `$TOWN talk …`
WATCH                         town's watch, to join by its path: `join(kind, { WATCH, path })`
read(path, as)                a file, read whole: text, or with `as`, the parts of that language
post(path, body, as)          a file written whole, so a reader sees the old or the new
mend(path, make, as)          a missing file, made from what make() returns
parts(as, bytes)              bytes read in a language, as parts
bytes(as, parts)              parts written in a language, as bytes
quote(s)                      a string as one shell word, for a command
```

A language, too, is written with these, as town's README says under *languages*.
