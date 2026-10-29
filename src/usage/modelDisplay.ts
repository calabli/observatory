/** Display-only formatting: the original identifier remains the query and pricing key. */
export function modelDisplayName(id: string): string {
  const match = /^claude-(opus|sonnet|haiku|fable)-(\d+)-(\d+)$/i.exec(id);
  if (!match) return id;
  const family = match[1][0].toUpperCase() + match[1].slice(1).toLowerCase();
  return `Claude ${family} ${match[2]}.${match[3]}`;
}
