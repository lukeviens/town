# town

a little VM with one instruction: the word.

Programs meet only in words. Two that know nothing of each other, each in a terminal of its own:

```
town ./residents ./target/release/lua                 # a town, from a checkout of this repo
town '{"kinds":["theme"]}'                            # a terminal, waiting to be coloured
echo '{"kind":"theme","body":{"bg":"dark"}}' | town '{}'   # anything that decides the theme
```

The listener hears, among the rest, `{"kind":"theme","tense":"present","body":{"bg":"dark"}}`, as
does any other that listens for `theme` afterwards, since a fact is kept.

```
town <path>…            open a town: residents, or folders of them, in order
town '<first word>'     be at the square, as the first word says: what it reads is said there,
                        and all it is told is written out
```

`town '<first word>'` is a program at the square, like any other:

- **its first word** says what it is: `{}` to talk, `{"kinds":["/","town/error"]}` to hear all,
  `{"speaks":"words"}` to talk short, `{"name":"luke"}` to come by a name, as another town does.
- **waits** for a town to come, and keeps what it reads until then. Given nothing, it does not
  wait.
- **ends** as its input ends: 0. As the square closes: 1.

This is a guide. The rules, by number, are [`PROTOCOL.md`](PROTOCOL.md).

## words

A word is `{ kind, tense, body }`, said at the square, a unix socket any program can reach.
Town stamps it with `when`, the moment it is said, in milliseconds since 1970, whatever `when` it
came with.

Its tense is what it means:

- **past**, an event: something happened. It passes through.
- **present**, a fact: it folds into the present, the last of each kind, and is kept.
  - A fact with no body takes its kind out of the present.
  - A word without a tense is a fact.
- **future**, a want: something is wanted. It passes through.

Its kind is a path, as a file's is:

- **under**: `nvim/1` is under `nvim`, and a kind may have a fact and kinds under it at once.
- **a fact of nothing**: one at `nvim` takes `nvim`'s fact away, and leaves the kinds under it.
- **well-formed**: no part of a word's kind is empty, `.` or `..`, so town refuses `nvim/`, `a//b`
  and `../x`.
- **the empty kind**: `""` is no word's kind. It is where a resident stands, and only `now` is
  told there (below).

Residents name the kinds they hear or own by a pattern: a kind with a slash after or before it, or
a slash alone. What a resident hears is its `kinds` but its `except`: `kinds = { "/" }` and
`except = { "other/" }` is every kind but what is under `other`. A kind is any text, `!` too.

```
nvim      nvim itself
nvim/     what is under nvim
/nvim     nvim, and what is under it
/         every kind but town/error
```

## residents

A town is its residents: whatever speaks to town. There are four ways in, and one door.

- **a program**: a runnable file.
- **a speaker's**: a file a speaker runs.
- **a join**: a program, at a kind.
- **at the square**: any program that reaches it.

A town opens with paths: residents, or folders of them, in order. A folder's files are read in name
order, then its folders; nothing hidden.

A resident's first word says what it is, with these fields and no others:

```
kinds       the patterns it hears
except      the patterns it does not hear, of what its kinds name
own         the kinds only it may say facts and events of; anyone may want them
speaks      its language, or [says, hears]; JSON if none
languages   the languages it is: each name, and its grammar
names       the names another town may come by: each name, and the kinds it hears
waits       the most words town keeps for it while it does not read; all if none
most        the most bytes of one word it is told; any if none
keep        for each pattern it owns, how many facts before the present the log keeps; all
            if none
```

Town sets no limit of its own: each is set by the one it bounds, if it wants one. What one says
may be as deep and as long as memory allows, in any language.

### a program

A runnable file in the town's folders.

- **what it is**: its first line says so.
- **once**: it runs once. Ended, well or not, it stays ended until it is saved or the town opens
  again. One that fails says how it ended.
- **where**: in its own folder. As it goes, or town stops, it and all it started are killed,
  `SIGKILL`.
- **its errors**: each line on stderr is a `town/error`.
- **builds**: every runnable file in the town's folders is run. Keep builds outside them, and link
  the program in.

```sh
#!/bin/sh
echo '{"kinds":["ping"]}'
while read -r w; do echo '{"kind":"pong","tense":"past"}'; done
```

### a speaker

A program that runs files that cannot run themselves, and speaks for each: one program, many
residents, each under its place. It:

1. **finds**: it hears `town/residents`, the paths town opened with, and takes its files.
2. **says each**: a fact at `town/residents/<name>`, placed at `<its place>/<name>` and checked as
   any resident's is. Each stands where its speaker stands.
