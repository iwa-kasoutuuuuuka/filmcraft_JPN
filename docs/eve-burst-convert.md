# Turning the PS2 "EVE burst error PLUS" scenarios into scripts

`tools/eve_burst/eve_to_script.py` reads the decoded scenarios of the PS2 game *EVE burst error PLUS*
(the `01_シナリオ/*/*.json` files of the user's own study dump; see its `このフォルダについて.txt`) and
writes a FilmCraft [script](script.md) for one **playthrough**. It is the game-specific sibling of
[game2script](game-import.md): that one applies text rules to a scenario that is already text, this
one runs the game's own instruction set (flags, branches, menus, scenario moves), because the game
is a branching adventure and a video needs one path through it.

It is for personal technical study of material you own. Nothing from the dump is ever copied into
the repository, written to, or published; the converter only reads it. The pictures, sounds and
voices are other people's work: do not publish what you make from them (see
[game-import.md](game-import.md#rights)).

## Running it

```bash
python tools/eve_burst/eve_to_script.py --root "E:/filmcraft_JPN/PS2_EVE_Bursterror解析データ" \
    --route kojirou --out out/kojirou.fcscript.json --report out/kojirou.report.json
python tools/eve_burst/eve_to_script.py --root ... --route marina --out out/marina.fcscript.json
```

Python 3.9+, standard library only; a full route takes about ten seconds (the default policy also
plays the other side alongside, see below; `--policy explore` takes about three). `--root` is the folder
that holds `01_シナリオ`, `02_画像` and `03_音声`. Paths inside the script are relative to the script
file, so write it somewhere on the same drive as the dump (on another drive they come out absolute).
Then check and build it as usual: `script.validate`, `script.pickVoices`, `anime.run`
([script.md](script.md)).

| Option | Meaning |
|---|---|
| `--route kojirou` / `marina` | The two routes: `a01` at its label 0 ("start as Kojirou": clears the flags, then the intro) and `b01`. `kojirou` is the default. |
| `--start SCN[:LABEL]` | Start anywhere instead: `a001`, `a01:2` (a label number), `b01`. The protagonist side follows the first letter (`b` = Marina). |
| `--choices FILE` | A JSON list of selections for the menus, in the order they come up (below). |
| `--choices-only` | End the path when the list is used up instead of going on with the policy. |
| `--policy days` / `explore` / `order` | The menu policy once the list is used up (below). `days` (the default) keeps the story days in order; `explore` is the one before it; `order` is the naive one. |
| `--no-zapping` | With `days`: do not play the other side alongside (below). |
| `--on-stuck teleport` / `stop` | What to do when nothing at the current place has anything new (below). |
| `--max-steps`, `--max-lines`, `--max-moves` | Hard caps (3 000 000 instructions, 60 000 lines, 4 000 scenario moves). A path that hits one ends with that reason. |
| `--fps` | Frames per second of the game's wait instruction (assumed 60). |
| `--terminal narration` / `skip` | Text printed on the hacking terminals: spoken as narration, or left out. |
| `--subcg ignore` / `background` | The small-window CGs: ignored (listed in the report), or shown as the background. |
| `--movies skip` / `direction` | Event movies: left out (listed in the report), or an empty direction line where they play. |
| `--scenarios DIR`, `--title`, `--report FILE` | Scenario folder override, script title, the report. |
| `--episodes` | Also write one script per story day beside `--out`: `kojirou.ep01.fcscript.json`, `kojirou.ep02…` (below). The whole script is still written. |
| `--episode-lines N` | Cut a day with more than N lines into several episodes, between scenes (implies `--episodes`). |

## The path policy

The scenario files hold every branch of the game. The interpreter runs the real instruction set:

- **Flags.** `F0`–`F255` are shared, `F256` and up belong to the current protagonist (`0x802A`
  picks the side, `0x8011` clears that side's flags). All flags start at 0.
- **Branches.** `0x8003` continues if all its conditions hold and otherwise jumps to its target;
  `0x8004` ends a branch body and skips the chain of `0x8003` branches that follows (else-if).
- **Jumps and moves.** `0x8005` jumps (dropping any open menus), `0x8020` loads another scenario at
  its first instruction (`a020-2` is `a020_2`), `0x800D` is the ending. The scenario `c01` that
  `a01` mentions does not exist on the disc: a path that moves there ends with a note.
- **Menus.** A command-select menu (`0x8002`) shows the entries whose flag conditions hold. The
  chosen entry runs until `0x8006` ("return to the calling menu"), which shows the menu again; an
  entry may open a submenu. Nothing in the code leaves a menu except moving on, so the interpreter
  treats the game's *cancel* button as backing out of a submenu to the one above. The instruction
  after a menu (usually a second save/load menu) is never run, and entries called セーブ, ロード,
  Ｓｙｓｔｅｍ… are never chosen.

At each menu the **`explore` policy** does this (the default, `days`, builds on it: below):

1. For every entry that can be chosen, it *looks ahead*: it runs the entry on a copy of the state,
   follows submenus (to a depth of four) and moves to other scenarios, and counts the text lines,
   flag changes, sounds and moves it would run that have **not run yet on this path**.
2. Entries with nothing new are skipped. This is what makes "talk to her again after you read the
   file" work (the same entry has new lines once a flag changed) and what stops a conversation
   being shown twice.
3. Of the rest, entries that come back to the menu go first (new ones, then repeats), in menu
   order; entries that move on to another place go after them, the one with the most new content
   first. That is how such games are played: look at everything, then leave.
4. When no entry has anything new, the interpreter backs out of the submenu to the menu above.
   At the outermost menu the path is **stuck**.

When stuck, `--on-stuck teleport` (the default) moves to the first scenario on the same side
(`a…` or `b…`, in number order; `ab01` and `c02`, where the two sides meet, last) in which a look
ahead shows something new, and starts it as the game does (its first instruction decides what to
show from the flags). It is as if the map had a road there; the game itself would keep the player
at a puzzle until the right trigger is found. The report lists every teleport. `--on-stuck stop`
ends the path there instead: with the sources as they are the Kojirou route then ends in `a007`
after 100 scenarios and 2 800 lines with `explore`, and in `a001_7` on 6 December after 593
scenarios and 19 900 lines with `days`.

### The `days` policy (the default)

`explore` alone does not follow the game's days: on the Kojirou path the day movies of the 4th to
6th never played and the 3rd took 9 600 lines (below: what it gives now). Four changes make the
path run the days in order:

1. **A later day comes last.** A look-ahead that runs into a day movie (`12xx`, `0x8046`) or a
   date screen of a later day than the path is on stops there: what follows belongs to the next
   day and is not counted. Such an entry (itself, or in a submenu or at the place it moves to) is
   taken only when no entry has anything new on this day; of those, the one that shows the day
   itself first. The menu log calls the pick `later day`.
2. **Something new on the way first.** Of the entries that move on, those that show something new
   before the next menu come first (the most first), then the others by all they would show. With
   "the most in total" alone, two places that each lead to the other (the `a007` ↔ `a002` streets)
   pointed at each other for what lay beyond both; the path went back and forth until the entry's
   repeat limit (40) was used up, and that hid the way on (`a007` → `a023`, the 3rd's evening).
3. **Walking, not teleporting.** When the outermost menu has nothing new, the interpreter searches
   the choices a player could make from there (breadth first, backing out of submenus included,
   at most 6 000 choices and 40 in a row) for the nearest one that shows something new on this
   day, and makes them: the path goes there through the places in between (report `walks` and
   `walkList`, menu log `walk`). A road to a later day is taken only when the whole reachable part
   has nothing on this day. Places are told apart by where the path is, not by its flags (with
   them every look at a place is a new state and the search does not end), so the flags along a
   road are those of the shortest way there. Only when there is no road does the path teleport,
   and then to a place that does not move on to a later day if there is one.
4. **Not back in time.** A look-ahead that shows a day movie of this day or an earlier one again
   stops there too: such an entry is taken only when nothing else is left, a road through it is
   no road, and a teleport goes there last. (The ending's look back at every day in `c02` does
   not count.) The 5th and 6th no longer play twice.

**Zapping.** The two sides share the flags F0–F255, and each needs some the other sets: Kojirou's
foreign school on the 4th (`a014`) waits for `F14`, which only Marina's `b014` sets. The game is
played by switching sides. So `days` plays the other side alongside, from its own start (`b01` for
Kojirou's route, `a01` label 0 for Marina's), with the same flags and writing nothing: before the
path moves on to a later day, and when it is stuck, the other side plays until it would go past the
path's day (or is stuck itself, waiting for a flag of this side); when that changed a flag, the
path looks again. The report's `zapping` gives the other side's day, lines, pauses, walks and
teleports. `--no-zapping` turns it off; the Kojirou route then has no 5th and 6th (1203, 1204,
1207, 1209), and on Marina's the 6th comes after the 7th.

Every run is bounded: instruction, line, move and per-instruction visit caps, and a repeat limit
per entry, so a hostile or merely looping scenario ends with a reason instead of hanging.

### Choosing by hand

`--choices` takes a JSON list. The n-th entry answers the n-th menu that comes up (the real
decisions only, not the look-aheads). Each entry is a 1-based number among the entries the menu
shows (those whose conditions hold, as a player would see them), a text (exact, else a part of
an entry's name), `"auto"` or `null` (let the policy decide), or `"cancel"` (back out). The report's
`menuChoices` lists the first 200 menus with what was shown and what was picked; copy from it to
steer a run:

```json
[2, "話す", "auto", "cancel", "茜"]
```

A selection that matches nothing is noted in the report and the policy takes over.

## What the script contains

- **Characters** use the game's three-letter codes from `キャラクターコード表.tsv` as ids: `koj`,
  `mar`, `kyo`, `yay`, `may`, `pur` (プリン/プリシア, voice code `pul`, sprite code `pur`), `aka`,
  `nik`, `hon`, `mid`, `mat`, `kat` (鈴木/桂木), `kag`, `sir`, `aqu`, `gul`, `dep`, `kou`, `uwe`. A
  name the table pins to one person maps to them; a vague name (声, 目の前の男, 女性の声…) and a
  compound one (孔・二階堂) takes the person from the voice file's code (`kyo0042` is Kyoko);
  anyone else (サラリーマン, 警官…) gets an id `n001`… with that name. `narrator` ("語り")
  speaks unspoken text; `sfx` ("擬音") owns the onomatopoeia lines that play a sound effect as
  their voice. The display name is the most used plain name (鈴木 or 桂木, プリン or プリシア).
- **Scenes** start at a change of background (`0x8014`), of music (`0x8026`, or a stop) or of the
  scenario's scene, never mid-picture; the same picture set again does not split. A new story day
  and the move into `ab01`/`c02` also start a scene. `pov` is `koj` or `mar` by the current side;
  `when` and `requires` are described under "Story time" below. A picture that cannot be found keeps the previous scene's.
- **Lines** are `dialogue`, `narration` (no speaker in the game), or `monologue` (unspoken text with
  a character's voice file, and a dialogue line that is wholly in （…）). `voiceFile` is the line's
  own voice. When a command played a sound effect just before a line it is that line's `sfx`; a
  sound with no line after it is a `direction`. Text has its `《》` changed to `〈〉` so the script
  does not read it as ruby. A wait (`0x802F`, frames) adds to the previous line's `pauseAfter`.
- **Expression**: the sprite shown (`0x8048`) or swapped (`0x804B`) for the speaker is the line's
  `expression`, named like the sprite file without the code (`aka03ad` → `03ad`); the character's
  `expressions` hold only the ones used.
- **Report** (`--report`): `endedBecause`, counts (scenes, lines, voiced lines…), `unknownOps` with an
  example each (an instruction not in the table; none for any of the 87 scenarios), the cosmetic
  instructions ignored and how often, `missingAssets` by kind, `movies`, the teleports, the
  scenarios visited, how many of the game's text instructions the path ran, notes, and the menu log.

## Story time, prerequisites and episodes

Every scene gets a `when` of the form `D03-0012`: the story day (3 December) and the scene's
number within that day, so the text sorts in time order and `script.validate` can check it. The
day moves on when the path shows a **date screen** (a background `1203md`…, from `日付画面`) or a
**day movie** (`1202`…`1209`, from `日付表示`); a new day always starts a new scene. Scenes before
the first date are day 0 (`D00-…`). A date earlier than the current one does not move time back
(the ending, `c02`, shows the date screens of 2 to 7 December again after the 9th). The report's
`dateMarkers` lists every date the path showed and whether it moved the day; `days` gives the
scenes and lines of each day.

Where the two sides meet (`ab01`, then `c02`) also starts a new scene, and the first scene there
`requires` the last scene before it. In one side's script that always holds; `merge_routes.py`
adds the other side's last scene (below). The report's `finale` names both.

`--episodes` cuts the script at day boundaries. Episode n is `<out stem>.epNN.fcscript.json`, with
the `title` 「小次郎編 第 1 話 12月3日」 (「序章」 for day 0, 「（2）」 for the second part of a day cut
by `--episode-lines`), its own `route`, and only the `characters` it uses (whole entries, so a line
without an expression falls back to the same picture as in the whole script). A `requires` that
points into an earlier episode is dropped there (that episode is shown first). A scene is never
cut, so a scene longer than `--episode-lines` is an episode of its own. The report's `episodes`
lists the files with their lines.

### What the days look like on the current path

With the `days` policy both paths follow the game's days: every day movie plays once, in order, and
moves the day (`dateMarkers`: Kojirou 1203, 1204, 1205, 1206, 1207, 1209; Marina 1202 to 1207 and
1209; all `applied`, only the ending's date screens of the 2nd to 7th are not). The days come out
as:

| | days (lines) |
|---|---|
| Kojirou | 12月3日 5 876, 12月4日 5 801, 12月5日 5 078, 12月6日 4 788, 12月7日 7 520, 12月9日 1 014 |
| Marina | 12月2日 1 191, 12月3日 4 043, 12月4日 5 451, 12月5日 3 634, 12月6日 4 938, 12月7日 3 509, 12月9日 1 014 |

(The 8th has no day movie on either side: the ship runs on to the 9th's 1209 in `ab01`.) With
`--policy explore` the Kojirou path had only the 3rd (9 616 lines, with the 4th to 6th mixed in by
teleports), the 7th (8 569) and the 9th, and the Tristan arrival (1207) played four times; on
Marina's the 6th came after the 7th.

A day is still long: Kojirou's 3rd, 5 876 lines × about 2.65 s, is about 4 h 20 min. For watchable
episodes cut by length: `--episode-lines 600` gives 58 Kojirou episodes (44 Marina); the first is
the prologue in `a01`, 4 scenes and 585 lines (about 26 minutes). The day episodes, both whole
scripts and the merged order below pass `script.validate` with no errors (the only warnings are the
usual "no recording and the speaker has no voice").

## Merging the two sides

`tools/eve_burst/merge_routes.py` puts the Kojirou and the Marina script into one viewing order:

```bash
python tools/eve_burst/merge_routes.py --kojirou out/kojirou.fcscript.json --marina out/marina.fcscript.json \
    --out out/both.fcscript.json --report out/both.report.json
python tools/eve_burst/merge_routes.py --a "out/kojirou.ep*.fcscript.json" --b "out/marina.ep*.fcscript.json" \
    --out out/both.fcscript.json
```

- **Day by day.** For each story day (from `when`): side A's scenes of that day, then side B's
  (`--order ba` the other way round), each side in its own order. A day only one side has comes
  as it is (Marina's 2 December first).
- **Where the sides meet** (`ab01`, `c02`: the first word of a scene's title, `--finale-scenarios`)
  comes once, after everything else. Both sides have the same text there (all 616 + 656 lines are
  equal), so side A's copy is kept (`--finale b` or `both` to change that). Its first scene
  `requires` the last scene before it on **both** sides.
- **Prerequisites.** The order is then made to satisfy every `requires`: a scene waits until what
  it requires has been shown, nothing else moves. A circle is left as it is and reported.
- **Ids** get the side's prefix (`--prefix-a k`, `--prefix-b m`: `k-s0001`, `k-s0001-001`), so the
  two sides never collide. A line's audio is named by its id, so the merged script is synthesized
  under its own names. The game's character codes (`koj`, `mar`, `kat`…) are one person on both
  sides (A's entry wins; B's expressions and a missing voice are added). Numbered minor characters
  (`n001`…) are the same person only when the name is the same; otherwise B's is renamed
  (`m-n004`). The report's `renamedCharacters` lists the changes.
- **Story time** becomes `D07-02-0013`: the day, the stretch of that day (01 = the first side's,
  02 = the other's…), the scene within the stretch, so it still sorts in time order.
- **Paths** (backgrounds, music, voices, effects, pictures, voice references) are rewritten to be
  right from the folder of `--out`; absolute ones stay as they are.
- Either side may be several files (the episodes, in order; a pattern is expanded by the tool).
  Bad input (not JSON, not a script, a result over 10 000 scenes or 200 000 lines…) is refused with
  a message and exit code 2.

With the current paths the merged order is Marina 12月2日 (1 191 lines), then for each of the 3rd to
the 7th Kojirou's day and Marina's (3rd 5 876 + 4 043, 4th 5 801 + 5 451, 5th 5 078 + 3 634, 6th
4 788 + 4 938, 7th 7 384 + 3 373), then the meeting part (136 lines on the 7th, 1 014 on the 9th):
2 728 scenes and 52 707 lines; 63 scenes of Marina's copy of the meeting part are left out.

## A series of episodes

`tools/eve_burst/episode_plan.py` cuts the merged script into a series of a set length and writes
a draft of each episode to adapt:

```bash
python tools/eve_burst/episode_plan.py --script out/both.fcscript.json --out-dir out/series --stem eve \
    --episodes 29 --minutes 27 --final-minutes 60
```

1. **Trim** what can go without reading it: a passage shown again (`--min-run`, 3 or more spoken
   lines in a row whose speaker and text were all shown before, directions in between not counted:
   the same conversation from the other side, a corridor walked twice) and an unrecorded line of
   dots only (「‥‥。」; it becomes 0.6 s more pause on the line before, `--keep-dots` keeps them). A
   scene left empty goes, and what required it requires the scene before it.
2. **Estimate** each line as `anime.build` lays it out: a recording, a sound effect or a movie
   (`video`, from its `mvhd` header) by its own length, an unrecorded line by its spoken characters
   at `--cps` 7 (at least 0.8 s), then its `pauseAfter` or `--pause` 0.35, and `--scene-gap` 0.6 per
   scene.
3. **Plan.** The target (`--episodes` − 1 regular episodes of `--minutes`, plus the last of
   `--final-minutes`) over the trimmed length gives one keep ratio for the series. The last episode
   is the meeting part and the ending, with the end of the last day of each side before it (in
   proportion to the sides) as far as it fits. The rest goes by day: each day gets episodes in
   proportion to its length (a day shorter than half an episode goes with the next one; with fewer
   episodes than days the shortest neighbours join). Within a day, each side's part is cut into
   whole episodes of about equal length, between scenes, and the episodes alternate between the
   sides by where they fall in their side's day, so each regular episode has one viewpoint, as the
   game switches sides.
4. **Write** `<stem>.ep01.fcscript.json`… (title 「第 3 話 12月3日（まりな）」, the characters it uses,
   paths right from `--out-dir`, `when` as `D03-03-0012`: day, episode, scene), `<stem>.plan.json`
   (per episode: days, viewpoints, scenes, lines and recorded lines, draft and target minutes, keep
   ratio, scenarios) and `<stem>.plan.md` (the same as a table, with the rules below for whoever
   adapts the episodes).

Nothing is rewritten by the tool; it only drops by the two rules of step 1.

With the current paths (both sides merged, 52 707 lines): estimated 43.2 hours, 35.0 after trimming
(9 859 lines shown again, 2 305 dot lines, 697 scenes left empty). 29 episodes of 27 minutes, the
last of 60, keep about 39 %: episode 1 is Marina's 2 December (25 scenes, 1 095 lines, 59 minutes
of draft, keep 46 %), then 6 episodes for the 3rd, 7 for the 4th, 5 for the 5th, 5 for the 6th, 4
for the 7th, and the last (210 scenes, 149 minutes of draft). Drafts run 57 to 87 minutes (keep 31
to 47 %). All 29 pass `script.validate` with no errors. On the 7th the Kojirou path goes back to
land scenarios after the ship (see "Open problems"), so episode 28 ends and the last episode starts
with them.

### Adapting an episode

The adapter (a person or an LLM session) edits a copy of a draft by these rules:

- A **recorded** line (`voiceFile`) may only be deleted. Its `text`, `speaker`, `kind` and
  `voiceFile` stay (they must match the recording); a `reading` may be added.
- An **unrecorded** line (most of the protagonists' lines, the narrator) may be rewritten, and
  several may be merged into one, as long as the story does not change: no fact, clue,
  relationship, foreshadowing or "who learns what" is changed. A rewritten line keeps its id and
  gets `"generated": true`; a merged line keeps the first line's id and the others go.
- A **new** line (a bridge) gets a new id (`<scene id>-g01`…), `"generated": true` and no recording.
- Nothing is **moved**: lines and scenes stay in the draft's order. Scenes may be deleted, not added.
- The length lands in the target (24–30 minutes, the last about 60).

`tools/eve_burst/adapt.py` makes this practical for an LLM: `view` prints a draft as one line per
script line (`<id> <speaker> <R or -> <text>`, R = recorded) with the estimated minutes per scene,
and `apply` takes a short edit list (`delete` with ids and ranges `a..b`, `deleteScenes`, `rewrite`,
`merge`, `insert` before/after a line, `reading`, `pause`; `voiceOnly` plays a recording whose text
is not the line, such as the ending's voices over a bare name, as a direction with the recording as
its sound, no subtitle; `speaker` gives an unrecorded line of a numbered minor character to another
character of the episode), writes the adapted episode with the
`generated` marks and new ids (`<scene>-g01`), and checks it (exit 0 / 1 rule or length broken /
2 edits unusable):

```bash
python tools/eve_burst/adapt.py view --script out/series/eve.ep01.fcscript.json --out ep01.txt
python tools/eve_burst/adapt.py apply --script out/series/eve.ep01.fcscript.json --edits ep01.edits.json \
    --out out/adapted/eve.ep01.fcscript.json --min 24 --max 30
```

All 29 drafts were adapted this way by parallel Opus sessions (one per episode, from one written
brief), and one more episode was added afterwards: the Kojirou path's night of 6 December (with
Kyoko at the embassy) came after the ship on the 7th, so the last draft held it and its adapter cut
it; it became episode 24 of 30. Then the joins between episodes were checked day by day (five more
sessions, each owning one day's episodes) and fixed by changing the edit lists: lines cut in one
episode that a later one relies on were put back, times of day and places in the opening narration
corrected, gaps bridged. Every episode is rebuilt from its edit list and checked again: 30
episodes, about 14.5 hours (26.5–29.0 minutes each, the last 62.9), all passing
`check_adaptation.py` and `script.validate`. The checker accepts the two exceptions above
(`voiceOnly`: a direction whose sound is the line's own recording; `speaker`: a numbered minor
character named) and nothing else.

`tools/eve_burst/check_adaptation.py` checks an adapted episode against its draft:

```bash
python tools/eve_burst/check_adaptation.py --source out/series/eve.ep01.fcscript.json \
    --edited out/adapted/eve.ep01.fcscript.json --min 24 --max 30
```

It prints kept, rewritten, new and deleted lines, recorded lines kept and deleted, scenes deleted,
the draft's and the edit's minutes (the same estimate), and every broken rule; it exits with 1 when
a rule is broken and 2 when a file cannot be read. Recordings are compared as files, so an edit in
another folder may write their paths differently. Run `script.validate` on the edit as well.

## What a full route gives

With the dump as it is now (voices missing would be listed by the report, not lost in the script):

| | Kojirou (`a01`) | Marina (`b01`) |
|---|---|---|
| Ends | game clear in `c02` | game clear in `c02` |
| Scenes | 1 672 | 1 119 |
| Lines | 30 077 (29 985 spoken) | 23 780 (23 713 spoken) |
| Lines with a voice in the game | 8 808 (all found) | 6 982 (all found) |
| Scenarios entered / distinct | 852 / 44 | 612 / 45 |
| Walks / teleports | 60 / 22 | 25 / 17 |
| Unknown instructions | 0 | 0 |

(With `--policy explore`: Kojirou 1 358 scenes, 19 199 lines, 71 teleports; Marina 944 scenes,
12 272 lines, 55 teleports.)

Both outputs pass `script.validate` with no errors (the only warnings are "no recording and the
speaker has no voice", which `script.pickVoices` then answers from the game's own voice files).

## What is lost

- **It is one path, not the game.** About 26 700 of the game's 52 000 text instructions run on the
  Kojirou path and 21 100 on Marina's (many of the rest are the other half of a branch). The order of scenes follows the policy,
  not a human player; teleports skip the puzzles a player would have to solve, and place and time
  can jump (the map position instruction `0x804C` is ignored).
- **Pictures:** one background per scene (no fades, no black screens), no positions, one character at a
  time (the speaker's sprite; a speaker who is not on screen shows their first expression),
  sub-CGs and movies are not placed by default.
- **Sound:** a line with two voice files (28 on the Kojirou path) uses the first; the voice-stop
  and "wait for the sound" instructions are ignored; the BGM start/stop is all that is kept of music.
- **Interactive parts:** the choice itself is gone (the video just shows what was chosen), the
  hacking-terminal and map screens are narration and nothing.
- **Mouths and timing** come from the audio as usual; the game's own click-to-advance flags
  (`n s w k` at the end of a message) are dropped.

## Open problems

- **Teleports are left** (22 on the Kojirou path, 17 on Marina's), where no road is found: often
  late on the 6th and on the ship. A teleport skips what a player would do there, and the place it
  lands in decides by its flags what to show. The road search ignores flags when it tells places
  apart, so a road that needs a flag set on the way (talk to A, then go to B) can be missed; a
  scenario entry with nothing to show (`0x8000`) and a road back in time are dead ends for it.
- **The other side is played by the same policy**, so what it sets, and when, is that policy's
  guess at a player's order. It can get ahead of the path by part of a day: it stops before a day
  change it chooses, but not before a day movie a scenario plays as it starts. Each route is
  written on its own, so each side is played twice (for its own script and alongside the other),
  and the two playings need not agree.
- **The 7th goes back to land.** On the Kojirou path, after the long ship stretch of the 7th
  (`a034`, 458 lines) the path returns to land scenarios (`a008_2`, `a026`, `a011_2`, `a012`,
  `a013`, `a017`, `a005`, `a006`, `a020_2`…, partly by teleport: `a009`→`a011_2`→`a012`→`a013`→`a017`)
  before the ship again and `ab01`. Whether a player sees them there is not checked.
  `episode_plan.py` keeps the path's order, so episodes 28 and 29 carry it; whoever adapts them
  should know.
- **Days are long** (up to 7 520 lines): cut with `--episode-lines`, or plan a series with
  `episode_plan.py`. The ship on the 7th still goes
  back and forth between its rooms (`a030` ↔ `a033`, `a029` ↔ `a035`, about 120 short moves), as
  the game's own corridors do.

- Menu semantics are inferred from the data, not from the engine: *cancel* as the way out of a
  submenu, "jump drops all open menus", the instruction after a menu never running. If a route
  looks wrong at a menu, steer it with `--choices`.
- The wait unit is assumed to be 60 per second (the values 30, 60, 120 suggest it); `--fps` changes it.
- Teleporting is a convenience, not the game. `--on-stuck stop` gives the honest but short path.
- `pul` and `pur` are treated as one person; `kat` is 鈴木 in Marina's story and 桂木 in
  Kojirou's (the table calls it a guess); the two 孔 voices (`dep`, `kou`) are separate characters.
- Unvoiced lines and the narrator are left to the synthesizer; onomatopoeia lines keep the game's
  sound as their voice, which makes `pickVoices` pick a sound effect as the `sfx` character's
  reference (harmless, but ignore it).

## Tests

`tools/eve_burst/tests/test_eve_to_script.py` (`python -m unittest discover -s tools/eve_burst/tests`):
hand-made scenarios in a temp directory with a fake asset resolver (speakers and voices, scene
splits, missing files, sprites, waits, sound effects, flags and else-if chains, menus, policies and
explicit choices, nested menus, the stuck path and teleporting, missing scenarios, unknown
instructions, hostile files, caps and loops, the command line, story time from date screens and day
movies, the requirement where the sides meet, episodes by day and by line cap and their files; the
`days` policy: a later day taken last, the road search and a road to a later day, walking where
`explore` teleports, no teleport back in time, the other side's shared flag, the other side
stopping at the end of the path's day, a missing other side), and, when the dump is present, both
real routes (ending reached, no unknown instructions, every day movie once and in order, story time
in order, valid by a hand check of the format and by the repo's own `script.validate` if
`target/debug/filmcraft-cli` exists).

`tools/eve_burst/tests/test_episode_plan.py`: splitting whole parts and cutting runs, the length of
lines (recordings, estimated lines with ruby and `reading`, pauses, directions with a sound or a
movie) and hostile media files, trimming (a passage shown again, one too short to count, dots
becoming a pause, an emptied scene and what required it), the plan (every scene once, each side in
order, one viewpoint per episode alternating, the last episode, short days, fewer episodes than
days, bad requests), the written files and the command line; `check_adaptation.py` on a good
adaptation, on every broken rule, on recordings written from another folder and by its exit codes;
and, with the dump, both routes planned into 29 valid episodes.

`tools/eve_burst/tests/test_merge_routes.py`: the day-by-day order, whose meeting part is kept and
what it requires, the other order, episode files and patterns, prerequisites that move a scene, a
circle of them, characters (codes shared, minor ones by name), paths from another folder, bad and
odd input, the command line, the repo's `script.validate` on a merged script, and both real routes
merged.
