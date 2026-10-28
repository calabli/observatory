type Props = {
  label: "开始时间" | "结束时间";
  value: string;
  onChange: (value: string) => void;
};
const pad = (n: number) => String(n).padStart(2, "0");
const hours = Array.from({ length: 24 }, (_, i) => pad(i));
const minutes = Array.from({ length: 60 }, (_, i) => pad(i));

export function DateTimeFields({ label, value, onChange }: Props) {
  const [date = "", rawTime = ""] = value.split("T");
  const fallback = label === "开始时间" ? "00:00:00" : "23:59:59";
  const time = /^\d{2}:\d{2}:\d{2}$/.test(rawTime) ? rawTime : fallback;
  const parts = time.split(":");
  const updatePart = (index: number, part: string) => {
    const next = [...parts];
    next[index] = part;
    onChange(`${date}T${next.join(":")}`);
  };
  return (
    <div className="usage-datetime" role="group" aria-label={label}>
      <span>{label}</span>
      <input
        type="date"
        aria-label={`${label}日期`}
        value={date}
        onChange={(e) =>
          onChange(e.target.value ? `${e.target.value}T${time}` : "")
        }
      />
      {(
        [
          ["时", hours],
          ["分", minutes],
          ["秒", minutes],
        ] as const
      ).map(([unit, options], i) => (
        <label key={unit} className="usage-time-unit">
          <select
            aria-label={`${label}${unit}`}
            disabled={!date}
            value={parts[i]}
            onChange={(e) => updatePart(i, e.target.value)}
          >
            {options.map((v) => (
              <option key={v} value={v}>
                {v}
              </option>
            ))}
          </select>
          <span>{unit}</span>
        </label>
      ))}
    </div>
  );
}