3. **is told for each**: each word one hears, once for each, `by` its name; as one comes, its own
   present, then `now`, `by` it. A word with no `by` is the speaker's own.
4. **speaks as each**: a word it says for one names it, `by`, beside the word, never in it; and
   each says its own `now`.
5. **says all is said**: its own `now`. Until then, what its residents owned is kept; then what one
   not said again owned is let go.
6. **is refused**: a fact town refuses is a `town/error` told to the speaker, `heard` the kind it
   said, `place` where that one would be.

As it goes, they go.

```
→ {"kinds":["town/residents"]}
→ {"kind":"town/residents/hi","body":{"kinds":["hello"]}}
← {"kind":"","tense":"past","by":"hi"}
→ {"kind":"","tense":"past","by":"hi"}
→ {"kind":"hi","tense":"past","body":{"to":"me"},"by":"hi"}
→ {"kind":"","tense":"past"}
```

`lua` is one: [`residents/lua`](residents/lua).

### a join

A program a resident asks town to run at a kind, `town/join`. To join a socket, join a program
that reaches it, `{ "nc", "-U", path }`.

- **owns**: its kind, and what is under it.
- **its asker's**: its asker may join there again, and the one before ends; another that asks is
  refused, and the one there stays.
- **spoken for**: by its asker, and goes as it goes. One whose asker has no fact, as a program
  whose first line is `{}`, is spoken for by none, and outlives its asker.
- **told**: `now`, then each want there: a future `tmux` is a command for the join at `tmux`. It
  is not told what is there: it says that itself, then `now`.
- **says**: facts and events, by paths from its kind: `a/b` is `kind/a/b`, and `""` is `kind`.
- **errors**: a want its language cannot say is a `town/error`.

A program that reads and writes lines joins unchanged:

```lua
join("tmux", { "tmux", "-C" }, { speaks = "line" })   -- each line a past word, its body the line
join("notes", { "mirror", file })   -- what a path contains, as facts, and again as it changes
```

`mirror` is one: [`residents/mirror.rs`](residents/mirror.rs).

### a town

A town joins another as a join: `town '{"name":"luke.example"}'`, run there, is that town's
square, come to by a name it gives. Over ssh:

```lua
join("leeds.example", { "ssh", "leeds.example", "town", '{"name":"luke.example"}' })
```

That town's `theme` is `leeds.example/theme` here, and a want at `leeds.example/place` is a want
at `place` there. A town it joins is under it, `leeds.example/york.example/…`, so towns nest as
folders do.

- **by name**: a resident there gives the name, what one that comes by it hears:
  `names = { ["luke.example"] = { kinds = { "/" } } }`. The town it comes to says what it is, not
  the one that comes. A name is one resident's; one no resident gives yet waits for one, and town
  says once that it waits.
- **as it is now**: one that comes by a name goes as the name goes or changes.
- **once**: it comes once, and ends as it goes: 0 as its join closes its input, 1 as that town goes.
  To come again is the joining resident's choice: it joins again, as on a past `town/join`.
- **both ways**: what each says is said in the other, as one at the square says it, and what each
  hears is told; its errors too, as words, at `leeds.example/town/error` here. To town it is a join
  like any, and says nothing of being a town.
- **again**: each time it comes, it is told that town's present, then `now`: so what that town no
  longer has is nothing here.
- **a domain**: a town is joined at its domain, and comes by it as a name. One come by
  `luke.example` is never told a word whose path passes through it, `york.example/luke.example/x`,
  as a Usenet site is never sent an article whose `Path:` names it: so towns that hear each other,
  in a ring of any size, have each word once. Who comes is proven by the road (TLS), not town.
- **who it is**: not town's to know. There, `authorized_keys` gives a key one command,
  `command="town '{\"name\":\"luke.example\"}'"`; here, only you can open the square.

### at the square

Any program that reaches the square says its first word, then talks, as `town '<first word>'`
does. What it may ask, own and hear: see **town**, **owning** and **hearing**.

### its fact

While a resident is here, town keeps a fact of it at its place, under `town/residents`: what it
is, and its `id`, `name`, `at`, `square`, `visitor`, whether it came by a name, and `by`, its
speaker. As it goes, the fact is nothing.

```
a program           its whole path          town/residents/Users/you/bin/clock
a speaker's         under its speaker's     town/residents/<speaker's place>/<name>
a join              under its asker's       town/residents/<asker's place>/notes
                    or, with none, its kind
at the square       its number              town/residents/square/<id>
```

