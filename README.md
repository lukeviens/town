# town

a little VM with one instruction: the word.

```
town <path>…      open a town: Lua files, or folders of them, in order
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

A town's programs are its residents, Lua files that listen for kinds of words and talk words back.
A file that returns nothing is a library, for the files after it; a folder reads its own files, in
name order, before its folders. Save one, and the town opens again a moment later.

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
present(kind)  past(kind)     a kind's fact now; the facts it has been lately, oldest first
talk(word)                    say a word, besides answering
work(cmd, body)               run a command without blocking; its result comes back a past `work`
after(ms, body)               wait without blocking; once ms have gone, a past `after` comes back
read(path)  emit(path, text)  a file, read whole; written whole, so a reader sees the old or the new
heal(path, make)              a missing file, made from what make() returns
quote(s)                      a string as one shell word, for a command
```

A resident is a table: `kinds`, what it hears (`"*"`, every kind), and `listen(w)`, which answers
with a word or nothing. As it opens, it hears the present of its kinds. A town opens again whole,
so a resident keeps nothing across a save but what the present holds; a `work` or `after` it asked
for is still answered, though not once town itself starts again. And perhaps

- `watch = { kind = path }`: a file, said as a fact of its content whenever it changes, and as a
  fact of nothing when it is gone; a folder, as a bodiless past word whenever anything in it does.
  A relative path is beside the resident.
- `join = { kind = argv }`: a program town keeps running. Each line it says is a past word of that
  kind; `work(line, { join = kind })` writes one back.
- `own = { kind, … }`: kinds only this resident says.

When something fails, town says a past `error`, `{ from, heard, message }`: `town listen` hears
it. Only town says `work`, `after` and `error`, and what a resident watches, joins or owns; the
square refuses them from anyone else.

The square, the log and town's own files are in `~/.cache/town`, private to you.

## install

On a mac with nix-darwin, town runs from login, kept open by launchd:

```nix
inputs.town.url = "github:lukeviens/town";

# in your darwin modules
town.darwinModules.default
{ town.programs = [ "/Users/you/.config/town" ]; }   # the default
```

It runs as `system.primaryUser`, which your mac's config must name. Elsewhere, keep
`town ~/.config/town` running however you keep programs running.

The town itself is yours to write.

## build

Rust, with Lua 5.4 compiled in.

```
cargo build --release   # → target/release/town
cargo test
nix build               # the same
```
