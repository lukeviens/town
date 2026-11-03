# town

a little VM with one instruction: the word.

```
town <path>…             open a town: programs, or folders of them, in order
town talk <word>         say a word
town listen [language]   hear the words: the present, then each word as it is said
town ask <word>          say a word, and wait for its answer
```

A word is `{ kind, tense, body }`, said on the square, a unix socket any program can reach. Town
stamps it with `when`, the moment it is said, in milliseconds since 1970. Its tense is what it
means:

- **past**, an event: something happened. It passes through.
- **present**, a fact: it folds into the present, the last of each kind, and is kept. A fact
  with no body is the fact that there is none: its kind leaves the present.
- **future**, an intent: something is wanted. It passes through.

A word without a tense is a fact. On the command line, a word is a line of JSON, or said short:
`[past|present|future] <kind> key=value …`, which town reads in `words`. `town ask` gives its word
an `id`, and its answer is a past word of the same kind and `id`.

A town's programs are its residents, files that listen for kinds of words and talk words back.
A Lua file that returns nothing is a library, for the files after it; a folder's own files are
read, in name order, before its folders. Save one, and the town opens again a moment later.

```lua
-- ~/.config/town/hi.lua
return react{
  on("hello", function(w) return event("hi", { to = w.body.from }) end),
}
```

```
town talk hello from=me   # town listen hears a hi
```

A resident is written with these, the same in every town:

```
fact(kind, body)  event(kind, body)  intent(kind, body)    a word in each tense
on(what, act)                 a clause: a kind, or { kind, tense, by }; act(w) answers
react{ on…, on… }             a resident from clauses: the first that matches answers
present(kind)  past(kind)     a kind's fact now; its recent facts, oldest first
talk(word)                    say a word, besides answering
work(cmd, body)               run a command without blocking; its result comes back as a past `work`
after(ms, body)               wait without blocking; once ms have gone, a past `after` comes back
join(kind, argv, body)        talk with a program: its lines are past words of kind, and a future
                              word of kind is a line to it; it ends as a past `join`
connect(kind, to, body)       talk with a socket, a path or a host:port, as join does a program;
                              it ends as a past `connect`. `speaks` in the body names the
                              language either speaks: what it says is read as parts, and what it
                              is told is written in it
read(path, as)                a file, read whole: text, or with `as`, the parts of that language
post(path, body, as)          a file written whole, so a reader sees the old or the new
mend(path, make, as)          a missing file, made from what make() returns
language(name, grammar)       say a language: how bytes are read as parts, and parts written
parts(as, bytes)              bytes read in a language, as parts
bytes(as, parts)              parts written in a language, as bytes
quote(s)                      a string as one shell word, for a command
```

A resident is a table: `kinds`, what it hears (`"*"`, every kind but `error`, which is named to be
heard), and `listen(w)`, which answers with a word or nothing. As it opens, it hears the present of
its kinds. A town opens again whole, so a resident keeps nothing across a save but what the present
holds; a `work` or `after` it asked for is still answered, though not once town itself starts
again, and a program it joined ends, to be joined again as it opens. And perhaps:

- `watch = { kind = path }`: a file, said as a fact of its content whenever it changes, and as a
  fact of nothing when it is gone; a folder, as a bodiless past word whenever anything in it does.
  A relative path is beside the resident. `{ path = path, as = language }` says a file as its parts.
- `own = { kind, … }`: kinds only this resident says.

A resident may be any program, not only Lua. Any runnable file that is not Lua is a program town
runs, wherever it is in the town's folders. What a folder holds is the town, so keep a build, or
anything that acts as it starts, outside it, and link the program in. A program's first line says
what it is, a resident's table as JSON; then each line it reads is a word of its kinds, the present
first, and each line it says is a word, said in town. Its lines are JSON, or the language it
`speaks`.

```sh
#!/bin/sh
echo '{"kinds":["ping"]}'                      # and perhaps "watch", "own" and "speaks"
while read -r w; do echo '{"kind":"pong","tense":"past"}'; done
```

