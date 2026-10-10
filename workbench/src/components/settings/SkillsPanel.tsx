import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errorText, type Skill } from "../../lib/api";
import { Icon } from "../../lib/icons";
import { btn, Group, Message, Row, Switch } from "../ui";

export function SkillsPanel() {
  const [dirs, setDirs] = useState<string[]>([]);
  const [skills, setSkills] = useState<Skill[]>([]);
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [d, s] = await Promise.all([api.skillDirs(), api.listSkills()]);
      setDirs(d);
      setSkills(s);
    } catch (e) {
      setError(errorText(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const act = async (action: () => Promise<unknown>) => {
    setError(null);
    try {
      await action();
    } catch (e) {
      setError(errorText(e));
    }
    await refresh();
  };

  const addFolder = async () => {
    const picked = await open({ directory: true, multiple: false, title: "Add a skills folder" });
    if (typeof picked === "string") await act(() => api.addSkillDir(picked));
  };

  return (
    <div className="flex flex-col gap-7">
      <Group
        title="Skills"
        description="The model sees each description and loads a skill when a request matches it."
      >
        {skills.length === 0 ? (
          <Row>
            <span className="text-[14px] text-muted">No skills found in these folders.</span>
          </Row>
        ) : (
          skills.map((s) => (
            <Row key={s.name} className="items-start py-3">
              <div className="min-w-0 flex-1">
                <div className="font-mono text-[14px] font-medium text-text">{s.name}</div>
                {s.description && <p className="mt-0.5 text-[13.5px] leading-snug text-muted">{s.description}</p>}
                <div className="mt-1 truncate font-mono text-[13px] text-faint" title={s.dir}>
                  {s.dir}
                </div>
              </div>
              <Switch
                checked={s.enabled}
                label={`Enable ${s.name}`}
                onChange={(on) => void act(() => api.setSkillEnabled(s.name, on))}
              />
            </Row>
          ))
        )}
      </Group>

      <Group
        title="Folders"
        description={
          <>
            A skill is a folder with a <code className="font-mono">SKILL.md</code> whose front matter has a name and a
            description.
          </>
        }
        action={
          <button type="button" onClick={() => void act(addFolder)} className={btn.smallSecondary}>
            <Icon name="plus" size={13} />
            Add folder
          </button>
        }
      >
        {dirs.map((d, i) => (
          <Row key={d}>
            <Icon name="folder" size={16} className="text-muted" />
            <span className="selectable min-w-0 flex-1 truncate font-mono text-[13px] text-text-2" title={d}>
              {d}
            </span>
            {i === 0 ? (
              <span className="text-[13px] text-faint">Built-in</span>
            ) : (
              <button
                type="button"
                aria-label={`Remove ${d}`}
                title="Remove"
                onClick={() => void act(() => api.removeSkillDir(d))}
                className={`${btn.smallIcon} hover:text-danger`}
              >
                <Icon name="trash" size={14} />
              </button>
            )}
          </Row>
        ))}
      </Group>

      {error && <Message tone="error">{error}</Message>}
    </div>
  );
}
