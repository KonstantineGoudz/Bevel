import {
  SHAPES, SHAPES_BY_ID, CATEGORIES, HEX_SIZE, ANCHOR_STEP,
  hexCorners, hexesInRect, nearestHex, worldAnchors, hitTest,
  snapPiece, normalizeAngle, shapePoints,
} from "./model.js";

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

const cam = { x: 0, y: 0, zoom: 1 };
let view = { w: 800, h: 600, dpr: 1 };

const state = {
  pieces: [],          // { id, shape, x, y, rot }
  nextId: 1,
  tool: null,          // shape id armed for placement
  selected: null,      // piece id
  hover: null,
  ghost: null,
  ghostRot: 0,
  drag: null,
  pan: null,
  space: false,
  snap: true,
  grid: true,
  undo: [],
};

const $ = (sel) => document.querySelector(sel);
const canvas = $("#canvas");
const ctx = canvas.getContext("2d");
let lwScale = 1;   // world units per screen pixel, for constant-width strokes

// ---------------------------------------------------------------------------
// Geometry helpers for the viewport
// ---------------------------------------------------------------------------

const screenOf = (x, y) => ({
  x: (x - cam.x) * cam.zoom + view.w / 2,
  y: (y - cam.y) * cam.zoom + view.h / 2,
});

const worldOf = (sx, sy) => ({
  x: (sx - view.w / 2) / cam.zoom + cam.x,
  y: (sy - view.h / 2) / cam.zoom + cam.y,
});

function resize() {
  const rect = canvas.parentElement.getBoundingClientRect();
  view.dpr = Math.min(window.devicePixelRatio || 1, 2);
  view.w = Math.max(1, Math.floor(rect.width));
  view.h = Math.max(1, Math.floor(rect.height));
  canvas.width = Math.floor(view.w * view.dpr);
  canvas.height = Math.floor(view.h * view.dpr);
  canvas.style.width = `${view.w}px`;
  canvas.style.height = `${view.h}px`;
}

// ---------------------------------------------------------------------------
// Painting
// ---------------------------------------------------------------------------

function pathPolygon(ctx, pts) {
  ctx.beginPath();
  ptr(pts[0]);
  for (let i = 1; i < pts.length; i++) ptr(pts[i]);
  ctx.closePath();
  function ptr(p) { ctx.lineTo(p.x, p.y); }
}

function roundRect(ctx, x, y, w, h, r) {
  ctx.beginPath();
  ctx.moveTo(x + r, y);
  ctx.arcTo(x + w, y, x + w, y + h, r);
  ctx.arcTo(x + w, y + h, x, y + h, r);
  ctx.arcTo(x, y + h, x, y, r);
  ctx.arcTo(x, y, x + w, y, r);
  ctx.closePath();
}

function shapePath(ctx, shape) {
  const g = shape.g;
  switch (g.type) {
    case "hex": pathPolygon(ctx, hexCorners(0, 0, g.r)); break;
    case "bar": roundRect(ctx, -g.len / 2, -g.thick / 2, g.len, g.thick, Math.min(4, g.thick / 2)); break;
    case "rect": roundRect(ctx, -g.w / 2, -g.h / 2, g.w, g.h, 3); break;
    case "circle": ctx.beginPath(); ctx.arc(0, 0, g.r, 0, Math.PI * 2); break;
    default: pathPolygon(ctx, shapePoints(shape));
  }
}

