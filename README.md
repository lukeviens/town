# town

a little VM with one instruction: the word.

Everything is a word, and words are the entirety of how programs meet.
For example, two programs that know nothing of each other, each in a terminal of its own:

```
town ./residents ./target/release/lua   # a town, from a checkout of this repo
town listen for theme                   # a terminal, waiting to be coloured
town talk theme bg=dark                 # anything that decides the theme
```

The listener hears, among the rest, `{"kind":"theme","tense":"present","body":{"bg":"dark"}}`, as
does any other that listens for `theme` afterwards, since a fact is kept.

```
town <path>…                            open a town: residents, or folders of them, in order
town talk <word>                        say a word, and go
town listen [language] [for pattern…]   hear the words: the present, then each as it is said
town ask <word>                         say a word, and wait for its answer
```

- **a word** is a line of JSON, or said short: `[past|present|future] <kind> key=value …`, each
  value text.
- **`town talk`** says its word and exits, without waiting to hear whether town took it. It exits
  1 if no town is at the square.
- **`town ask`** says its word as given, with an `id`. An ask is future, `town ask future time`:
  any other it refuses, and exits 2. Its answer is a past word of the same kind and `id`. It waits
  `TOWN_WAIT` seconds, 1800 if unset, then exits 1, as it does when its word is refused or no town
  is at the square.
- **`town listen`** exits 1 if no town is at the square, and 2 for arguments it cannot read.

## words

A word is `{ kind, tense, body }`, said on the square, a unix socket any program can reach.
Town stamps it with `when`, the moment it is said, in milliseconds since 1970, whatever `when` it
came with.

Its tense is what it means:

- **past**, an event: something happened. It passes through.
- **present**, a fact: it folds into the present, the last of each kind, and is kept.
  - A fact with no body takes its kind out of the present.
  - A word without a tense is a fact.
- **future**, an intent: something is wanted. It passes through.

Its kind is a path, as a file's is:

- **under**: `nvim/1` is under `nvim`, and a kind may have a fact and kinds under it at once.
- **a fact of nothing**: one at `nvim` takes `nvim`'s fact away, and leaves the kinds under it.
- **well-formed**: no part of a word's kind is empty, `.` or `..`, so town refuses `nvim/`, `a//b`
  and `../x`.
- **the empty kind**: `""` is no word's kind. It is where a resident stands, and only `now` is
  told there (below).

Residents name the kinds they hear or own by a pattern: a kind with a slash after or before it, or
a slash alone.

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
- **a join**: a program or a socket, at a kind.
- **at the square**: any program that reaches it.

A town opens with paths: residents, or folders of them, in order. A folder's files are read in name
order, then its folders.

A resident's first word says what it is, with these fields and no others:

```
kinds       the patterns it hears
own         the kinds only it may say facts and events of; anyone may want them
speaks      its language, or [says, hears]; JSON if none
languages   the languages it is: each name, and its grammar
```

### a program

A runnable file in the town's folders.

- **what it is**: its first line says so.
- **once**: it runs once. Ended, well or not, it stays ended until it is saved or the town opens
  again. One that fails says how it ended.
- **where**: in its own folder. As it goes, or town stops, it and all it started are killed,
  `SIGKILL`.
- **its errors**: each line on stderr is a `town/error`, its first KiB.
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
   any resident's is. Each stands where its speaker stands, and its speaker is told what it hears.
3. **speaks as each**: a word it says for one names it, `by`, beside the word, never in it.
4. **says all is said**: a past `town/residents` with no body. Until then, what its residents
   owned is held; then what one not said again owned is let go.
5. **lets go**: a fact town refuses is the speaker's `town/error`, `heard` the kind it said,
   `place` where that one would be. It is told so, though it hears none of its own errors.

As it goes, they go.

```
{"kinds":["town/residents"]}
{"kind":"town/residents/hi","body":{"kinds":["hello"]}}
{"kind":"hi","tense":"past","body":{"to":"me"},"by":"hi"}
{"kind":"town/residents","tense":"past"}
```

`lua` is one: [`residents/lua`](residents/lua).

### a join

A program a resident asks town to run at a kind, `town/join`, or a socket to reach there,
`town/connect`.

- **owns**: its kind, and what is under it.
- **spoken for**: by its asker, and goes as it goes. One whose asker has no fact, as a program
  whose first line is `{}`, is spoken for by none, and outlives its asker.
- **told**: the facts at and under its kind, `now`, then each want there: a future `tmux` is a
  command for the join at `tmux`.
- **says**: facts and events, by paths from its kind: `a/b` is `kind/a/b`, and `""` is `kind`.
- **errors**: a want its language cannot hold is a `town/error`.

A program that reads and writes lines joins unchanged:

```lua
join("tmux", { "tmux", "-C" }, { speaks = "line" })   -- each line a past word, its body the line
join("notes", { WATCH, file })   -- what a path holds, as facts, and again as it changes
```

`watch` is one: [`residents/watch.rs`](residents/watch.rs).

### at the square