Each line it writes to stderr is an `error`. Its watches, and the kinds it owns, last only while it
runs. A program runs once: one that fails says how it ended, and stays ended until the town opens
again, as one that ends well does. It runs in its own folder, and what it starts ends with it. Its
input ends when the town does, and a program ends then too.

[`examples/`](examples) has five small residents that talk to each other. From a checkout of this
repo, run `town examples`.

When something fails, town says a past `error`, `{ from, heard, message }`, and `said`, for a
line that was no word: `town listen` hears it. `work`, `after`, `join`, `connect`, `language` and
`error` are town's own, and what a resident watches, joins or owns is that resident's: the square
refuses them from anyone else.

The square's first line says how a connection speaks: `listen [language]`, `join [in] [out]`, or
`talk [in]`, in JSON if it names none; any other first line is a word in JSON, as are the lines
after it.

## languages

A language is a grammar that reads bytes as a body's parts and writes parts as bytes, the one
grammar run both ways. A part is nothing, text, bytes, a number, true or false, a list, or parts by
name; in Lua, a nothing in a list is `null`, so the list keeps its places. JSON has no bytes: it
says them as a list of numbers, and reads that back as a list. A grammar reads a stream: what has
not all come yet is waited for, so a connection in a language hears a word only once it is whole.

```
upto(s)  but(s)               text up to s, which is passed over; text of none of the bytes in s
seq{ … }  choice{ … }         each in turn: its named parts, or its one other piece; the first
                              that reads
many(g, n)  map(g, n)         as many as read, or n: in order; by the parts `key` and `value`
unless(g)                     nothing, where g would not read
int{ size, signed, order, from, to, base, negative }    a whole number, bounded, less a base
float{ size }                 a number of 2, 4 or 8 bytes
text(n)  raw(n)               n bytes, as text or as they are
be(g, value)  ref(name)       what g reads, as value; the language of that name
"bytes"  { name = g }         in a seq or a choice: those bytes; what g reads, by name
```

A count, `n`, is a number, a grammar that reads one, or the name of a part read before it in the
same seq, which is then not kept; written, it is the length of what it counts. `whole(g)` is the
text or bytes `g` reads in pieces, as one. A Lua table is parts by name unless it is a list;
`list{}` is an empty one.

A language is said as a word, a past `language`, `{ name, grammar }`, by any program;
`language(name, grammar)` says one from Lua. Town speaks it from then, and a later one of the name
takes its place, until the town opens again and its programs say theirs again. What speaks one not
said yet, a watch, a join, a connection, a program, waits for it, and says once that it waits; a
program speaks JSON while it waits, so it may say its language itself.

Town itself speaks two: JSON, for its log and its square, and `words`, a word on a line, `[tense]
kind key=value …`, a tab between each, which a shell reads with `read`, for its own CLI. Any other
is a program like any other, named among a town's programs before the residents that speak it, as
[`languages/`](languages) holds them: `kv`, lines of `key=value`; and `msgpack` and `cbor`, each
value in its smallest form.

```lua
language("kv", map(choice{ seq{ { key = but("=\n") }, "=", { value = upto("\n") } }, upto("\n") }))
```

The square, the log and town's own files are in `~/.cache/town`, private to you.

## install

On a Mac with nix-darwin, town starts at login, and launchd keeps it running:

```nix
inputs.town.url = "github:lukeviens/town";

# in your darwin modules
town.darwinModules.default
{ town.programs = [ "/Users/you/.config/town" ]; }   # the default

# a language, named before the residents that speak it
({ config, ... }: {
  town.programs =
    [ "${config.town.package}/share/town/languages/kv.lua" "/Users/you/.config/town" ];
})
```

It runs as `system.primaryUser`, which your Mac's config must name. Elsewhere, keep
`town languages/kv.lua ~/.config/town` running however you keep programs running.

The town itself is yours to write.

## build

Rust, with Lua 5.4 compiled in.

```
cargo build --release   # → target/release/town
cargo test
nix build               # the same
```
