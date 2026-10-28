import * as Select from "@radix-ui/react-select";
import { Check, ChevronDown, ChevronUp, Layers3 } from "lucide-react";
import { AppGlyph } from "./AppGlyph";

export function AppSelect({
  value,
  sources,
  onChange,
}: {
  value: string;
  sources: { id: string; name: string }[];
  onChange: (value: string) => void;
}) {
  const items = [{ id: "", name: "全部应用" }, ...sources];
  return (
    <Select.Root
      value={value || "all-apps"}
      onValueChange={(next) => onChange(next === "all-apps" ? "" : next)}
    >
      <Select.Trigger className="app-select-trigger" aria-label="统计应用">
        <Select.Value />
        <Select.Icon className="app-select-arrow">
          <ChevronDown size={14} />
        </Select.Icon>
      </Select.Trigger>
      <Select.Portal
        container={document.querySelector<HTMLElement>(".obs-shell")}
      >
        <Select.Content
          className="app-select-menu"
          position="popper"
          sideOffset={5}
          collisionPadding={10}
        >
          <Select.ScrollUpButton className="app-select-scroll">
            <ChevronUp size={14} />
          </Select.ScrollUpButton>
          <Select.Viewport className="app-select-viewport">
            {items.map((item) => (
              <Select.Item
                className="app-select-item"
                key={item.id}
                value={item.id || "all-apps"}
                textValue={item.name}
              >
                <Select.ItemText>
                  <span className="app-select-label">
                    {item.id ? (
                      <AppGlyph app={item.id} />
                    ) : (
                      <span className="app-glyph">
                        <Layers3 size={14} />
                      </span>
                    )}
                    <span>{item.name}</span>
                  </span>
                </Select.ItemText>
                <Select.ItemIndicator className="app-select-check">
                  <Check size={14} />
                </Select.ItemIndicator>
              </Select.Item>
            ))}
          </Select.Viewport>
          <Select.ScrollDownButton className="app-select-scroll">
            <ChevronDown size={14} />
          </Select.ScrollDownButton>
        </Select.Content>
      </Select.Portal>
    </Select.Root>
  );
}
