"""The screen fields: eve_to_script.py --screen writes them, screen_fields.py copies them into an episode.

Everything here is made up in a temp directory (no real data)."""
import copy
import json
import os
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, ".."))
sys.path.insert(0, HERE)
import eve_to_script as E  # noqa: E402
import screen_fields as S  # noqa: E402
from test_eve_to_script import Base, FakeAssets, Sc  # noqa: E402


def png(path, w, h):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + (13).to_bytes(4, "big") + b"IHDR" + w.to_bytes(4, "big") + h.to_bytes(4, "big"))


class EventAssets(FakeAssets):
    """Backgrounds whose name starts with `ev` come from the event picture folder."""

    def background(self, name):
        return f"{E.EVENT_CG_FOLDER}/{name}.png" if name.startswith("ev") else super().background(name)


def show(sc, name, slot):
    return sc.add(0x8048, slot=slot, asset={"type": "立ち絵", "name": name})


def movie(sc, name, op=0x8008):
    return sc.add(op, asset={"type": "ムービー", "name": name})


def inset(sc, name, x, y):
    return sc.add(0x8036, args=[1, x, y, 0], asset={"type": "サブCG", "name": name})


class Converter(Base):
    def test_movies_are_lines_of_their_own_and_no_id_changes(self):
        a = Sc("a01").label()
        movie(a, "1203", 0x8046).bg("b1").say("小次郎", "1")
        movie(a, "scene03").say("小次郎", "2").bg("b2").say("小次郎", "3")
        movie(a, "scene10").clear_game()
        plain, _ = self.convert(a)
        s, rep = self.convert(a, screen=True)
        self.assertEqual([[x["id"] for x in sc["lines"]] for sc in s["scenes"]],
                         [["s0001-v01", "s0001-001", "s0001-v02", "s0001-002"], ["s0002-001", "s0002-v01"]])
        videos = [x for x in self.lines(s) if "video" in x]
        self.assertEqual([x["video"] for x in videos], ["movie/1203.mp4", "movie/scene03.mp4", "movie/scene10.mp4"])
        self.assertTrue(all(x["kind"] == "direction" and "text" not in x for x in videos))
        strip = lambda sc: [(x["id"], x.get("text")) for x in sc["lines"] if "video" not in x]
        self.assertEqual([strip(sc) for sc in s["scenes"]], [strip(sc) for sc in plain["scenes"]])
        self.assertEqual(rep["counts"]["videoLines"], 3)
        self.assertFalse(any("video" in x for x in self.lines(plain)))

    def test_a_missing_movie_is_left_out(self):
        a = Sc("a01").label().bg("b1").say("小次郎", "1")
        movie(a, "gone").say("小次郎", "2").clear_game()
        s, rep = self.convert(a, screen=True, assets=FakeAssets(missing={"gone"}))
        self.assertEqual([x["id"] for x in self.lines(s)], ["s0001-001", "s0001-002"])
        self.assertEqual(rep["missingAssets"]["movie"], ["gone"])

    def test_inset_comes_with_its_place_and_goes_with_a_clear(self):
        png(os.path.join(self.root, "sub", "s13.png"), 272, 247)
        a = Sc("a01").label().bg("b1").say("小次郎", "before")
        inset(a, "s13", 149, 11).say("小次郎", "shown").say("小次郎", "still")
        a.add(0x8012, args=[1]).say("小次郎", "gone").say("小次郎", "nothing")
        inset(a, "nosize", 10, 10).say("小次郎", "centred")
        a.bg("b2").say("小次郎", "next scene")
        a.clear_game()
        s, rep = self.convert(a, screen=True)
        ln = {x["text"]: x for x in self.lines(s)}
        self.assertNotIn("subCg", ln["before"])
        self.assertEqual(ln["shown"]["subCg"], "sub/s13.png")
        x, y = ln["shown"]["subCgAnchor"]
        self.assertAlmostEqual(x, 0.5 + ((149 + 136) / 640 - 0.5) * (640 / 448) / (16 / 9), places=3)
        self.assertAlmostEqual(y, (11 + 123.5) / 448, places=3)
        self.assertNotIn("subCg", ln["still"])
        self.assertEqual(ln["gone"]["subCg"], "")
        self.assertNotIn("subCg", ln["nothing"])
        self.assertEqual(ln["centred"]["subCg"], "sub/nosize.png")
        self.assertNotIn("subCgAnchor", ln["centred"])  # no picture to measure: the middle of the frame
        self.assertEqual(ln["next scene"]["subCg"], "sub/nosize.png")  # no clear: still there, said again for the new scene
        self.assertEqual(rep["counts"]["subCgChanges"], 4)
        tall, _ = self.convert(a, screen=True, frame_aspect=1.0)
        x, y = {t["text"]: t for t in self.lines(tall)}["shown"]["subCgAnchor"]
        self.assertAlmostEqual(x, (149 + 136) / 640, places=3)
        self.assertAlmostEqual(y, 0.5 + ((11 + 123.5) / 448 - 0.5) * 1.0 / (640 / 448), places=3)

    def test_sides_of_a_two_shot(self):
        a = Sc("a01").label().bg("b1")
        show(a, "aka00ad", 2).say("茜", "centre")
        show(a, "aka00ad", 3)
        show(a, "yay00ad", 1)
        a.say("茜", "right").say("弥生", "left").say("小次郎", "hero")
        a.add(0x8049, fade_ms=0)
        show(a, "aka00ad", 2).say("茜", "back")
        a.say("茜", "again")
        a.clear_game()
        s, rep = self.convert(a, screen=True)
        self.assertEqual([(x["text"], x.get("slot")) for x in self.lines(s)],
                         [("centre", None), ("right", 3), ("left", 1), ("hero", None), ("back", 2), ("again", None)])
        self.assertEqual(rep["counts"]["slots"], 2)
        plain, _ = self.convert(a)
        self.assertFalse(any("slot" in x or "_slot" in x for x in self.lines(plain)))
        self.assertFalse(any("_slot" in x for x in self.lines(s)))

    def test_fade_out_fades_the_next_scene_in(self):
        a = Sc("a01").label().bg("b1").say("小次郎", "1")
        a.add(0x800F, args=[8]).add(0x800E, args=[4]).say("小次郎", "same picture")  # nothing to fade to
        a.bg("b2").say("小次郎", "cut")
        a.add(0x800F, args=[8]).add(0x800B, asset={"type": "効果音", "name": "se01"}).bg("b3").add(0x800E, args=[4]).say("小次郎", "faded")
        a.add(0x8010, args=[0]).bg("b4").add(0x800B, asset={"type": "効果音", "name": "se02"}).bg("b5").say("小次郎", "white")
        a.bg("b6").say("小次郎", "once")
        a.clear_game()
        s, rep = self.convert(a, screen=True)
        by = {sc["background"]: sc.get("transition") for sc in s["scenes"]}
        # b3: the sound before the cut is the last line of b2 and does not use the fade up;
        # b4: the scene of a sound alone takes the fade, so b5 after it cuts
        self.assertEqual(by, {"bg/b1.png": None, "bg/b2.png": None, "bg/b3.png": "fade", "bg/b4.png": "fade", "bg/b5.png": None, "bg/b6.png": None})
        self.assertEqual(rep["counts"]["fades"], 2)
        plain, _ = self.convert(a)
        self.assertFalse(any("transition" in sc for sc in plain["scenes"]))

    def test_no_sprites_over_an_event_picture(self):
        a = Sc("a01").label().bg("ev1").say("茜", "in the picture")
        a.bg("ev2")
        show(a, "aka00ad", 2).say("茜", "drawn over it")
        a.bg("b1").say("茜", "plain")
        a.clear_game()
        s, rep = self.convert(a, screen=True, assets=EventAssets())
        self.assertEqual([sc.get("hideCharacters") for sc in s["scenes"]], [True, None, None])
        self.assertEqual(rep["counts"]["hiddenScenes"], 1)

    def test_command_line(self):
        a = Sc("a01").label().bg("b1")
        movie(a, "scene01").say("小次郎", "1").clear_game()
        from test_eve_to_script import write_scenarios
        write_scenarios(self.root, a)
        self.assertEqual(E.main(["--root", self.root, "--start", "a01", "--out", self.out, "--screen"]), 0)
        with open(self.out, encoding="utf-8") as f:
            ids = [x["id"] for sc in json.load(f)["scenes"] for x in sc["lines"]]
        self.assertEqual(ids, ["s0001-001"])  # the temp folder has no movie file: the line of the movie is left out