function decorate(ctx, shape) {
  const g = shape.g;
  const style = shape.style;
  if (!style) return;

  ctx.save();
  shapePath(ctx, shape);
  ctx.clip();

  if (style === "dashed") {
    ctx.setLineDash([6, 6]);
    ctx.strokeStyle = "rgba(255,255,255,.28)";
    ctx.lineWidth = 1.4 / lwScale;
    ctx.beginPath();
    ctx.moveTo(-g.r, -g.r * 0.5); ctx.lineTo(g.r, -g.r * 0.5);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  if (style === "waves") {
    ctx.strokeStyle = "rgba(190,225,240,.28)";
    ctx.lineWidth = 1.6 / lwScale;
    for (let i = -1; i <= 1; i++) {
      ctx.beginPath();
      for (let x = -g.r; x <= g.r; x += 4) {
        const y = i * 16 + Math.sin((x + i * 10) / 9) * 3.5;
        if (x === -g.r) ctx.moveTo(x, y); else ctx.lineTo(x, y);
      }
      ctx.stroke();
    }
  }

  if (style === "planks") {
    ctx.strokeStyle = "rgba(0,0,0,.35)";
    ctx.lineWidth = 1.6 / lwScale;
    for (let i = -2; i <= 2; i++) {
      ctx.beginPath();
      ctx.moveTo(-g.r, i * 18); ctx.lineTo(g.r, i * 18);
      ctx.stroke();
    }
  }

  if (style === "rail") {
    ctx.strokeStyle = "rgba(0,0,0,.4)";
    ctx.lineWidth = 1.4 / lwScale;
    for (let x = -g.len / 2 + 8; x < g.len / 2; x += 16) {
      ctx.beginPath(); ctx.moveTo(x, -g.thick / 2 - 4); ctx.lineTo(x, g.thick / 2 + 4); ctx.stroke();
    }
  }

  if (style === "window") {
    ctx.fillStyle = "rgba(10,20,30,.55)";
    ctx.fillRect(-g.len / 2 + 14, -g.thick / 2 + 3, g.len - 28, g.thick - 6);
    ctx.strokeStyle = "rgba(200,235,255,.5)";
    ctx.lineWidth = 1.2 / lwScale;
    ctx.beginPath(); ctx.moveTo(0, -g.thick / 2 + 3); ctx.lineTo(0, g.thick / 2 - 3); ctx.stroke();
  }

  if (style === "door") {
    ctx.strokeStyle = "rgba(0,0,0,.35)";
    ctx.lineWidth = 1.2 / lwScale;
    for (let x = -g.len / 2 + 12; x < g.len / 2 - 4; x += 18) {
      ctx.beginPath(); ctx.moveTo(x, -g.thick / 2 + 2); ctx.lineTo(x, g.thick / 2 - 2); ctx.stroke();
    }
    ctx.fillStyle = "rgba(255,255,255,.55)";
    ctx.beginPath(); ctx.arc(g.len / 2 - 16, 0, 2.6, 0, Math.PI * 2); ctx.fill();
  }

  if (style === "gate") {
    ctx.strokeStyle = "rgba(0,0,0,.45)";
    ctx.lineWidth = 2 / lwScale;
    for (let x = -g.len / 2 + 10; x < g.len / 2 - 4; x += 12) {
      ctx.beginPath(); ctx.moveTo(x, -g.thick / 2); ctx.lineTo(x, g.thick / 2); ctx.stroke();
    }
  }

  if (style === "secret") {
    ctx.setLineDash([5, 5]);
    ctx.strokeStyle = "rgba(255,255,255,.45)";
    ctx.lineWidth = 1.4 / lwScale;
    ctx.beginPath();
    ctx.moveTo(-g.len / 2, -g.thick / 2 + 2); ctx.lineTo(g.len / 2, -g.thick / 2 + 2);
    ctx.stroke();
    ctx.setLineDash([]);
  }

  if (style === "rubble") {
    ctx.fillStyle = "rgba(0,0,0,.3)";
    for (let i = 0; i < 7; i++) {
      const x = -g.len / 2 + 10 + ((i * 37) % (g.len - 20));
      const y = ((i * 23) % (g.thick - 6)) - g.thick / 2 + 3;
      ctx.beginPath(); ctx.arc(x, y, 3 + (i % 3), 0, Math.PI * 2); ctx.fill();
    }
  }
  ctx.restore();
}

function paintShape(ctx, shape) {
  shapePath(ctx, shape);
  ctx.fillStyle = shape.color;
  ctx.fill();
  ctx.lineWidth = 1.6 / lwScale;
  ctx.strokeStyle = "rgba(255,255,255,.26)";
  ctx.stroke();
  decorate(ctx, shape);
}

function paintAnchors(ctx, piece, shape, zoom, strong) {
  const size = 4.5 / zoom;  for (const a of worldAnchors(piece, shape)) {
    const color = a.type === "end" ? "#e0a24a" : a.type === "slot" ? "#b08bf0" : "#59a7c4";
    ctx.globalAlpha = strong ? 1 : 0.55;
    ctx.beginPath();
    ctx.moveTo(a.x, a.y - size);
    ctx.lineTo(a.x + size, a.y);
    ctx.lineTo(a.x, a.y + size);
    ctx.lineTo(a.x - size, a.y);
    ctx.closePath();
    ctx.fillStyle = color;
    ctx.fill();
    ctx.lineWidth = 1 / zoom;
    ctx.strokeStyle = "rgba(0,0,0,.55)";
    ctx.stroke();
  }
  ctx.globalAlpha = 1;
}

function drawGrid() {
  const a = worldOf(0, 0);
  const b = worldOf(view.w, view.h);
  const cells = hexesInRect(a.x, a.y, b.x, b.y, HEX_SIZE);

  ctx.save();
  ctx.translate(view.w / 2, view.h / 2);
  ctx.scale(cam.zoom, cam.zoom);
  ctx.translate(-cam.x, -cam.y);

  ctx.lineWidth = 1 / cam.zoom;
  ctx.strokeStyle = "rgba(120,150,180,.13)";
  for (const cell of cells) {
    pathPolygon(ctx, hexCorners(cell.x, cell.y, HEX_SIZE));
    ctx.stroke();
  }

  // Highlight the hex under the cursor.
  if (state.hoverHex) {
    const h = state.hoverHex;
    pathPolygon(ctx, hexCorners(h.x, h.y, HEX_SIZE));
    ctx.fillStyle = "rgba(120,170,220,.05)";
    ctx.fill();
  }
  ctx.restore();
}

function drawPieces() {
  lwScale = cam.zoom;
  for (const piece of state.pieces) {
    const shape = SHAPES_BY_ID[piece.shape];
    if (!shape) continue;
    const s = screenOf(piece.x, piece.y);
    ctx.save();
    ctx.translate(s.x, s.y);
    ctx.rotate(piece.rot);
    ctx.scale(cam.zoom, cam.zoom);
    ctx.globalAlpha = 1;
    paintShape(ctx, shape);
    ctx.restore();

    if (piece.id === state.selected) {
      ctx.save();
      ctx.translate(s.x, s.y);
      ctx.rotate(piece.rot);
      ctx.scale(cam.zoom, cam.zoom);
      shapePath(ctx, shape);
      ctx.lineWidth = 2.5 / cam.zoom;
      ctx.strokeStyle = "#f0a63c";
      ctx.stroke();
      ctx.restore();

      ctx.save();
      ctx.translate(s.x, s.y);
      ctx.rotate(piece.rot);
      ctx.scale(cam.zoom, cam.zoom);
      paintAnchors(ctx, { x: 0, y: 0, rot: 0 }, shape, cam.zoom, true);
      ctx.restore();
    }
  }
}

function drawGhost() {
  if (!state.ghost) return;
  lwScale = cam.zoom;
  const { piece, shape, snap } = state.ghost;
  const s = screenOf(piece.x, piece.y);

  ctx.save();
  ctx.translate(s.x, s.y);
  ctx.rotate(piece.rot);
  ctx.scale(cam.zoom, cam.zoom);
  ctx.globalAlpha = 0.6;
  paintShape(ctx, shape);
  ctx.globalAlpha = 1;
  paintAnchors(ctx, { x: 0, y: 0, rot: 0 }, shape, cam.zoom, true);
  ctx.restore();

  if (snap && snap.kind === "anchor" && snap.targetId) {
    const target = state.pieces.find((p) => p.id === snap.targetId);
    if (target) {
      const ts = screenOf(target.x, target.y);
      ctx.beginPath();
      ctx.arc(ts.x, ts.y, 30 * cam.zoom, 0, Math.PI * 2);
      ctx.strokeStyle = "rgba(224,162,74,.5)";
      ctx.lineWidth = 1.5;
      ctx.stroke();
    }
  }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

function render() {
  ctx.setTransform(view.dpr, 0, 0, view.dpr, 0, 0);
  ctx.clearRect(0, 0, view.w, view.h);
  ctx.fillStyle = "#0a0d12";
  ctx.fillRect(0, 0, view.w, view.h);

  if (state.grid) drawGrid();
  drawPieces();
  drawGhost();

  requestAnimationFrame(render);
}

// ---------------------------------------------------------------------------
// Snapping and placing
// ---------------------------------------------------------------------------

const snapOptions = () => ({ anchors: state.snap, grid: state.snap, threshold: 22 / cam.zoom });

function placeAt(wx, wy) {
  const shape = SHAPES_BY_ID[state.tool];
  if (!shape) return;
  const raw = { x: wx, y: wy, rot: state.ghostRot };
  const snapped = snapPiece(raw, shape, state.pieces, snapOptions());
  commit();
  const piece = {
    id: `p${state.nextId++}`,
    shape: shape.id,
    x: snapped.x,
    y: snapped.y,
    rot: normalizeAngle(snapped.rot),
  };
  state.pieces.push(piece);
  state.selected = piece.id;
  autosave();
}

function updateGhost(wx, wy) {
  if (!state.tool) { state.ghost = null; return; }
  const shape = SHAPES_BY_ID[state.tool];
  const raw = { x: wx, y: wy, rot: state.ghostRot };
  const snapped = snapPiece(raw, shape, state.pieces, snapOptions());
  state.ghost = { piece: { ...raw, ...snapped }, shape, snap: snapped };
  state.ghostRot = snapped.rot;
}

function updateDrag(wx, wy) {
  const piece = state.pieces.find((p) => p.id === state.drag.id);
  if (!piece) return;
  const shape = SHAPES_BY_ID[piece.shape];
  const raw = { x: wx + state.drag.dx, y: wy + state.drag.dy, rot: piece.rot };
  const snapped = snapPiece(raw, shape, state.pieces, snapOptions());
  piece.x = snapped.x;
  piece.y = snapped.y;
  piece.rot = normalizeAngle(snapped.rot);
  state.drag.snap = snapped;
}

// ---------------------------------------------------------------------------
// Undo and persistence
// ---------------------------------------------------------------------------

function commit() {
  state.undo.push(JSON.stringify(state.pieces));
  if (state.undo.length > 60) state.undo.shift();
}

function undo() {
  const prev = state.undo.pop();
  if (!prev) return;
  state.pieces = JSON.parse(prev);
  state.selected = null;
  autosave();
}

const STORAGE = "party-dungeon-level.v1";

function serialise() {
  return {
    format: "party-dungeon-level",
    version: 1,
    hexSize: HEX_SIZE,
    pieces: state.pieces.map((p) => ({
      shape: p.shape,
      x: Math.round(p.x * 100) / 100,
      y: Math.round(p.y * 100) / 100,
      rotDeg: Math.round((p.rot * 180) / Math.PI * 10) / 10,
    })),
  };
}

function loadData(data) {
  if (!data || data.format !== "party-dungeon-level" || !Array.isArray(data.pieces)) {
    throw new Error("not a level file");
  }
  const pieces = [];
  for (const p of data.pieces) {
    if (!SHAPES_BY_ID[p.shape]) continue;
    pieces.push({
      id: `p${state.nextId++}`,
      shape: p.shape,
      x: Number(p.x) || 0,
      y: Number(p.y) || 0,
      rot: normalizeAngle(((Number(p.rotDeg) || 0) * Math.PI) / 180),
    });
  }
  state.pieces = pieces;
  state.selected = null;
  state.tool = null;
  state.ghost = null;
  syncPalette();
}

let saveTimer = 0;
function autosave() {
  clearTimeout(saveTimer);
  saveTimer = setTimeout(() => {
    try { localStorage.setItem(STORAGE, JSON.stringify(serialise())); } catch { /* ignore */ }
    syncHud();
  }, 150);
}

function download() {
  const blob = new Blob([JSON.stringify(serialise(), null, 2)], { type: "application/json" });
  const a = document.createElement("a");
  a.href = URL.createObjectURL(blob);
  a.download = "level.json";
  a.click();
  URL.revokeObjectURL(a.href);
}

// ---------------------------------------------------------------------------
// Palette
// ---------------------------------------------------------------------------

function drawIcon(el, shape) {
  const size = 46;
  const dpr = Math.min(window.devicePixelRatio || 1, 2);
  el.width = size * dpr;
  el.height = size * dpr;
  el.style.width = `${size}px`;
  el.style.height = `${size}px`;
  const c = el.getContext("2d");
  c.scale(dpr, dpr);

  const pts = shapePoints(shape);
  let w = 1, h = 1;
  if (pts.length) {
    w = Math.max(...pts.map((p) => Math.abs(p.x))) * 2 || 1;
    h = Math.max(...pts.map((p) => Math.abs(p.y))) * 2 || 1;
  } else if (shape.g.type === "circle") {
    w = h = shape.g.r * 2;
  }
  const scale = Math.min(38 / w, 32 / h);

  c.translate(size / 2, size / 2);
  c.scale(scale, scale);
  c.lineJoin = "round";
  lwScale = scale;
  paintShape(c, shape);
  c.globalAlpha = 0.9;
  paintAnchors(c, { x: 0, y: 0, rot: 0 }, shape, scale, false);
  c.globalAlpha = 1;
}

function buildPalette() {
  const groups = $("#groups");
  groups.innerHTML = "";
  for (const cat of CATEGORIES) {
    const shapes = SHAPES.filter((s) => s.cat === cat);
    if (!shapes.length) continue;
    const h = document.createElement("h2");
    h.textContent = cat;
    groups.appendChild(h);

    for (const shape of shapes) {
      const btn = document.createElement("button");
      btn.className = "piece";
      btn.type = "button";
      btn.dataset.shape = shape.id;
      btn.title = `${shape.name} — ${shape.anchors.length} anchor${shape.anchors.length === 1 ? "" : "s"} (${shape.anchors[0].type})`;
      const icon = document.createElement("canvas");
      const label = document.createElement("span");
      label.className = "name";
      label.textContent = shape.name;
      const badge = document.createElement("span");
      badge.className = "badge";
      badge.textContent = shape.anchors.length;
      btn.append(icon, label, badge);
      btn.addEventListener("click", () => selectTool(shape.id));
      groups.appendChild(btn);
      drawIcon(icon, shape);
    }
  }
}

function syncPalette() {
  for (const btn of document.querySelectorAll(".piece")) {
    btn.classList.toggle("active", btn.dataset.shape === state.tool);
  }
}

function selectTool(id) {
  state.tool = state.tool === id ? null : id;
  state.selected = null;
  state.ghostRot = 0;
  state.ghost = null;
  syncPalette();
  syncHud();
  canvas.style.cursor = state.tool ? "crosshair" : "default";
}

// ---------------------------------------------------------------------------
// HUD
// ---------------------------------------------------------------------------

function syncHud() {
  const count = state.pieces.length;
  $("#count").textContent = `${count} placed`;
  const hint = $("#hint");
  if (state.tool) {
    hint.textContent = `${SHAPES_BY_ID[state.tool].name} — click to place · R rotates · Esc stops`;
  } else if (state.selected) {
    hint.textContent = "Drag to move · R rotates · Delete removes";
  } else {
    hint.textContent = "Pick a component on the left, then click the grid";
  }
  const hex = state.hoverHex;
  $("#status").textContent = [
    hex ? `hex ${hex.q},${hex.r}` : "—",
    `zoom ${Math.round(cam.zoom * 100)}%`,
    state.snap ? "snap on" : "snap off",
    state.selected ? "1 selected" : "",
  ].filter(Boolean).join("  ·  ");
}

// ---------------------------------------------------------------------------
// Pointer and keyboard
// ---------------------------------------------------------------------------

function pointerWorld(e) {
  const r = canvas.getBoundingClientRect();
  return worldOf(e.clientX - r.left, e.clientY - r.top);
}

canvas.addEventListener("pointerdown", (e) => {
  canvas.setPointerCapture(e.pointerId);
  const p = pointerWorld(e);

  if (e.button === 1 || state.space) {
    state.pan = { x: e.clientX, y: e.clientY, cx: cam.x, cy: cam.y };
    canvas.style.cursor = "grabbing";
    return;
  }

  if (state.tool) {
    placeAt(p.x, p.y);
    return;
  }

  const shapeAt = pick(p.x, p.y);
  if (shapeAt) {
    state.selected = shapeAt.id;
    const dx = shapeAt.x - p.x;
    const dy = shapeAt.y - p.y;
    commit();
    state.drag = { id: shapeAt.id, dx, dy, moved: false };
    canvas.style.cursor = "grabbing";
  } else {
    state.selected = null;
    state.drag = null;
  }
  syncHud();
});

canvas.addEventListener("pointermove", (e) => {
  const p = pointerWorld(e);
  state.hoverHex = nearestHex(p.x, p.y);

  if (state.pan) {
    cam.x = state.pan.cx - (e.clientX - state.pan.x) / cam.zoom;
    cam.y = state.pan.cy - (e.clientY - state.pan.y) / cam.zoom;
    return;
  }
  if (state.drag) {
    state.drag.moved = true;
    updateDrag(p.x, p.y);
    return;
  }
  state.hover = pick(p.x, p.y)?.id ?? null;
  if (state.tool) updateGhost(p.x, p.y);
  syncHud();
});

canvas.addEventListener("pointerup", (e) => {
  if (state.pan) {
    state.pan = null;
    canvas.style.cursor = state.space ? "grab" : "default";
  }
  if (state.drag) {
    if (!state.drag.moved) state.undo.pop();
    else autosave();
    state.drag = null;
    canvas.style.cursor = state.tool ? "crosshair" : "default";
  }
  syncHud();
  try { canvas.releasePointerCapture(e.pointerId); } catch { /* ignore */ }
});

canvas.addEventListener("pointerleave", () => {
  state.hoverHex = null;
  if (!state.tool) state.ghost = null;
});

canvas.addEventListener("wheel", (e) => {
  e.preventDefault();
  const before = pointerWorld(e);
  const factor = Math.exp(-e.deltaY * 0.0015);
  cam.zoom = Math.min(2.5, Math.max(0.35, cam.zoom * factor));
  const after = pointerWorld(e);
  cam.x += before.x - after.x;
  cam.y += before.y - after.y;
  syncHud();
}, { passive: false });

function pick(x, y) {
  for (let i = state.pieces.length - 1; i >= 0; i--) {
    const piece = state.pieces[i];
    const shape = SHAPES_BY_ID[piece.shape];
    if (shape && hitTest(shape, piece, x, y, 6 / cam.zoom)) return piece;
  }
  return null;
}

window.addEventListener("keydown", (e) => {
  if (e.key === " ") { state.space = true; canvas.style.cursor = "grab"; e.preventDefault(); return; }

  if (e.key === "Escape") {
    state.tool = null;
    state.selected = null;
    state.ghost = null;
    syncPalette();
    syncHud();
    canvas.style.cursor = "default";
    return;
  }

  if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "z") {
    e.preventDefault();
    undo();
    return;
  }

  if (e.key.toLowerCase() === "r") {
    const step = e.shiftKey ? -ANCHOR_STEP : ANCHOR_STEP;
    if (state.selected) {
      const piece = state.pieces.find((p) => p.id === state.selected);
      if (piece) {
        commit();
        piece.rot = normalizeAngle(piece.rot + step);
        autosave();
      }
    } else if (state.tool) {
      state.ghostRot = normalizeAngle(state.ghostRot + step);
      if (state.ghost) updateGhost(state.ghost.piece.x, state.ghost.piece.y);
    }
    return;
  }

  if (e.key === "Delete" || e.key === "Backspace") {
    if (state.selected) {
      commit();
      state.pieces = state.pieces.filter((p) => p.id !== state.selected);
      state.selected = null;
      autosave();
      syncHud();
    }
    return;
  }

  if (e.key.toLowerCase() === "s") {
    state.snap = !state.snap;
    document.querySelector('[data-act="snap"]').classList.toggle("on", state.snap);
    if (state.ghost) updateGhost(state.ghost.piece.x, state.ghost.piece.y);
    syncHud();
  }

  if (e.key.toLowerCase() === "g") {
    state.grid = !state.grid;
    document.querySelector('[data-act="grid"]').classList.toggle("on", state.grid);
  }
});

window.addEventListener("keyup", (e) => {
  if (e.key === " ") {
    state.space = false;
    if (!state.pan) canvas.style.cursor = state.tool ? "crosshair" : "default";
  }
});

// ---------------------------------------------------------------------------
// Toolbar
// ---------------------------------------------------------------------------

document.querySelectorAll("[data-act]").forEach((btn) => {
  btn.addEventListener("click", () => {
    const act = btn.dataset.act;
    if (act === "undo") undo();
    if (act === "clear") {
      if (!state.pieces.length) return;
      commit();
      state.pieces = [];
      state.selected = null;
      autosave();
      syncHud();
    }
    if (act === "save") download();
    if (act === "load") $("#file").click();
    if (act === "grid") {
      state.grid = !state.grid;
      btn.classList.toggle("on", state.grid);
    }
    if (act === "snap") {
      state.snap = !state.snap;
      btn.classList.toggle("on", state.snap);
      syncHud();
    }
  });
});

$("#file").addEventListener("change", async (e) => {
  const file = e.target.files?.[0];
  if (!file) return;
  try {
    const data = JSON.parse(await file.text());
    commit();
    loadData(data);
    autosave();
    syncHud();
  } catch (err) {
    $("#hint").textContent = `Could not load: ${err.message}`;
  }
  e.target.value = "";
});

// ---------------------------------------------------------------------------
// Boot
// ---------------------------------------------------------------------------

buildPalette();
resize();
new ResizeObserver(resize).observe(canvas.parentElement);

try {
  const saved = localStorage.getItem(STORAGE);
  if (saved) loadData(JSON.parse(saved));
} catch { /* ignore */ }

state.hoverHex = null;
syncHud();
requestAnimationFrame(render);

// Debug handle for scripted checks; harmless in normal use.
window.__editor = { state, cam, view };