- **one place, one resident**: none comes where another is.
- **none**: a resident that hears, owns and is nothing has no fact.
- **town** itself: number 0, here from the start.
- **who is here**: `town '{"kinds":["/town/residents"]}'`.

### hearing

A resident is told, in order:

1. the present of its kinds;
2. `now`, a past word at `""` with no body: the present is told;
3. each word of its kinds as it is said, its own too.

- **`now`**: not told in a language that cannot say it, as `line` cannot.
- **errors**: `town/error` is heard only where it is named, `["/", "town/error"]`, as hearing an
  error can make another. Each carries its `place`. A resident never hears its own, but one at the
  square hears its own and those of the ones it speaks for, whatever it names: it has no other way
  to know.
- **answering**: an error made while answering an error, refused, misheard, on stderr or said,
  is `answering: true`: said, but told to none that hears `town/error`, so errors never answer
  each other.
- **none missed**: no word is lost, but for one that says `waits`: past that, town drops words for
  it, and says so, and tells it so, once each time it falls behind. One told a word past its `most`
  is not told it.
- **slow to come**: one slow to say what it is is told the present as it is then, and that it may
  have missed words between.

### saying

- **a word**: its `kind`, `tense` and `body`, in its language. Town stamps `when`.
- **no change, no word**: a fact said again as it is, or nothing where none is, goes nowhere.
- **`now`**: the word it is told, said back: it has said again each fact it has. What it owns, or
  owned before it came again, and has not said since, is nothing; what was kept for it is let go;
  and a past word at its fact says so. At a join, `""` is its kind, but a past word there with no
  body is `now`.
- **the door**: every word comes through one door, which checks that its sayer may say it.
- **while here**: a resident is here while its end is open. One that only listens keeps it open,
  as `town '{"kinds":["/"]}'` does while what it reads is open.

### owning

- **one owner**: a kind has one, the first to own it, whatever pattern names it. One owns `theme`;
  another that would own `/theme` is refused.
- **the nearest**: within an owned kind, the nearest owner says its own. A join at `box/cat` says
  what is there, though one at `box` owns what is under `box`.
- **the square**: one at the square owns nothing within another's, and one that owns `box/cat`
  keeps a join at `box` away.
- **held**: as town opens again, what a resident owned is held for it until it comes back and says
  `now`, or goes: meanwhile none else owns it, nor says a fact there.

## town

Under `town` are town's own words: who is here, what it is asked, and what failed.

```
town/residents   fact: the paths it opened with; under it, each resident here
                 past, under it: that resident has said `now`
town/work        future { argv }: a program run to its end; past { code, signal, out, ms }
town/join        future { as, argv, speaks }: a program at a kind; past { code, out }, as it
                 ends, `out` why, if it could not begin
town/open        future { path }: a file saved; past { paths }: those it touched, not as they
                 were opened. Town opens its programs again, a speaker those it runs
town/read        future { kind, tense }: past { body }, what town keeps of it
town/error       past { from, place, heard, message, answering }, or `said`, what was no word
```

Town waits on no clock and knows no shell: `when` is only what a word says. A wait is a program,
`{ "sleep", "2" }`, and so is a shell, `{ "sh", "-c", cmd }`.

- **town's alone**: what is so under `town`, facts and events, town alone says, and none owns
  there. A resident asks of it, by a want, and says its own errors there; one at the square asks
  only `town/read`, and wants nothing of a joined kind.
- **a work**: runs in its own group, as a program does, and ends with town. `out` is what it says,
  as a shell's `$(…)` gives it; each line on its stderr is a `town/error`; `signal`, the one that
  ended it, if one did.

- **answers**: past, the body asked with its own parts added, so the asker knows it for its own.
  A read is town's alone, and its answer the asker's: none else hears either, and neither is
  logged. So too a read of a town joined at `other`, `other/town/read`: it goes to that join
  alone, and its answer to the asker alone, by the `id` it asked by.
- **reading**: present is the fact now; past, each fact of it, the present among them, oldest
  first; a pattern gives each kind. The future is not kept, so it is not read. A town that comes by
  a name reads only what its name gives.
- **refused**: any other want under `town`, as it comes, before any hears it.

Its world is the environment it starts in: every process it starts runs in it, and is found on
its `PATH`, as a shell finds them. A test gives a town a world of its own.

The log:

- **what is kept**: each fact, in a store: one that adds a word, reads all back in order, and
  writes all again smaller. Town's own is a file, `past.log` in its home (below), so a town started
  again has the present it had; a test's may be memory.
- **how far back**: all, but what a kind's owner `keep`s: the present and as many before it. The
  log is written again once what is no longer kept is as much as the rest.
- **not kept**: who is here, which is so only while town runs.

