// Notion large-database scroll benchmark.
//
// Open a big database (table/board view) and run this in the page's console:
//   - Lotion debug build: View → Developer Tools (⌥⌘I), or run with
//     LOTION_SCROLLBENCH=1 to have it run automatically and logged.
//   - Notion desktop: View → Developer Tools (⌥⌘I).
//   - Safari: Develop → Show JavaScript Console.
// It scrolls the main Notion scroller top→bottom at a fixed speed and reports
// frame-time percentiles. Same script, same view ⇒ comparable numbers.
(async function lotionScrollBench(opts) {
  opts = Object.assign({ pxPerFrame: 40, maxFrames: 600 }, opts || {});
  const scrollers = Array.from(document.querySelectorAll('.notion-frame .notion-scroller, .notion-peek-renderer .notion-scroller, .notion-scroller'))
    .filter((el) => el.scrollHeight > el.clientHeight + 100);
  const scroller = scrollers.sort((a, b) => b.scrollHeight - a.scrollHeight)[0];
  if (!scroller) return { error: 'no scrollable Notion view found' };
  scroller.scrollTop = 0;
  await new Promise((r) => setTimeout(r, 500));
  const frames = [];
  let last = performance.now();
  let maxRows = 0;
  for (let i = 0; i < opts.maxFrames; i++) {
    await new Promise((r) => requestAnimationFrame(r));
    const now = performance.now();
    frames.push(now - last);
    last = now;
    maxRows = Math.max(maxRows, document.querySelectorAll('.notion-collection-item, .notion-table-view-row, [data-block-id]').length);
    const before = scroller.scrollTop;
    scroller.scrollTop = before + opts.pxPerFrame;
    if (scroller.scrollTop === before) break; // reached the bottom
  }
  frames.shift();
  const sorted = frames.slice().sort((a, b) => a - b);
  const pct = (p) => +sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))].toFixed(1);
  const budget = 1000 / 60 + 1;
  const result = {
    frames: frames.length,
    scrolledPx: scroller.scrollTop,
    p50ms: pct(0.5), p95ms: pct(0.95), p99ms: pct(0.99), maxMs: +sorted[sorted.length - 1].toFixed(1),
    jankPct: +(100 * frames.filter((f) => f > budget).length / frames.length).toFixed(1),
    avgFps: +(1000 / (frames.reduce((a, b) => a + b, 0) / frames.length)).toFixed(1),
    peakRenderedRows: maxRows,
    ua: navigator.userAgent,
  };
  console.table ? console.table(result) : console.log(result);
  return result;
})();
