# town

a little VM with one instruction: the word.

Everything is a word, and words are the entirety of how programs meet.
For example, two programs that know nothing of each other, each in a terminal of its own:

```
town ./residents          # a town, from a checkout of this repo
town listen for theme     # a terminal, waiting to be coloured
town talk theme bg=dark   # anything that decides the theme
```

The listener hears, among the rest, `{"kind":"theme","tense":"present","body":{"bg":"dark"}}`, as
does any other that listens for `theme` afterwards, since a fact is kept.

```
town <path>…                            open a town: residents, or folders of them, in order
town talk <word>                        say a word, and go
town listen [language] [for pattern…]   hear the words: the present, then each as it is said
town ask <word>                         say a word, and wait for its answer
```

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

`town/error` is heard only where it is named, `["/", "town/error"]`: hearing an error can make
another, so errors heard by every resident could go on without end. So, too, when what one says
while it hears an error, the last word it was told, is refused or is no word, town only prints it.

On the command line:

- **a word** is a line of JSON, or said short: `[past|present|future] <kind> key=value …`, each
  value text.
- **`town talk`** says its word and exits, without waiting to hear whether town took it. It exits
  1 if no town is at the square.
- **`town ask`** says its word as given, with an `id`. An ask is future, `town ask future time`:
  any other it refuses, and exits 2. Its answer is a past word of the same kind and `id`. It waits
  `TOWN_WAIT` seconds, 1800 if unset, then exits 1, as it does when its word is refused or no town
  is at the square.
- **`town listen`** exits 1 if no town is at the square, and 2 for arguments it cannot read.

The log:

- **what is kept**: each fact, in `past.log` in town's home (below), so a town started again has
  the present it had.
- **how far back**: `past(kind)` reads the log's last 128 KiB.
- **not kept**: who is here, which is so only while town runs.

## residents

A town is its residents. Anything that speaks to town is one, and all are the same:
a file of Lua, a program, a join, a socket reached, one at the square.

- **opened**: a town is opened with paths, residents or folders of them, in order.
- **in order**: a folder's own files are read, in name order, before its folders.
- **libraries**: a Lua file that returns nothing is a library, for the files after it.

Each says first what it is, with these fields and no others:

```
kinds       what it hears, by patterns
own         kinds only it may say facts and events of, while it is here; anyone may want them
speaks      the language it speaks: JSON if it names none, or [says, hears]
languages   the languages it is: each name, and its grammar
```

Town keeps what each resident is as a fact under `town/residents`:

- **while it is here**: its fact holds what it is, and its `id`, `name`, `at` and `square`.
  As it goes, town says the fact of nothing there.
- **its place**: where it lives. A file by its whole path,
  `town/residents/Users/you/.config/town/hi.lua`; a join under the place of who asked for it,
  `…/hi.lua/notes`, or by its kind, if town did; one at the square by its number, `square/<id>`.
- **what it shows**: who owns what, and which languages are spoken. These facts are never logged.
- **no fact**: a resident that hears, owns and is nothing has no fact.
- **town itself**: number 0, here from the start.
- **who is here**: `town listen for /town/residents`.

A resident hears, in town's one order:

1. the present of its kinds;
2. `now`, a bodiless past word at `""`, the end of the present: each word after it is said as it
   happens;
3. each word of its kinds, its own among them.

`now` is not told to a Lua resident, which hears within town, nor to one whose language cannot
hold it, as `line` cannot.

What a resident does:

- **hearing**:
  - never its own errors, those said as from it or one it speaks for, since one could make
    another; but one at the square hears them, whatever it names, as it has no other way to know;
  - within town, no word is missed;
  - town keeps up to 1024 words waiting for one that stops reading; past that, it drops words, and
    says so, once each time it falls behind;
  - one slow to say what it is is told the present as it is when it says, so no fact it hears is
    old; town says that it may have missed words between.
- **saying**: words in its language, each of `kind`, `tense`, `body` and `when`, a whole number
  town stamps over, each through town's one door, which checks that it may say it. It lasts while
  it speaks: one that only listens keeps its end open, as `town listen` does.
- **owning**:
  - a kind has one owner, the first to own it, whatever pattern either names it by: one owns
    `theme`, and another that would own `/theme` is refused;
  - within an owned kind, the nearest owner says its own, if town runs it: a join at `box/cat` says
    what is at `box/cat`, though one at `box` owns what is under `box`;
  - one at the square owns nothing within another's, so none takes `box/cat` from that join; and
    whichever came first: one at the square that owns `box/cat` keeps a join at `box` away;
  - across an opening, what a resident town runs owned is held from the square until it comes
    again, or will not, as a program says what it is only once it runs.