def line(i, speaker="aka", text="x", **kw):
    return {"id": i, "kind": "dialogue", "speaker": speaker, "text": text, **kw}


def scene(i, lines, **kw):
    return {"id": i, "lines": lines, **kw}


def script(scenes, pictured=("aka", "koj", "yay")):
    chars = {c: {"name": c, "expressions": {"normal": {"image": f"{c}.png"}}} for c in pictured}
    chars["narrator"] = {"name": "語り"}
    return {"version": 1, "characters": chars, "scenes": scenes}


class Fields(unittest.TestCase):
    def setUp(self):
        self._td = tempfile.TemporaryDirectory()
        self.dir = self._td.name

    def tearDown(self):
        self._td.cleanup()

    def donor(self, scenes, prefix="k"):
        return S.Donor(prefix, script(scenes), os.path.join(self.dir, "donor"))

    def apply(self, target, donors, siblings=(), **kw):
        return S.apply(target, donors, list(siblings), os.path.join(self.dir, "out"), **kw)

    def test_scene_fields_and_fades_over_cut_scenes(self):
        d = self.donor([
            scene("s1", [line("s1-1")], when="D03-0001"),
            scene("s2", [line("s2-1")], when="D03-0002", transition="fade"),
            scene("s3", [line("s3-1")], when="D03-0003", hideCharacters=True),
            scene("s4", [line("s4-1")], when="D03-0004"),
            scene("s5", [line("s5-1")], when="D03-0005"),
        ])
        m = self.donor([scene("s1", [line("s1-1")], when="D03-0001")], prefix="m")
        t = script([
            scene("k-s1", [line("k-s1-1")], hideCharacters=True),  # wrongly hidden before: put right
            scene("k-s3", [line("k-s3-1")]),  # the fade into s2 was cut with it: this scene fades in
            scene("k-s4", [line("k-s4-1")], transition="fade"),  # an old fade the donor does not have
            scene("m-s1", [line("m-s1-1")]),  # the other side
            scene("k-s5", [line("k-s5-1")]),
            scene("extra", [line("extra-1")]),  # not in any donor: left as it is
        ])
        rep = self.apply(t, [d, m])
        sc = {x["id"]: x for x in t["scenes"]}
        self.assertEqual([sc[i].get("transition") for i in ("k-s1", "k-s3", "k-s4", "m-s1", "k-s5", "extra")],
                         ["fade", "fade", None, "fade", "fade", None])
        self.assertEqual([sc[i].get("hideCharacters") for i in ("k-s1", "k-s3", "k-s4")], [None, True, None])
        self.assertEqual((rep["scenes"], rep["scenesMatched"], rep["fades"], rep["hidden"]), (6, 5, 4, 1))
        t2 = script([scene("k-s1", [line("k-s1-1")])])
        self.apply(t2, [d], fade_first=False)
        self.assertNotIn("transition", t2["scenes"][0])

    def test_sides_and_the_free_side(self):
        d = self.donor([scene("s1", [
            line("s1-1"), line("s1-2", slot=3), line("s1-3", "koj"), line("s1-4", "koj"), line("s1-5"),
            line("s1-6", "yay", slot=1), line("s1-7", "koj"), line("s1-8", slot=2), line("s1-9", "koj"),
        ])])
        t = script([scene("k-s1", [line(f"k-s1-{n}", who, slot=9) for n, who in
                                   enumerate(["aka", "aka", "koj", "koj", "aka", "yay", "koj", "aka", "koj"], 1)]
                          + [line("k-s1-g1", "narrator", kind="narration"), {"id": "k-s1-d", "kind": "direction", "speaker": "koj"}])])
        rep = self.apply(t, [d])
        self.assertEqual([x.get("slot") for x in t["scenes"][0]["lines"]],
                         [None, 3, 1, None, None, 1, None, 2, None, None, None])
        # 3: the hero takes the free side and stays (4); 6: yay takes the left from him; 7: both sides are
        # taken, so he goes to the centre and the two-shot ends; 8: the donor's centre, with nobody at a side
        self.assertEqual((rep["slots"], rep["freeSide"]), (2, 1))

    def test_inset_follows_the_donor_over_cut_and_added_lines(self):
        d = self.donor([scene("s1", [
            line("s1-1"), line("s1-2", subCg="cg/a.png", subCgAnchor=[0.25, 0.3]), line("s1-3"), line("s1-4", subCg=""),
            line("s1-5"), line("s1-6", subCg="cg/b.png", subCgAnchor=[2, "x"]), line("s1-7"),
        ]), scene("s2", [line("s2-1")])])
        t = script([scene("k-s1", [line("k-s1-1"), line("k-s1-3", subCg="old.png"), line("k-s1-g1"), line("k-s1-5"), line("k-s1-7")]),
                    scene("k-s2", [line("k-s2-1", subCgAnchor=[0.1, 0.1])])])
        rep = self.apply(t, [d])
        ln = {x["id"]: x for sc in t["scenes"] for x in sc["lines"]}
        want = os.path.relpath(os.path.join(self.dir, "donor", "cg", "a.png"), os.path.join(self.dir, "out")).replace("\\", "/")
        self.assertEqual(ln["k-s1-3"]["subCg"], want)  # it came up on the line that was cut
        self.assertEqual(ln["k-s1-3"]["subCgAnchor"], [0.25, 0.3])
        self.assertNotIn("subCg", ln["k-s1-g1"])  # an added line keeps the screen of the line before
        self.assertEqual(ln["k-s1-5"]["subCg"], "")
        self.assertTrue(ln["k-s1-7"]["subCg"].endswith("cg/b.png"))
        self.assertNotIn("subCgAnchor", ln["k-s1-7"])  # a bad anchor is not copied
        self.assertNotIn("subCg", ln["k-s2-1"])
        self.assertNotIn("subCgAnchor", ln["k-s2-1"])
        self.assertEqual((rep["insetsOn"], rep["insetsOff"]), (2, 1))

    def movies_donor(self):
        far = [line(f"s3-{n}") for n in range(1, 60)]
        return self.donor([
            scene("s1", [{"id": "s1-v01", "kind": "direction", "video": "mv/day3.mp4"}, line("s1-1"), line("s1-2"),
                         {"id": "s1-v02", "kind": "direction", "video": "mv/event.mp4"}, line("s1-3")], when="D03-0001"),
            scene("s2", [{"id": "s2-v01", "kind": "direction", "video": "mv/lost.mp4"}, line("s2-1")], when="D03-0002"),
            scene("s3", far, when="D03-0003"),
            scene("s4", [{"id": "s4-v01", "kind": "direction", "video": "mv/day4.mp4"}] + [line(f"s4-{n}") for n in range(1, 80)]
                  + [{"id": "s4-v02", "kind": "direction", "video": "mv/event.mp4"}, line("s4-80"),
                     {"id": "s4-v03", "kind": "direction", "video": "mv/ending.mp4"}], when="D04-0001"),
        ])

    def test_movies(self):
        d = self.movies_donor()
        t = script([
            scene("k-s1", [line("k-s1-g0", "narrator", kind="narration"), line("k-s1-2"), line("k-s1-3")]),
            scene("k-s3", [line("k-s3-55")]),
            scene("k-s4", [line("k-s4-g0", "narrator", kind="narration"), line("k-s4-70", slot=3), line("k-s4-80")]),
        ])
        rep = self.apply(t, [d])
        self.assertEqual([[x["id"] for x in sc["lines"]] for sc in t["scenes"]], [
            ["k-s1-v01", "k-s1-g0", "k-s1-2", "k-s1-v02", "k-s1-3"],  # before the line added in front
            ["k-s3-55"],  # the movie of s2 is 55 lines away: left out
            ["k-s4-v01", "k-s4-g0", "k-s4-70", "k-s4-80", "k-s4-v03"],  # the day's movie however far; the ending last
        ])
        self.assertEqual([v["id"] for v in rep["videosLeftOut"]], ["k-s2-v01"])
        self.assertEqual(rep["videosRepeated"], 1)  # event.mp4 a second time
        v = t["scenes"][0]["lines"][0]
        self.assertEqual((v["kind"], v["video"].replace("\\", "/").split("donor/")[-1]), ("direction", "mv/day3.mp4"))
        once = copy.deepcopy(t)
        again = self.apply(t, [d])
        self.assertEqual(t, once)  # running it again changes nothing
        self.assertEqual(len(again["videos"]), len(rep["videos"]))

    def test_a_movie_between_episodes_goes_to_the_one_with_the_next_line(self):
        d = self.movies_donor()
        ep1 = script([scene("k-s1", [line("k-s1-1"), line("k-s1-2")])])
        ep2 = script([scene("k-s1", [line("k-s1-3")]), scene("k-s3", [line("k-s3-1")])])
        ep3 = script([scene("k-s4", [line("k-s4-1"), line("k-s4-80")])])
        r1 = self.apply(copy.deepcopy(ep1), [d], [ep2, ep3])
        r2 = self.apply(copy.deepcopy(ep2), [d], [ep1, ep3, copy.deepcopy(ep2)])  # its own earlier stage among them
        r3 = self.apply(copy.deepcopy(ep3), [d], [ep1, ep2])
        self.assertEqual([[v["file"] for v in r["videos"]] for r in (r1, r2, r3)],
                         [["day3.mp4"], ["event.mp4", "lost.mp4"], ["day4.mp4", "event.mp4", "ending.mp4"]])
        alone = self.apply(copy.deepcopy(ep1), [d])
        self.assertEqual([v["file"] for v in alone["videos"]], ["day3.mp4", "event.mp4", "lost.mp4"])  # just past its last line; the ending is far

    def test_strange_input_does_not_raise(self):
        d = S.Donor("k", {"scenes": [None, {"id": ["x"], "lines": 3}, {"id": "s1", "transition": 5, "when": 7, "lines": [
            None, {"id": {"a": 1}}, {"id": "s1-1", "slot": True, "subCg": 9, "subCgAnchor": "no"}, {"id": "s1-1", "slot": 3}, {"id": "s1-v", "video": ""}]}]}, self.dir)
        self.assertEqual((d.count, d.slot, d.videos), (2, {}, []))  # s1-1 once, and the line with an empty `video`
        t = {"characters": [], "scenes": [3, {"id": ["x"], "lines": [None, {"id": ["y"], "speaker": {"z": 1}}, {"id": "k-s1-1", "speaker": 4}]}]}
        rep = self.apply(t, [d])
        self.assertEqual((rep["lines"], rep["matched"]), (2, 1))
        self.assertEqual(S.apply({}, [d], [None, 3], self.dir)["matched"], 0)
        self.assertEqual(S.parse_donor("k=a/b.json"), ("k", "a/b.json"))
        self.assertEqual(S.parse_donor("E:/a=b/c.json"), ("", "E:/a=b/c.json"))
        self.assertEqual(S.parse_donor("plain.json"), ("", "plain.json"))

    def test_command_line(self):
        dp = os.path.join(self.dir, "donor", "k.fcscript.json")
        os.makedirs(os.path.dirname(dp))
        with open(dp, "w", encoding="utf-8") as f:
            json.dump(script([scene("s1", [{"id": "s1-v01", "kind": "direction", "video": "mv/a.mp4"}, line("s1-1", slot=3)], transition="fade")]), f)
        tp = os.path.join(self.dir, "ep.fcscript.json")
        with open(tp, "w", encoding="utf-8") as f:
            json.dump(script([scene("k-s1", [line("k-s1-1")])]), f)
        out = os.path.join(self.dir, "out", "ep.screen.fcscript.json")
        rp = os.path.join(self.dir, "out", "rep.json")
        self.assertEqual(S.main(["--script", tp, "--donor", f"k={dp}", "--out", out, "--report", rp, "--siblings", os.path.join(self.dir, "*.fcscript.json")]), 0)
        with open(out, encoding="utf-8") as f:
            got = json.load(f)
        self.assertEqual([x["id"] for x in got["scenes"][0]["lines"]], ["k-s1-v01", "k-s1-1"])
        self.assertEqual(got["scenes"][0]["lines"][0]["video"], "../donor/mv/a.mp4")
        self.assertEqual(got["scenes"][0]["lines"][1]["slot"], 3)
        with open(rp, encoding="utf-8") as f:
            self.assertEqual(json.load(f)["matched"], 1)
        # another conversion's ids: refused, nothing written
        other = os.path.join(self.dir, "out", "other.json")
        self.assertEqual(S.main(["--script", tp, "--donor", dp, "--out", other]), 1)
        self.assertFalse(os.path.exists(other))
        self.assertEqual(S.main(["--script", os.path.join(self.dir, "none.json"), "--donor", dp, "--out", other]), 2)
        self.assertEqual(S.main(["--script", tp, "--donor", os.path.join(self.dir, "none.json"), "--out", other]), 2)


if __name__ == "__main__":
    unittest.main()
