// Level editor model: hex maths, shape catalogue, snapping.
// No DOM, no canvas — the editor and the tests both import this.

export const HEX_SIZE = 56;            // circumradius of one hex, world units
export const ANCHOR_STEP = Math.PI / 6; // 30° rotation increments

const SQRT3 = Math.sqrt(3);
const EDGE_LEN = HEX_SIZE * SQRT3;     // hex edge length — also a wall's length

export const ANCHOR = { EDGE: "edge", END: "end", SLOT: "slot" };

// ---------------------------------------------------------------------------
// Hex grid (pointy top, axial q/r, canvas y-down)
// ---------------------------------------------------------------------------

export function hexCenter(q, r, size = HEX_SIZE) {
  return { x: size * SQRT3 * (q + r / 2), y: size * 1.5 * r };
}

export function hexCorners(cx, cy, size = HEX_SIZE) {
  const pts = [];
  for (let i = 0; i < 6; i++) {
    const a = (Math.PI / 3) * i + Math.PI / 6;
    pts.push({ x: cx + size * Math.cos(a), y: cy + size * Math.sin(a) });
  }
  return pts;
}

export function hexEdgeMidpoints(cx, cy, size = HEX_SIZE) {
  const d = (size * SQRT3) / 2;
  return Array.from({ length: 6 }, (_, i) => {
    const a = (Math.PI / 3) * i;
    return { x: cx + d * Math.cos(a), y: cy + d * Math.sin(a), angle: a, edge: i };
  });
}

export function hexRound(q, r) {
  let x = q;
  let z = r;
  let y = -x - z;
  let rx = Math.round(x);
  let ry = Math.round(y);
  let rz = Math.round(z);
  const dx = Math.abs(rx - x);
  const dy = Math.abs(ry - y);
  const dz = Math.abs(rz - z);
  if (dx > dy && dx > dz) rx = -ry - rz;
  else if (dy > dz) ry = -rx - rz;
  else rz = -rx - ry;
  return { q: rx, r: rz };
}

export function pixelToHex(x, y, size = HEX_SIZE) {
  const r = ((2 / 3) * y) / size;
  const q = ((SQRT3 / 3) * x - y / 3) / size;
  return hexRound(q, r);
}

export function nearestHex(x, y, size = HEX_SIZE) {
  const { q, r } = pixelToHex(x, y, size);
  return { q, r, ...hexCenter(q, r, size) };
}

// Nearest hex edge midpoint: walk the six edges of the containing hex.
// The angle is folded into [0, π) because every bar is symmetric under a half
// turn — without this, dragging across an edge flips the rotation by 180°.
export function nearestEdge(x, y, size = HEX_SIZE) {
  const { q, r } = pixelToHex(x, y, size);
  const c = hexCenter(q, r, size);
  let best = null;
  for (const e of hexEdgeMidpoints(c.x, c.y, size)) {
    const d = Math.hypot(e.x - x, e.y - y);
    if (!best || d < best.d) best = { ...e, q, r, d };
  }
  const angle = ((best.angle % Math.PI) + Math.PI) % Math.PI;
  return { ...best, angle };
}