- **town's own words**: the kinds under `town` (below), and intents for a joined kind, which are
  told to the program town runs there: a future `tmux`, told to the join at `tmux`, is a command
  tmux runs. Residents town runs may say and own them; one at the square may not.
- **speaking for others**: a resident speaks for those under its place, so one program may be
  many residents.
  - A fact it says at `town/residents/<name>` is one under it: town places it at
    `<its place>/<name>`, as it places a join's words under its kind. That is the one thing under
    `town` any resident says, as it is its own place.
  - Each is checked as any resident is when it comes, and stands where its speaker stands: inside,
    or at the square.
  - To say a word as one of them, it names which, `by`, beside the word and never part of it: a
    word is its kind, tense, body and when.
  - As it goes, they go.
  - Only a program, or one at the square, speaks for others: a join's words are placed under its
    kind, and a Lua resident's words have no `by`, so town refuses one it says.

  ```
  {"kind":"town/residents/theme.lua","body":{"own":["theme"]}}
  {"kind":"theme","body":{"bg":"#000"},"by":"theme.lua"}
  ```

### in Lua

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
- **a library**: a Lua file that is no resident is what the files after it are written with, each
  its own copy: words, not a place to share things. A save of one opens the whole town again, as
  does a save of a Lua file the town has not opened: one new, or one that failed.

A resident is written with these, the same in every town:

```
fact(kind, body)  event(kind, body)  intent(kind, body)    a word in each tense
on(what, act)                 a clause: a kind, or { kind, tense, by }; act(w) answers
react{ on…, on… }             a resident from clauses: the first that matches answers; beside
                              them, by name, `own` and `languages`
present(kind)  past(kind)     a kind's fact now; its recent facts, oldest first; for `kind/`,
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
read(path, as)                a file, read whole: text, or with `as`, the parts of that language
post(path, body, as)          a file written whole, so a reader sees the old or the new
mend(path, make, as)          a missing file, made from what make() returns
parts(as, bytes)              bytes read in a language, as parts
bytes(as, parts)              parts written in a language, as bytes
quote(s)                      a string as one shell word, for a command
```

### as a program

Any runnable file that is not Lua is a program town runs, wherever it is in the town's folders.

- **what it is**: its first line of output says so, as any resident's does.
- **once**: it runs once. Whether it fails or ends well, it stays ended until it is saved, or the
  town opens again; one that fails says how it ended.
- **where**: it runs in its own folder, and all it starts ends with it. As it goes, or the town
  stops, it and all it started are killed, `SIGKILL`, at once.
- **its errors**: each line it writes to stderr is a `town/error`, its first KiB.
- **builds**: every runnable file in the town's folders is run, so keep a build, or anything that
  acts as it starts, outside them, and link the program in.

```sh
#!/bin/sh
echo '{"kinds":["ping"]}'                      # and perhaps "own" and "speaks"
while read -r w; do echo '{"kind":"pong","tense":"past"}'; done
```

### at a kind

`join(kind, argv)` runs a program at a kind, and `connect(kind, to)` reaches a socket there.

- **owns**: `kind` and what is under it.
- **spoken for**: by the resident that asked for it, so it goes as that one goes. One whose asker
  has no fact, as a program whose first line is `{}`, stands by its kind, spoken for by none, and
  outlives its asker.
- **told**: the facts at and under its kind, `now`, then each intent there.
- **says**: facts and events.
- **paths**: each kind is its path from `kind`: `a/b` is `kind/a/b`, and `""` is `kind`.
- **errors**: an intent its language cannot hold is a `town/error`.

```lua
join("tmux", { "tmux", "-C" }, { speaks = "line" })   -- each line a past word, each body a line
join("notes", { "town-watch", file })   -- what a path holds, as facts, and again as it changes
```

`town-watch <path>` is town's own program, one like any other: see
[`src/bin/town-watch.rs`](src/bin/town-watch.rs).

### at the square

`town listen`, `talk` and `ask` say what they are for you. Any other program says it itself, first:
`{"kinds":["/"]}` hears every kind but `town/error`, and talks; `{}` only talks.

## town

Under `town` are town's own words: what it is, what it is asked, and what failed.

```
town/residents          fact: the paths it was opened with; under it, each resident here
town/work               future { cmd }: a command run; past: what it said, or why it could not
                        start, how it ended, its ms
town/after              future { ms }: a wait; past, once the ms have gone
town/join               future { as, argv, speaks }: a program at a kind; past { code }, as it ends
town/connect            future { as, to, speaks }: a socket at a kind; past { code }, as it ends
town/open               future { path }: the resident there opened again, if it is one, or was
town/error              past { from, heard, message }, and `said`, the first KiB of a line that was
                        no word
```

