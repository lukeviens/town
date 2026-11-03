# town

a little VM with one instruction: the word.

```
town <path>…      open a town: programs, or folders of them, in order
town talk <word>  say a word
town listen       hear the words: the present, then each word as it is said
town ask <word>   say a word, and wait for its answer
```

A word is `{ kind, tense, body }`, said on the square, a unix socket any program can reach. Town
stamps it with `when`, the moment it is said, in milliseconds since 1970. Its tense is what it
means:

- **past**, an event: something happened. It passes through.
- **present**, a fact: it folds into the present, the last of each kind, and is kept. A fact
  with no body is the fact that there is none: its kind leaves the present.
- **future**, an intent: something is wanted. It passes through.

A word without a tense is a fact. On the command line, a word is a line of JSON, or said short:
`[past|present|future] <kind> key=value …`. `town ask` gives its word an `id`, and its answer is a
past word of the same kind and `id`.

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

Every town starts from the same grammar:

```
fact(kind, body)  event(kind, body)  intent(kind, body)    a word in each tense
on(spec, act)                 a clause: a kind, or { kind, tense, by }; act(w) answers
react{ on…, on… }             a resident from clauses: the first that matches answers
present(kind)  past(kind)     a kind's fact now; its recent facts, oldest first
talk(word)                    say a word, besides answering
work(cmd, body)               run a command without blocking; its result comes back as a past `work`
after(ms, body)               wait without blocking; once ms have gone, a past `after` comes back
join(kind, argv)              talk with a program: its lines are past words of kind, and a future
                              word of kind is a line to it; it ends as a past `join`
read(path)  post(path, text)  a file, read whole; written whole, so a reader sees the old or the new
mend(path, make)              a missing file, made from what make() returns
quote(s)                      a string as one shell word, for a command
```

A resident is a table: `kinds`, what it hears (`"*"`, every kind but `error`, which is named to be
heard), and `listen(w)`, which answers with a word or nothing. As it opens, it hears the present of
its kinds. A town opens again whole, so a resident keeps nothing across a save but what the present
holds; a `work` or `after` it asked for is still answered, though not once town itself starts
again, and a program it joined ends, to be joined again as it opens. And perhaps:

- `watch = { kind = path }`: a file, said as a fact of its content whenever it changes, and as a
  fact of nothing when it is gone; a folder, as a bodiless past word whenever anything in it does.
  A relative path is beside the resident.
- `own = { kind, … }`: kinds only this resident says.

A resident may be written in any language. Any runnable file that is not Lua is a program town
runs, wherever it is in the town's folders. What a folder holds is the town, so keep a build, or
anything that acts as it starts, outside it, and link the program in. A program's first line says
what it is, a resident's table as JSON; then each line it reads is a word of its kinds, the present
first, and each line it says is a word, said in town.

```sh
#!/bin/sh
echo '{"kinds":["ping"]}'                      # and perhaps "watch" and "own"
while read -r w; do echo '{"kind":"pong","tense":"past"}'; done
```

Each line it writes to stderr is an `error`. Its watches, and the kinds it owns, last only while it
runs. A program runs once: one that fails says how it ended, and stays ended until the town opens
again, as one that ends well does. It runs in its own folder, and what it starts ends with it. Its
input ends when the town does, and a program ends then too.

[`examples/`](examples) has five small residents that use all of it and talk to each other. From a
checkout of this repo, run `town examples`.

When something fails, town says a past `error`, `{ from, heard, message }`: `town listen` hears
it. Only town says `work`, `after`, `join` and `error`, and what a resident watches, joins or owns;
the square refuses them from anyone else.

The square, the log and town's own files are in `~/.cache/town`, private to you.

## install

On a Mac with nix-darwin, town starts at login, and launchd keeps it running:

```nix
inputs.town.url = "github:lukeviens/town";

# in your darwin modules
town.darwinModules.default
{ town.programs = [ "/Users/you/.config/town" ]; }   # the default
```

It runs as `system.primaryUser`, which your Mac's config must name. Elsewhere, keep
`town ~/.config/town` running however you keep programs running.

The town itself is yours to write.

## build

Rust, with Lua 5.4 compiled in.

```
cargo build --release   # → target/release/town
cargo test
nix build               # the same
```
