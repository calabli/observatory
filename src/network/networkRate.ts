export type Counters = {
  id: string;
  received: number;
  transmitted: number;
  at: number;
};
export function calculateRate(
  previous: Counters | null,
  current: Counters,
): { down: number; up: number } | null {
  if (!previous || previous.id !== current.id) return null;
  const elapsed = current.at - previous.at;
  if (
    elapsed <= 0 ||
    elapsed > 15000 ||
    current.received < previous.received ||
    current.transmitted < previous.transmitted
  )
    return null;
  return {
    down: ((current.received - previous.received) * 1000) / elapsed,
    up: ((current.transmitted - previous.transmitted) * 1000) / elapsed,
  };
}
