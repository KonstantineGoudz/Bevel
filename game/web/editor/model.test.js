import test from "node:test";
import assert from "node:assert/strict";
import {
  HEX_SIZE, SHAPES, SHAPES_BY_ID, CATEGORIES,
  hexCenter, hexEdgeMidpoints, pixelToHex, nearestEdge,
  worldAnchors, hitTest, anchorSnap, gridSnap, snapPiece, rotate,
} from "./model.js";

const SQRT3 = Math.sqrt(3);
const EDGE_LEN = HEX_SIZE * SQRT3;
const near = (a, b, eps = 0.5) => assert.ok(Math.abs(a - b) <= eps, `${a} !~ ${b}`);

test("hex centres are axial", () => {
  const o = hexCenter(0, 0);
  near(o.x, 0); near(o.y, 0);
  const e = hexCenter(1, 0);
  near(e.x, HEX_SIZE * SQRT3); near(e.y, 0);
  const up = hexCenter(0, 1);
  near(up.x, HEX_SIZE * SQRT3 / 2); near(up.y, HEX_SIZE * 1.5);
});

test("pixel -> hex round-trips", () => {
  for (const [q, r] of [[0, 0], [3, -2], [-4, 5], [7, 1]]) {
    const c = hexCenter(q, r);
    const h = pixelToHex(c.x, c.y);
    assert.deepEqual(h, { q, r });
  }
});

test("nearest edge finds the edge you are standing on", () => {
  const d = (HEX_SIZE * SQRT3) / 2;
  // Just inside the hex, on the east edge.
  const e = nearestEdge(d - 6, 0);
  near(e.x, d); near(e.y, 0); near(e.angle, 0); assert.equal(e.edge, 0);

  // Just inside the north-east edge.
  const e2 = nearestEdge(d * 0.45, d * 0.78);
  near(e2.angle, Math.PI / 3);
});

test("edge angles fold into [0, π) so walls never flip", () => {
  const d = (HEX_SIZE * SQRT3) / 2;
  // The same physical edge, approached from the far side.
  near(nearestEdge(-d + 6, 0).angle, 0);
  near(nearestEdge(d, 0).angle, 0);
});

test("edge midpoints sit at half the circumradius squared", () => {
  const d = (HEX_SIZE * SQRT3) / 2;
  const mids = hexEdgeMidpoints(0, 0);
  assert.equal(mids.length, 6);
  for (const m of mids) near(Math.hypot(m.x, m.y), d);
});

test("a wall snaps end-to-end onto another wall", () => {
  const wall = SHAPES_BY_ID.wall;
  const a = { id: "a", shape: "wall", x: 0, y: 0, rot: 0 };
  const ghost = { id: "ghost", shape: "wall", x: EDGE_LEN - 2, y: 1, rot: 0 };

  const s = anchorSnap(ghost, wall, [a], 24);
  assert.ok(s, "expected an anchor snap");
  near(s.x, EDGE_LEN);
  near(s.y, 0);
  assert.equal(s.kind, "anchor");
});

test("a wall on the next hex edge does not weld across the corner", () => {
  const wall = SHAPES_BY_ID.wall;
  const a = { id: "a", shape: "wall", x: 0, y: 0, rot: 0 };
  // Rotated 60°, positioned so its left anchor sits exactly on a's right anchor.
  const off = rotate(-EDGE_LEN / 2, 0, Math.PI / 3);
  const ghost = {
    id: "ghost", shape: "wall",
    x: EDGE_LEN / 2 - off.x, y: -off.y, rot: Math.PI / 3,
  };
  const m = worldAnchors(ghost, wall)[0];
  near(m.x, EDGE_LEN / 2); near(m.y, 0);
  assert.equal(anchorSnap(ghost, wall, [a], 24), null);
});

test("grid snap orients walls to the edge and leaves props alone", () => {
  const d = (HEX_SIZE * SQRT3) / 2;
  const wallSnap = gridSnap({ x: d + 3, y: 2, rot: 1.234 }, SHAPES_BY_ID.wall);
  assert.equal(wallSnap.kind, "grid");
  near(wallSnap.rot, 0);

  const crate = { x: 7, y: -9, rot: 0.5 };
  const crateSnap = gridSnap(crate, SHAPES_BY_ID.crate);
  near(crateSnap.rot, 0.5);
  const center = hexCenter(crateSnap.q, crateSnap.r);
  near(crateSnap.x, center.x); near(crateSnap.y, center.y);
});

test("anchors win over the grid when they are in range", () => {
  const wall = SHAPES_BY_ID.wall;
  const a = { id: "a", shape: "wall", x: 0, y: 0, rot: 0 };
  const ghost = { id: "g", shape: "wall", x: EDGE_LEN - 3, y: 0, rot: 0 };
  const s = snapPiece(ghost, wall, [a]);
  assert.equal(s.kind, "anchor");

  const far = snapPiece({ id: "g", shape: "wall", x: 900, y: 900, rot: 0 }, wall, [a]);
  assert.equal(far.kind, "grid");
});

test("a crate clicks into a crate", () => {
  const crate = SHAPES_BY_ID.crate;
  const a = { id: "a", shape: "crate", x: 0, y: 0, rot: 0 };
  // Its right slot is at (27, 0); put the ghost's left slot a few units away.
  const ghost = { id: "g", shape: "crate", x: 54 + 3, y: 2, rot: 0 };
  const s = anchorSnap(ghost, crate, [a], 24);
  assert.ok(s);
  near(s.x, 54); near(s.y, 0);
});

test("world anchors rotate with the piece", () => {
  const crate = SHAPES_BY_ID.crate;
  const p = { x: 10, y: 20, rot: Math.PI / 2 };
  const anchors = worldAnchors(p, crate);
  // Local (27, 0) becomes (0, 27) before translation.
  near(anchors[0].x, 10); near(anchors[0].y, 47);
  near(anchors[0].nx, 0); near(anchors[0].ny, 1);
});

test("hit testing follows the shape", () => {
  const crate = SHAPES_BY_ID.crate;
  const p = { x: 0, y: 0, rot: 0 };
  assert.equal(hitTest(crate, p, 0, 0), true);
  assert.equal(hitTest(crate, p, 100, 0), false);
  assert.equal(hitTest(crate, p, 22, 22), false);   // outside the 38×38 square
  assert.equal(hitTest(crate, p, 15, 15), true);
});

test("the catalogue is 20 pieces across six categories", () => {
  assert.equal(SHAPES.length, 20);
  assert.equal(new Set(SHAPES.map((s) => s.id)).size, 20);
  for (const cat of CATEGORIES) {
    assert.ok(SHAPES.some((s) => s.cat === cat), `missing category ${cat}`);
  }
  for (const s of SHAPES) {
    assert.ok(s.anchors.length > 0, `${s.id} has no anchors`);
    assert.ok(["edge", "center"].includes(s.mode), `${s.id} has no snap mode`);
    assert.ok(s.color, `${s.id} has no colour`);
  }
});
