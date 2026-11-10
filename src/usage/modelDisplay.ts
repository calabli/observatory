/** Display-only formatting: the original identifier remains the query and pricing key. */
// The model dropdown has hundreds of entries and calls this twice per entry
// per render: hoist the regex to a module-level constant so it is not
// recompiled on every call.
const CLAUDE_DISPLAY_PATTERN =
  /^claude-(opus|sonnet|haiku|fable)-(\d+)-(\d+)$/i;
export function modelDisplayName(id: string): string {
  const match = CLAUDE_DISPLAY_PATTERN.exec(id);
  if (!match) return id;
  const family = match[1][0].toUpperCase() + match[1].slice(1).toLowerCase();
  return `Claude ${family} ${match[2]}.${match[3]}`;
}
