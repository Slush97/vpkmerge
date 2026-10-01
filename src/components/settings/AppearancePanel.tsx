import { useState } from "react";
import { api, errorText, type Settings, type SettingsPatch, type Theme } from "../../lib/api";
import { Icon } from "../../lib/icons";
import { ACCENTS, applyTheme, DEFAULT_ACCENT } from "../../lib/theme";
import { Group, Message, Row, Segmented } from "../ui";

interface Props {
  settings: Settings | null;
  onSettingsChange: (s: Settings) => void;
}

export function AppearancePanel({ settings, onSettingsChange }: Props) {
  const [error, setError] = useState<string | null>(null);
  const theme = settings?.theme ?? "system";
  const accent = (settings?.accent ?? DEFAULT_ACCENT).toLowerCase();

  const save = (patch: SettingsPatch) => {
    setError(null);
    applyTheme(patch.theme ?? theme, patch.accent ?? accent);
    api
      .updateSettings(patch)
      .then(onSettingsChange)
      .catch((e) => {
        setError(errorText(e));
        applyTheme(theme, accent);
      });
  };

  return (
    <div className="flex flex-col gap-7">
      <Group title="Theme">
        <Row>
          <span className="flex-1 text-[14px] text-text">Appearance</span>
          <Segmented<Theme>
            label="Theme"
            value={theme}
            options={[
              { value: "system", label: "System" },
              { value: "light", label: "Light" },
              { value: "dark", label: "Dark" },
            ]}
            onChange={(t) => save({ theme: t })}
          />
        </Row>
        <Row>
          <span className="flex-1 text-[13.5px] leading-snug text-muted">
            System follows your desktop's light or dark setting and switches with it.
          </span>
        </Row>
      </Group>

      <Group title="Accent color" description="Used for buttons, selection and links.">
        <Row className="py-4">
          <div role="radiogroup" aria-label="Accent color" className="flex flex-wrap gap-x-3 gap-y-4">
            {ACCENTS.map((a) => {
              const on = a.value === accent;
              return (
                <button
                  key={a.value}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  aria-label={a.name}
                  title={a.name}
                  onClick={() => save({ accent: a.value })}
                  className="group flex w-[68px] flex-col items-center gap-1.5"
                >
                  <span className="inline-flex size-11 items-center justify-center">
                    <span
                      className={`inline-flex size-[26px] rotate-45 items-center justify-center transition-[transform,box-shadow] duration-150 ${
                        on ? "" : "group-hover:scale-110"
                      }`}
                      style={{
                        background: a.value,
                        boxShadow: on ? `0 0 0 3px var(--group), 0 0 0 4px ${a.value}` : undefined,
                      }}
                    >
                      {on && <Icon name="check" size={14} strokeWidth={3} className="-rotate-45 text-[#18140e]" />}
                    </span>
                  </span>
                  <span className={`text-[13px] ${on ? "font-medium text-text" : "text-muted"}`}>{a.name}</span>
                </button>
              );
            })}
          </div>
        </Row>
      </Group>

      {error && <Message tone="error">{error}</Message>}
    </div>
  );
}
