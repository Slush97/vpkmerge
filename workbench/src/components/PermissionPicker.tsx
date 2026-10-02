import { useEffect, useRef, useState } from "react";
import { api, errorText, type PermissionLevel, type Settings } from "../lib/api";
import { Icon } from "../lib/icons";

const levels: { value: PermissionLevel; label: string; detail: string }[] = [
  { value: "readOnly", label: "Read only", detail: "Looks but changes nothing. Edits and commands are blocked." },
  { value: "ask", label: "Ask first", detail: "Reads freely. Asks before edits, commands and tools that change things." },
  { value: "autoEdit", label: "Auto-edit", detail: "Edits files without asking. Still asks before commands and other tools." },
  { value: "fullAccess", label: "Full access", detail: "Runs everything without asking." },
];

interface Props {
  settings: Settings | null;
  onChange: (settings: Settings) => void;
}

export function PermissionPicker({ settings, onChange }: Props) {
  const [open, setOpen] = useState(false);
  const [below, setBelow] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const current = levels.find((l) => l.value === settings?.permissionLevel) ?? levels[1];

  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (root.current && !root.current.contains(e.target as Node)) setOpen(false);
    };
    document.addEventListener("mousedown", onDown);
    return () => document.removeEventListener("mousedown", onDown);
  }, [open]);

  const choose = (level: PermissionLevel) => {
    setError(null);
    api
      .updateSettings({ permissionLevel: level })
      .then((s) => {
        onChange(s);
        setOpen(false);
      })
      .catch((e) => setError(errorText(e)));
  };

  return (
    <div
      ref={root}
      className="relative"
      onKeyDown={(e) => {
        if (e.key === "Escape" && open) {
          e.stopPropagation();
          setOpen(false);
        }
      }}
    >
      <button
        type="button"
        onClick={() => {
          if (!open && root.current) {
            const r = root.current.getBoundingClientRect();
            const above = r.top - 64;
            setBelow(above < 300 && window.innerHeight - r.bottom - 16 > above);
          }
          setOpen((o) => !o);
        }}
        aria-expanded={open}
        aria-haspopup="true"
        title="What tools may do without asking"
        className={`inline-flex h-8 items-center gap-1.5 whitespace-nowrap rounded-lg px-2.5 text-[13.5px] font-medium transition-colors hover:bg-hover ${
          current.value === "fullAccess" ? "text-warning" : "text-text-2"
        }`}
      >
        <Icon name="shield" size={15} className={current.value === "fullAccess" ? "" : "text-muted"} />
        <span>{current.label}</span>
        <Icon name={open ? "chevUp" : "chevDown"} size={14} className="text-faint" />
      </button>

      {open && (
        <div
          role="radiogroup"
          aria-label="Permission level"
          className={`pop-in absolute left-0 z-40 w-[340px] rounded-2xl bg-elevated p-2 shadow-pop ${
            below ? "top-full mt-2" : "bottom-full mb-2"
          }`}
        >
          {levels.map((l) => {
            const selected = l.value === current.value;
            return (
              <button
                key={l.value}
                type="button"
                role="radio"
                aria-checked={selected}
                onClick={() => choose(l.value)}
                className={`flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left transition-colors hover:bg-hover ${
                  selected ? "bg-active" : ""
                }`}
              >
                <span className="min-w-0 flex-1">
                  <span className="block text-[14px] font-medium text-text">{l.label}</span>
                  <span className="block text-[13px] leading-snug text-faint">{l.detail}</span>
                </span>
                {selected && <Icon name="check" size={16} strokeWidth={2.5} className="text-accent-ink" />}
              </button>
            );
          })}
          {error && <p className="px-3 pb-1 pt-2 text-[13px] text-danger">{error}</p>}
        </div>
      )}
    </div>
  );
}