`town listen`, `talk` and `ask` say what they are for you. Any other program says it itself:
`{"kinds":["/"]}` hears every kind but `town/error`, and talks; `{}` only talks.

- **town's own**: it says and owns nothing under `town`, but may ask `town/read`, and wants
  nothing of a joined kind.
- **owning**: it owns nothing within another's; and one that owns `box/cat` keeps a join at `box`
  away.
- **errors**: it hears its own, and those of the ones it speaks for, whatever it names, as it has
  no other way to know.

### its fact

While a resident is here, town keeps a fact of it at its place, under `town/residents`: what it
is, and its `id`, `name`, `at` and `square`. As it goes, the fact is nothing.

```
a program           its whole path          town/residents/Users/you/bin/clock
a speaker's         under its speaker's     town/residents/<speaker's place>/<name>
a join              under its asker's       town/residents/<asker's place>/notes
                    or, with none, its kind
at the square       its number              town/residents/square/<id>
```

- **one place, one resident**: none comes where another is.
- **none**: a resident that hears, owns and is nothing has no fact.
- **town**: number 0, here from the start.
- **who is here**: `town listen for /town/residents`.

### hearing

A resident is told, in order:

1. the present of its kinds;
2. `now`, a past word at `""` with no body: the present is told;
3. each word of its kinds as it is said, its own too.

- **`now`**: not told to a speaker's, or in a language that cannot hold it, as `line` cannot.
- **errors**: `town/error` is heard only where it is named, `["/", "town/error"]`, as hearing an
  error can make another. A resident never hears its own; a speaker hears those of the ones it
  speaks for, each with its `place`. If what one says while it hears an error is refused, or no
  word, town only prints it.
- **none missed**: within town, no word is lost. For one that stops reading, town keeps 1024 words;
  past that, it drops words, and says so once each time it falls behind.
- **slow to come**: one slow to say what it is is told the present as it is then, and that it may
  have missed words between.

### saying

- **a word**: its `kind`, `tense` and `body`, in its language. Town stamps `when`.
- **the door**: every word comes through one door, which checks that its sayer may say it.
- **while here**: a resident is here while its end is open. One that only listens keeps it open,
  as `town listen` does.

### owning

- **one owner**: a kind has one, the first to own it, whatever pattern names it. One owns `theme`;
  another that would own `/theme` is refused.
- **the nearest**: within an owned kind, the nearest owner says its own. A join at `box/cat` says
  what is there, though one at `box` owns what is under `box`.
- **held**: across an opening, what a resident owned is held from the square until it comes again,
  or will not.

## town

Under `town` are town's own words: what it is, what it is asked, and what failed.

```
town/residents   fact: the paths it opened with; under it, each resident here
                 past, no body: a speaker's all said
town/work        future { cmd }: a command run; past: what it said, or why it could not start,
                 how it ended, its ms
town/after       future { ms }: a wait; past, once the ms have gone
town/join        future { as, argv, speaks }: a program at a kind; past { code }, as it ends
town/connect     future { as, to, speaks }: a socket at a kind; past { code }, as it ends
town/open        future { path }: a file saved; past { paths }, once the save has settled, every
                 file it touched: town opens its programs again, and a speaker those it runs
town/read        future { kind, tense }: past { body }, what town keeps of it
town/error       past { from, place, heard, message }, or `said`, the first KiB of what was no word
```

- **answers**: past, the body asked with its own parts added, so the asker knows it for its own.
  A read's goes to the asker alone, and is not logged.
- **reading**: present is the fact now; past, the facts before it, oldest first; a pattern gives
  each kind. The future is not kept, so it is not read.
- **refused**: any other future under `town`.

The log:

- **what is kept**: each fact, in `past.log` in town's home (below), so a town started again has
  the present it had.
- **how far back**: a read of a kind's past reads the log's last 128 KiB.
- **not kept**: who is here, which is so only while town runs.

Its world is the environment it starts in: every process it starts runs in it, and is found on
its `PATH`. Town adds `TOWN`, itself, and `WATCH`, its watch, beside it or in `libexec/town`; each
as given, if it names a file. Its programs are not on `PATH`. A test gives a town a world of its
own.

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
                              the most, or `order = "little"`, the least; `from` to `to` as held,
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
- **`reload`**: joins a `watch` on each path town opened with, but one inside another, and asks
  `town/open` of each file that changes or goes.
- **`lua`**: a speaker for Lua files, its own among them: [`residents/lua`](residents/lua).
- **`watch`**: a program to join, `WATCH`: [`residents/watch.rs`](residents/watch.rs).

`lua` and `watch` are programs, built beside town. Name `residents/` and `lua` among a town's
paths; town finds `watch` itself.

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

- **town's home**: `~/.town`, private to you, holds the square and the log. Keep your residents
  elsewhere, as `~/.config/town`.
- **another**: `TOWN_HOME`, for a second town or a test. Set it for every program that reaches the
  square too.

The town itself is yours to write.

## build

Rust; `lua` has Lua 5.4 compiled in.

```
cargo build --release   # → target/release/town, and watch and lua beside it
cargo test
nix build               # the same
```
