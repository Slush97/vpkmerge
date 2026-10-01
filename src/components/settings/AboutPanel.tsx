import { useEffect, useState } from "react";
import { api, errorText, type AppInfo } from "../../lib/api";
import { Icon } from "../../lib/icons";
import { btn, Group, Mark, Message, Row } from "../ui";

export function AboutPanel() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api.appInfo().then(setInfo).catch((e) => setError(errorText(e)));
  }, []);

  if (error) return <Message tone="error">{error}</Message>;
  if (!info) return null;

  return (
    <div className="flex flex-col gap-7">
      <div className="flex items-center gap-4 px-1">
        <Mark size={48} />
        <div>
          <div className="font-display text-[20px] font-normal uppercase tracking-[0.22em]">Workbench</div>
          <div className="text-[13.5px] text-muted">Version {info.version}</div>
        </div>
      </div>

      <Group title="Data">
        <Row>
          <span className="w-[140px] shrink-0 text-[14px] text-text">Data folder</span>
          <span className="selectable min-w-0 flex-1 truncate font-mono text-[13px] text-text-2" title={info.dataDir}>
            {info.dataDir}
          </span>
          <button
            type="button"
            onClick={() => void api.openDataDir().catch((e) => setError(errorText(e)))}
            className={btn.smallSecondary}
          >
            <Icon name="folder" size={13} />
            Open
          </button>
        </Row>
        <Row className="items-start py-3">
          <span className="w-[140px] shrink-0 text-[14px] text-text">Credentials</span>
          {info.secretStorage === "keychain" ? (
            <span className="inline-flex flex-1 items-center gap-1.5 text-[14px] text-text-2">
              <Icon name="checkCircle" size={15} className="text-success" />
              OS keychain
            </span>
          ) : (
            <p className="flex-1 text-[13.5px] leading-snug text-text-2">
              <span className="font-medium text-warning">Owner-only file. </span>
              No keychain service was found, so keys and tokens live in a private file in the data folder. With GNOME
              Keyring or KWallet running, restart Workbench and sign in again to use the keychain.
            </p>
          )}
        </Row>
      </Group>

      <Group title="Privacy">
        <Row>
          <p className="text-[14px] leading-snug text-text-2">
            Workbench sends nothing anywhere except the model, agent and MCP requests you make.
          </p>
        </Row>
      </Group>
    </div>
  );
}
