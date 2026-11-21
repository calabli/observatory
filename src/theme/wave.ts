/**
 * Color of bar `index` out of `count` in a widget-style bar chart: a fade from
 * `--wave-from` to `--wave-to` (both defined in widget.css), or a single
 * accent when one is given.
 */
export function waveColor(index: number, count: number, accent?: string) {
  if (accent) return accent;
  const share = count > 1 ? Math.round((index / (count - 1)) * 100) : 100;
  return `color-mix(in oklab, var(--wave-from), var(--wave-to) ${share}%)`;
}
