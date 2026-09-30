import { getCurrentWindow } from "@tauri-apps/api/window";
import { Icon } from "../lib/icons";

const control =
  "inline-flex size-8 items-center justify-center rounded-full text-muted transition-colors hover:bg-hover hover:text-text";

export function WindowControls() {
  return (
    <div className="flex shrink-0 items-center gap-1 pl-1">
      <button type="button" aria-label="Minimize" className={control} onClick={() => void getCurrentWindow().minimize()}>
        <Icon name="minus" size={15} />
      </button>
      <button type="button" aria-label="Maximize" className={control} onClick={() => void getCurrentWindow().toggleMaximize()}>
        <Icon name="square" size={13} />
      </button>
      <button
        type="button"
        aria-label="Close window"
        className={`${control} hover:bg-[#e5484d] hover:text-white`}
        onClick={() => void getCurrentWindow().close()}
      >
        <Icon name="x" size={15} />
      </button>
    </div>
  );
}
