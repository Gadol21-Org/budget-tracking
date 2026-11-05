// The SVG charts, drawn as markup strings (the picture has no state: taps are handled by the component around it).

export const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

export interface BarOpts {
  labels: string[];
  tips: string[];
  /** series[j][i]: value (agorot) of series j in slot i. */
  series: number[][];
  colors: string[];
  mode?: "single" | "grouped" | "stacked";
  highlight: Set<number>;
  avg?: number;
  height?: number;
  /** Slot the user tapped to read its values (does not change the active period). */
  peek?: number | null;
}

/** Bar chart as inline SVG; every slot is tappable (data-i). Drawn right to left in time (RTL): the first slot is on the right, so the previous/next arrows match. */
export function barChart(o: BarOpts): string {
  const W = 360, padX = 6, H = o.height ?? 120, top = 16;
  const n = o.labels.length, slot = (W - 2 * padX) / n;
  const mode = o.mode ?? "single";
  const totals = o.labels.map((_, i) => (mode === "grouped" ? Math.max(...o.series.map((s) => s[i])) : o.series.reduce((a, s) => a + s[i], 0)));
  const max = Math.max(1, ...totals, o.avg ?? 0) * 1.05;
  const y = (v: number) => (v / max) * H;
  let out = `<svg viewBox="0 0 ${W} ${H + top + 18}" width="100%" role="img" style="direction:ltr">`;
  o.labels.forEach((label, i) => {
    const x0 = padX + (n - 1 - i) * slot;
    const hot = o.highlight.size === 0 || o.highlight.has(i);
    const op = hot ? 1 : 0.45;
    // Figures above the tapped month and the active period's months: both bars of a pair, or the stack total.
    let figs = "";
    if (o.peek === i || (o.highlight.has(i) && o.highlight.size < n)) {
      const fmt = (v: number) => Math.round(v / 100).toLocaleString("he-IL");
      if (mode === "grouped") {
        // One figure above each bar of the pair; the taller one is lifted so the two never overlap.
        const bw = (slot * 0.8) / o.series.length;
        const ys = o.series.map((sr) => top + H - y(sr[i]) - 4);
        const order = o.series.map((_, j) => j).sort((a, b) => o.series[b][i] - o.series[a][i]);
        order.forEach((j, rank) => {
          const yy = ys[j] - (rank === 0 && o.series.length > 1 && Math.abs(ys[0] - ys[1]) < 10 ? 10 : 0);
          figs += `<text x="${x0 + slot * 0.1 + j * bw + bw / 2}" y="${yy}" font-size="8" font-weight="700" text-anchor="middle" fill="${o.colors[j]}" stroke="var(--bg)" stroke-width="3" paint-order="stroke">${fmt(o.series[j][i])}</text>`;
        });
      } else figs += `<text x="${x0 + slot / 2}" y="${top + H - y(totals[i]) - 4}" font-size="9" font-weight="700" text-anchor="middle" fill="currentColor" stroke="var(--bg)" stroke-width="3" paint-order="stroke">${fmt(totals[i])}</text>`;
    }
    if (mode === "grouped") {
      const bw = (slot * 0.8) / o.series.length;
      o.series.forEach((s, j) => {
        const h = y(s[i]);
        out += `<rect x="${x0 + slot * 0.1 + j * bw}" y="${top + H - h}" width="${bw - 1}" height="${h}" rx="2" fill="${o.colors[j]}" opacity="${op}"/>`;
      });
    } else {
      let base = top + H;
      o.series.forEach((s, j) => {
        const h = y(s[i]);
        base -= h;
        out += `<rect x="${x0 + slot * 0.14}" y="${base}" width="${slot * 0.72}" height="${h}" rx="${mode === "stacked" ? 0 : 3}" fill="${o.colors[j]}" opacity="${op}"/>`;
      });
    }
    out += figs; // drawn over the bars so a figure is never hidden behind its neighbour
    out += `<text x="${x0 + slot / 2}" y="${top + H + 14}" font-size="9" text-anchor="middle" fill="${o.highlight.has(i) && o.highlight.size < n ? "var(--brand)" : "var(--muted)"}" font-weight="${o.highlight.has(i) && o.highlight.size < n ? 700 : 400}">${esc(label)}</text>`;
    out += `<rect data-i="${i}" x="${x0}" y="0" width="${slot}" height="${H + top + 18}" fill="transparent" style="cursor:pointer"><title>${esc(o.tips[i])}</title></rect>`;
  });
  if (o.avg) out += `<line x1="${padX}" x2="${W - padX}" y1="${top + H - y(o.avg)}" y2="${top + H - y(o.avg)}" stroke="#d97706" stroke-dasharray="4 3" stroke-width="1.5"/>`;
  return out + "</svg>";
}