## languages

Residents meet in words, but a program writes and reads bytes. A language is how its bytes are
words: town reads what it writes as words, and writes each word it hears as bytes. So a program
speaks as it already does: tmux in lines, a config in `key=value`, a fast one in msgpack.

- **spoken**: a resident names the language it speaks, `speaks`, or two, `[says, hears]`. One that
  names none speaks JSON, as town's log and square do.
- **a grammar**: a language is one grammar, run both ways: it reads a word's parts, `kind`,
  `tense`, `body` and `when`, from bytes, and writes them as bytes.
- **a resident**: a language is a resident's, as all else is. One that says
  `languages = { kv = grammar }` is `kv`: its grammar is in its fact, and `kv` is spoken while it
  is here.
  - **one name, one resident**: another that says it is `kv` is refused.
  - **a version**: a name under it, `kv/2`.
  - **said again**: what is said and told after is in its new grammar.
- **waiting**: one that speaks a language no resident is speaks JSON meanwhile, and town says once
  that it waits. What is said in it waits too, and nothing is told in it.
- **parts**: nothing, text, bytes, a number, true or false, a list, or parts by name. JSON says
  bytes as a list of numbers.
- **a stream**: a word is heard once all of it has come.

A grammar is made of these:

```
upto(s)  but(s)               text up to s, which is passed over; text of a byte or more, none in s
seq{ … }  choice{ … }         each in turn, as its named parts, or its one other piece; the first
                              that reads
many(g, n)  map(g, n)         as many as read, or n: in order; by the parts `key` and `value`
unless(g)                     nothing, where g would not read
int{ size, signed, order, from, to, base, negative }    a whole number of size bytes, the first
                              the most, or `order = "little"`, the least; `from` to `to` as written,
                              read less `base`, or, `negative`, `base` less it
float{ size }                 a number of 2, 4 or 8 bytes
text(n)  raw(n)               n bytes, as text or as they are
whole(g)                      what g reads in pieces, as one
be(g, value)  ref(name)       what g reads, as value; the language of that name
"bytes"  { name = g }         in a seq or a choice: those bytes; what g reads, by name
```

A count, `n`, is a number, a grammar that reads one, or the name of a part read before it in the
same seq, which is then not kept. Written, it is the length of what it counts.

Town's own, in [`residents/languages/`](residents/languages):

```
words     a word a line: [tense] kind key=value …, a tab between each
line      a line of text
kv        lines of key=value
msgpack   each value in its smallest form
cbor      each value in its smallest form
```

`kv`:

```lua
local pair = seq{ { key = but("=\n") }, "=", { value = upto("\n") } }
return { languages = { kv = map(choice{ pair, upto("\n") }) } }
```

## town's own residents

In [`residents/`](residents), each made as any other is:

- **languages**: the five above.
- **`reload`**: joins a `mirror` on each path town opened with, save one inside another, and asks
  `town/open` of each file that changes or goes.
- **`lua`**: a speaker for Lua files, its own among them: [`residents/lua`](residents/lua).
- **`mirror`**: a program to join, by name: [`residents/mirror.rs`](residents/mirror.rs).

`lua` and `mirror` are programs, built beside town. Name `residents/` and `lua` among a town's
paths; `mirror` is on `PATH` beside town, as any program a resident calls.

[`examples/`](examples) is a small town of five residents that talk to each other:
`town ./residents ./target/release/lua ./examples`.

## install

On a Mac with nix-darwin, town starts at login, and launchd keeps it running:

```nix
inputs.town.url = "github:lukeviens/town";

# in your darwin modules
town.darwinModules.default
# the default: town's own residents, then yours
({ config, ... }: {
  town.residents = [ "${config.town.package}/share/town/residents" "/Users/you/.config/town" ];
})
```

It runs as `system.primaryUser`, which your Mac's config must name. Elsewhere, keep
`town ./residents ./target/release/lua ~/.config/town` running however you keep a program running.

- **town's home**: `~/.town`, private to you, contains the square and the log. Keep your residents
  elsewhere, as `~/.config/town`.
- **its stderr**: each error town says, said again as a line, for whoever runs it: errors are past
  words, and the log keeps none.
- **one to a home**: a town keeps its home's lock while it runs; another finds it kept, and says
  it is already up.
- **another**: `TOWN_HOME`, for a second town or a test. Set it for every program that reaches the
  square too.

The town itself is yours to write.

## build

Rust; `lua` has Lua 5.4 compiled in.

```
cargo build --release   # → target/release/town, and mirror and lua beside it
cargo test
nix build               # the same
```