export function hexesInRect(x0, y0, x1, y1, size = HEX_SIZE, pad = 2) {
  let minQ = Infinity, maxQ = -Infinity, minR = Infinity, maxR = -Infinity;
  for (const [x, y] of [[x0, y0], [x1, y0], [x0, y1], [x1, y1]]) {
    const h = pixelToHex(x, y, size);
    minQ = Math.min(minQ, h.q); maxQ = Math.max(maxQ, h.q);
    minR = Math.min(minR, h.r); maxR = Math.max(maxR, h.r);
  }
  const out = [];
  for (let q = minQ - pad; q <= maxQ + pad; q++) {
    for (let r = minR - pad; r <= maxR + pad; r++) {
      const c = hexCenter(q, r, size);
      if (c.x >= x0 - size && c.x <= x1 + size && c.y >= y0 - size && c.y <= y1 + size) {
        out.push({ q, r, ...c });
      }
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Anchors
// ---------------------------------------------------------------------------

function hexAnchors(r) {
  const d = (r * SQRT3) / 2;
  return Array.from({ length: 6 }, (_, i) => {
    const a = (Math.PI / 3) * i;
    return {
      name: `e${i}`, x: d * Math.cos(a), y: d * Math.sin(a),
      nx: Math.cos(a), ny: Math.sin(a), type: ANCHOR.EDGE,
    };
  });
}

function endAnchors(len) {
  return [
    { name: "left", x: -len / 2, y: 0, nx: -1, ny: 0, type: ANCHOR.END },
    { name: "right", x: len / 2, y: 0, nx: 1, ny: 0, type: ANCHOR.END },
  ];
}

function ringAnchors(n, radius, start = 0) {
  return Array.from({ length: n }, (_, i) => {
    const a = start + ((Math.PI * 2) / n) * i;
    return {
      name: `s${i}`, x: radius * Math.cos(a), y: radius * Math.sin(a),
      nx: Math.cos(a), ny: Math.sin(a), type: ANCHOR.SLOT,
    };
  });
}

// ---------------------------------------------------------------------------
// Shape catalogue — 20 pieces, six categories
// ---------------------------------------------------------------------------

const BAR = (len, thick) => ({ type: "bar", len, thick });

export const SHAPES = [
  // Surfaces: hex footprint, lands on hex centres.
  { id: "floor",  name: "Floor",   cat: "Surface", mode: "center", color: "#39485a",
    g: { type: "hex", r: HEX_SIZE }, anchors: hexAnchors(HEX_SIZE) },
  { id: "pit",    name: "Pit",     cat: "Surface", mode: "center", color: "#1a2028",
    g: { type: "hex", r: HEX_SIZE }, anchors: hexAnchors(HEX_SIZE), style: "dashed" },
  { id: "water",  name: "Water",   cat: "Surface", mode: "center", color: "#1f4e63",
    g: { type: "hex", r: HEX_SIZE }, anchors: hexAnchors(HEX_SIZE), style: "waves" },
  { id: "bridge", name: "Bridge",  cat: "Surface", mode: "center", color: "#4a3a28",
    g: { type: "hex", r: HEX_SIZE }, anchors: hexAnchors(HEX_SIZE), style: "planks" },

  // Barriers: a wall is a bar that lies along one hex edge.
  { id: "wall",   name: "Wall",    cat: "Barrier", mode: "edge", color: "#8d9aab",
    g: BAR(EDGE_LEN, 14), anchors: endAnchors(EDGE_LEN) },
  { id: "fence",  name: "Fence",   cat: "Barrier", mode: "edge", color: "#6f7f92",
    g: BAR(EDGE_LEN, 6), anchors: endAnchors(EDGE_LEN), style: "rail" },
  { id: "window", name: "Window",  cat: "Barrier", mode: "edge", color: "#7fb3c8",
    g: BAR(EDGE_LEN, 14), anchors: endAnchors(EDGE_LEN), style: "window" },
  { id: "pillar", name: "Pillar",  cat: "Barrier", mode: "center", color: "#8d9aab",
    g: { type: "circle", r: 20 }, anchors: ringAnchors(4, 26, Math.PI / 4) },

  // Thresholds: a door is a bar that can be opened, still bar-shaped.
  { id: "door",   name: "Door",    cat: "Threshold", mode: "edge", color: "#c9924a",
    g: BAR(EDGE_LEN, 14), anchors: endAnchors(EDGE_LEN), style: "door" },
  { id: "gate",   name: "Gate",    cat: "Threshold", mode: "edge", color: "#b47a3f",
    g: BAR(EDGE_LEN, 16), anchors: endAnchors(EDGE_LEN), style: "gate" },
  { id: "secret", name: "Secret",  cat: "Threshold", mode: "edge", color: "#9b8fc4",
    g: BAR(EDGE_LEN, 14), anchors: endAnchors(EDGE_LEN), style: "secret" },
  { id: "rubble", name: "Rubble",  cat: "Threshold", mode: "edge", color: "#7d7261",
    g: BAR(EDGE_LEN, 18), anchors: endAnchors(EDGE_LEN), style: "rubble" },

  // Fixtures: props on hex centres, slot anchors on their sides.
  { id: "crate",  name: "Crate",   cat: "Fixture", mode: "center", color: "#8a6334",
    g: { type: "rect", w: 38, h: 38 }, anchors: ringAnchors(4, 27) },
  { id: "chest",  name: "Chest",   cat: "Fixture", mode: "center", color: "#9c7440",
    g: { type: "rect", w: 48, h: 32 }, anchors: ringAnchors(4, 30) },
  { id: "altar",  name: "Altar",   cat: "Fixture", mode: "center", color: "#6e7f9c",
    g: { type: "rect", w: 44, h: 28 }, anchors: ringAnchors(4, 28) },
  { id: "brazier", name: "Brazier", cat: "Fixture", mode: "center", color: "#b4622f",
    g: { type: "circle", r: 15 }, anchors: ringAnchors(4, 20, Math.PI / 4) },

  // Resources
  { id: "herb",   name: "Herbs",   cat: "Resource", mode: "center", color: "#5d8a4a",
    g: { type: "circle", r: 13 }, anchors: ringAnchors(1, 19, -Math.PI / 2) },
  { id: "ore",    name: "Ore",     cat: "Resource", mode: "center", color: "#6f8fa3",
    g: { type: "diamond", r: 18 }, anchors: ringAnchors(1, 24, -Math.PI / 2) },

  // Markers
  { id: "spawn",  name: "Spawn",   cat: "Marker", mode: "center", color: "#b4565c",
    g: { type: "cross", r: 18 }, anchors: ringAnchors(1, 24, -Math.PI / 2) },
  { id: "entry",  name: "Entry",   cat: "Marker", mode: "center", color: "#57b39a",
    g: { type: "flag", r: 20 }, anchors: ringAnchors(1, 26, -Math.PI / 2) },
];

export const SHAPES_BY_ID = Object.fromEntries(SHAPES.map((s) => [s.id, s]));
export const CATEGORIES = ["Surface", "Barrier", "Threshold", "Fixture", "Resource", "Marker"];

// ---------------------------------------------------------------------------
// Transforms, anchors, hit testing
// ---------------------------------------------------------------------------

export function rotate(x, y, rot) {
  const c = Math.cos(rot);
  const s = Math.sin(rot);
  return { x: x * c - y * s, y: x * s + y * c };
}

export function worldAnchors(piece, shape) {
  return shape.anchors.map((a, index) => {
    const p = rotate(a.x, a.y, piece.rot);
    const n = rotate(a.nx, a.ny, piece.rot);
    return {
      index, name: a.name, type: a.type,
      x: piece.x + p.x, y: piece.y + p.y,
      nx: n.x, ny: n.y,
    };
  });
}

export function toLocal(piece, x, y) {
  return rotate(x - piece.x, y - piece.y, -piece.rot);
}

export function shapePoints(shape) {
  const g = shape.g;
  switch (g.type) {
    case "hex":
      return hexCorners(0, 0, g.r);
    case "bar":
      return [
        { x: -g.len / 2, y: -g.thick / 2 }, { x: g.len / 2, y: -g.thick / 2 },
        { x: g.len / 2, y: g.thick / 2 }, { x: -g.len / 2, y: g.thick / 2 },
      ];
    case "rect":
      return [
        { x: -g.w / 2, y: -g.h / 2 }, { x: g.w / 2, y: -g.h / 2 },
        { x: g.w / 2, y: g.h / 2 }, { x: -g.w / 2, y: g.h / 2 },
      ];
    case "diamond":
      return [{ x: 0, y: -g.r }, { x: g.r * 0.7, y: 0 }, { x: 0, y: g.r }, { x: -g.r * 0.7, y: 0 }];
    case "cross": {
      const r = g.r, w = g.r * 0.34;
      return [
        { x: -w, y: -r }, { x: w, y: -r }, { x: w, y: -w }, { x: r, y: -w },
        { x: r, y: w }, { x: w, y: w }, { x: w, y: r }, { x: -w, y: r },
        { x: -w, y: w }, { x: -r, y: w }, { x: -r, y: -w }, { x: -w, y: -w },
      ];
    }
    case "flag":
      return [
        { x: -2, y: -g.r }, { x: 2, y: -g.r }, { x: 2, y: g.r },
        { x: g.r * 0.9, y: g.r * 0.35 }, { x: 2, y: g.r * 0.05 },
      ];
    default:
      return [];
  }
}

function pointInPolygon(pts, x, y) {
  let inside = false;
  for (let i = 0, j = pts.length - 1; i < pts.length; j = i++) {
    const xi = pts[i].x, yi = pts[i].y, xj = pts[j].x, yj = pts[j].y;
    if ((yi > y) !== (yj > y) && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}

export function hitTest(shape, piece, x, y, pad = 4) {
  const p = toLocal(piece, x, y);
  if (shape.g.type === "circle") return Math.hypot(p.x, p.y) <= shape.g.r + pad;
  return pointInPolygon(shapePoints(shape), p.x, p.y) ||
    Math.abs(p.x) <= 2 && Math.abs(p.y) <= 2;
}

// ---------------------------------------------------------------------------
// Snapping
// ---------------------------------------------------------------------------

// Anchor snap: translation only. The two anchors must face each other, which
// is what stops a wall on one hex edge from welding to a wall on the next edge
// across the shared corner (their normals are 120° apart, not 180°).
export function anchorSnap(piece, shape, others, threshold = 24) {
  const moving = worldAnchors(piece, shape);
  let best = null;

  for (const other of others) {
    if (other.id === piece.id) continue;
    const otherShape = SHAPES_BY_ID[other.shape];
    if (!otherShape) continue;
    for (const target of worldAnchors(other, otherShape)) {
      for (const m of moving) {
        if (m.type !== target.type) continue;
        const dot = m.nx * target.nx + m.ny * target.ny;
        if (dot > -0.6) continue;                       // not facing each other
        const d = Math.hypot(target.x - m.x, target.y - m.y);
        if (d > threshold) continue;
        if (!best || d < best.d) best = { d, m, target, otherId: other.id };
      }
    }
  }
  if (!best) return null;

  return {
    x: piece.x + (best.target.x - best.m.x),
    y: piece.y + (best.target.y - best.m.y),
    rot: piece.rot,
    kind: "anchor",
    targetId: best.otherId,
  };
}

// Grid snap: edges for walls and doors, hex centres for everything else.
export function gridSnap(piece, shape) {
  if (shape.mode === "edge") {
    const e = nearestEdge(piece.x, piece.y);
    return { x: e.x, y: e.y, rot: e.angle, kind: "grid", q: e.q, r: e.r, edge: e.edge };
  }
  const h = nearestHex(piece.x, piece.y);
  return { x: h.x, y: h.y, rot: piece.rot, kind: "grid", q: h.q, r: h.r };
}

export function snapPiece(piece, shape, others, { anchors = true, grid = true, threshold = 24 } = {}) {
  if (anchors) {
    const a = anchorSnap(piece, shape, others, threshold);
    if (a) return a;
  }
  if (grid) return gridSnap(piece, shape);
  return { ...piece, kind: "free" };
}

export function normalizeAngle(a) {
  const tau = Math.PI * 2;
  return ((a % tau) + tau) % tau;
}