export function lineChart(values: number[], labels: string[], color: string, ghost?: number[], labelEvery = 2): string {
  const W = 360, H = 90, padX = 10, top = 10;
  const all = [...values, ...(ghost ?? [])];
  const max = Math.max(0, ...all), min = Math.min(0, ...all), span = max - min || 1;
  const n = Math.max(values.length, ghost?.length ?? 0, labels.length);
  // RTL: index 0 (earliest) is on the right.
  const x = (i: number) => padX + ((n - 1 - i) * (W - 2 * padX)) / Math.max(1, n - 1);
  const y = (v: number) => top + ((max - v) / span) * H;
  const poly = (vs: number[]) => vs.map((v, i) => `${x(i)},${y(v)}`).join(" ");
  const last = values.length - 1;
  return (
    `<svg viewBox="0 0 ${W} ${H + top + 18}" width="100%" role="img" style="direction:ltr">` +
    `<line x1="${padX}" x2="${W - padX}" y1="${y(0)}" y2="${y(0)}" stroke="var(--line)" stroke-width="1"/>` +
    (ghost ? `<polyline fill="none" stroke="var(--muted)" stroke-width="1.6" stroke-dasharray="4 3" stroke-linejoin="round" points="${poly(ghost)}"/>` : "") +
    `<polyline fill="none" stroke="${color}" stroke-width="2.2" stroke-linejoin="round" points="${poly(values)}"/>` +
    `<circle cx="${x(last)}" cy="${y(values[last])}" r="4" fill="${color}"/>` +
    labels.map((l, i) => (i % labelEvery === 0 || i === labels.length - 1 ? `<text x="${x(i)}" y="${top + H + 14}" font-size="9" text-anchor="middle" fill="var(--muted)">${esc(l)}</text>` : "")).join("") +
    "</svg>"
  );
}

/**
 * The chart as SVG, in the style of Canva's pie charts: a solid pie of rounded slices separated by thin gaps, with the
 * share written on the bigger ones. While a slice `sel` is picked the others turn grey and a dark bubble with `tip`
 * (its name and amount) points at it. The caption underneath is the title and the total.
 */
export function donut(parts: { value: number; color: string }[], caption: { title: string; total: string }, sel: number | null = null, tip = ""): string {
  const total = parts.reduce((s, p) => s + p.value, 0) || 1;
  const R = 56, c = 70, cy = 78;
  const gap = 1.1;
  const pt = (a: number, rad: number) => `${(c + rad * Math.cos(a)).toFixed(2)},${(cy + rad * Math.sin(a)).toFixed(2)}`;
  let a0 = -Math.PI / 2;
  let paths = "";
  let labels = "";
  let bubble = "";
  parts.forEach((p, i) => {
    if (p.value <= 0) return;
    const share = p.value / total;
    const a1 = a0 + Math.min(0.9999, share) * 2 * Math.PI;
    const mid = (a0 + a1) / 2;
    const on = sel == null || i === sel;
    const color = on ? p.color : "#d3d8df";
    if (share >= 0.9999) {
      // A single category is the whole pie: a plain circle, with no seam.
      paths += `<circle data-i="${i}" style="cursor:pointer" cx="${c}" cy="${cy}" r="${R}" fill="${color}"/>`;
      if (on) labels += `<text x="${c}" y="${cy + 3}" text-anchor="middle" font-size="9" font-weight="700" fill="#fff" style="pointer-events:none">100%</text>`;
      a0 = a1;
      return;
    }
    // Plain wedges parted by a surface-coloured outline; on a thin slice the outline shrinks so the slice keeps its colour.
    const big = a1 - a0 > Math.PI ? 1 : 0;
    const w = Math.min(gap, 0.2 * R * (a1 - a0));
    paths += `<path data-i="${i}" style="cursor:pointer" d="M${c},${cy} L${pt(a0, R)} A${R},${R} 0 ${big} 1 ${pt(a1, R)}Z" fill="${color}" stroke="var(--surface)" stroke-width="${w.toFixed(2)}" stroke-linejoin="round"/>`;
    if (on && share >= 0.07) labels += `<text x="${(c + 0.64 * R * Math.cos(mid)).toFixed(1)}" y="${(cy + 0.64 * R * Math.sin(mid) + 3).toFixed(1)}" text-anchor="middle" font-size="9" font-weight="700" fill="#fff" style="pointer-events:none">${Math.round(share * 100)}%</text>`;
    if (i === sel && tip) {
      // The bubble sits above the picture, never on it; its tip points down toward the slice. The slice itself gets a same-coloured outline so a sliver stays visible.
      const w = Math.min(136, 12 + tip.length * 5.2), h = 15;
      const px = c + R * Math.cos(mid);
      const x = Math.max(w / 2 + 2, Math.min(140 - w / 2 - 2, px)), tx = Math.max(x - w / 2 + 8, Math.min(x + w / 2 - 8, px));
      bubble = `<g style="pointer-events:none"><path d="M${c},${cy} L${pt(a0, R)} A${R},${R} 0 ${a1 - a0 > Math.PI ? 1 : 0} 1 ${pt(a1, R)}Z" fill="${p.color}" stroke="${p.color}" stroke-width="1.6" stroke-linejoin="round"/><rect x="${(x - w / 2).toFixed(1)}" y="1" width="${w.toFixed(1)}" height="${h}" rx="5" fill="#1f2a30"/><path d="M${(tx - 4).toFixed(1)},${1 + h} L${tx.toFixed(1)},${h + 5} L${(tx + 4).toFixed(1)},${1 + h}Z" fill="#1f2a30"/><text x="${x.toFixed(1)}" y="11.3" text-anchor="middle" font-size="8.5" font-weight="600" fill="#fff">${esc(tip)}</text></g>`;
    }
    a0 = a1;
  });
  const title = caption.title.length > 22 ? `${caption.title.slice(0, 21)}…` : caption.title;
  return `<svg viewBox="0 0 140 172" width="132" role="img">${paths}${labels}${bubble}<text x="70" y="153" text-anchor="middle" font-size="8.5" fill="var(--muted)">${esc(title)}</text><text x="70" y="166" text-anchor="middle" font-size="11" font-weight="700" fill="currentColor">${esc(caption.total)}</text></svg>`;
}