- **answers**: each, past, carries the body it was asked with, its own parts added, so whoever
  asked knows it for theirs.
- **refused**: any other future under `town`.
- **watching**: town watches nothing itself. `reload` joins `town-watch` on each path it was
  opened with but one inside another, which that one's watch holds, and asks `town/open` of each
  file a watch says changed since it began, or is gone; town opens each once the opens one save
  asks have stopped.
- **its world**: the environment it starts in, each var and its value. Every process it starts
  runs in it, a command's `sh`, a join, a program, found on its `PATH`, and a Lua resident reads
  it with `os.getenv`. A test gives a town a world of its own, whose `sh` records what it is
  given and runs none, and so sees what its residents would do.

## languages

A language is one grammar run both ways: it reads bytes as parts, and writes parts as bytes. Town
speaks one itself, JSON: its log, its square, and every resident that names no other.

- **parts**: nothing, text, bytes, a number, true or false, a list, or parts by name.
  - In Lua, a table is parts by name unless it is a list; `list{}` makes one a list, so an empty
    one is said `[]`, not `{}`; and a nothing in a list is `null`, so the list keeps its places.
  - JSON says bytes as a list of numbers.
- **a stream**: a word is heard only once all of it has come.
- **a resident**: a resident is a language if it says so, `languages = { kv = grammar }`. It is
  spoken while that resident is here, and its grammar is in its fact.
  - A name belongs to one resident: another that says it is `kv` is refused.
  - A version is a name under it, `kv/2`.
- **waiting**: a resident that speaks a language no resident is yet speaks JSON meanwhile, and town
  says once that it waits. One at the square is told so.
- **as it is now**: a language is spoken as its resident says it now: said again, another grammar,
  what is said and told after is in that one; while none is it, what is said in it waits, and what
  would be told in it is not.

```
upto(s)  but(s)               text up to s, which is passed over; text of one byte or more, up to
                              the first byte in s
seq{ … }  choice{ … }         each in turn, read as its named parts, or, with none, its one other
                              piece; the first that reads
many(g, n)  map(g, n)         as many as read, or n: in order; by the parts `key` and `value`
unless(g)                     nothing, and only where g would not read
int{ size, signed, order, from, to, base, negative }    a whole number of size bytes, the first
                              the most, or `order = "little"`, the least; `from` to `to` as held,
                              read less `base`, or, `negative`, `base` less it
float{ size }                 a number of 2, 4 or 8 bytes
text(n)  raw(n)               n bytes, as text or as they are
whole(g)                      the text or bytes g reads in pieces, as one
be(g, value)  ref(name)       what g reads, as value; the language of that name
"bytes"  { name = g }         in a seq or a choice: those bytes; what g reads, by name
```

A count, `n`, is a number, a grammar that reads one, or the name of a part read before it in the
same seq, which is then not kept. Written, it is the length of what it counts.

Town's own are these, in [`residents/languages/`](residents/languages):

```
words     a word a line, [tense] kind key=value …, a tab between each, as a shell says one
line      a line of text, for a program that says lines
kv        lines of key=value
msgpack   each value in its smallest form
cbor      each value in its smallest form
```

`kv`, for one:

```lua
local pair = seq{ { key = but("=\n") }, "=", { value = upto("\n") } }
return { languages = { kv = map(choice{ pair, upto("\n") }) } }
```

## town's own residents

[`residents/`](residents) holds town's own residents: its languages, and `reload`, which opens a
resident again as it is saved. Name them first:

- **from a checkout**: `town ./residents ~/.config/town`.
- **installed by nix**: they are in `share/town/residents`, beside town.
- **from a crate**: one that depends on town, with its `testing` feature, finds them at
  `town::residents::own()`.

[`examples/`](examples) is a small town of five residents that talk to each other: from a checkout,
`town ./residents ./examples`.

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

It runs as `system.primaryUser`, which your Mac's config must name. Elsewhere, keep `town
./residents ~/.config/town` running, from a checkout, however you keep a program running.

- **town's home**: the square and the log are in `~/.town`, private to you. It holds town's own
  files, not residents: keep yours elsewhere, as `~/.config/town`.
- **another**: `TOWN_HOME`, if it is set, for a second town or a test. Set it, too, for every
  program that reaches the square, or it finds no town there.

The town itself is yours to write.

## build

Rust, with Lua 5.4 compiled in.

```
cargo build --release   # → target/release/town, and town-watch beside it
cargo test
nix build               # the same
```
