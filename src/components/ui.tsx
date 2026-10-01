import type { ReactNode } from "react";

const base =
  "inline-flex items-center justify-center gap-1.5 whitespace-nowrap font-medium transition-colors disabled:cursor-not-allowed disabled:opacity-40";

const fillHover = "hover:bg-[color-mix(in_srgb,var(--accent-fill)_88%,black)]";
const raisedHover = "hover:bg-[color-mix(in_srgb,var(--elevated)_93%,var(--text))]";

export const btn = {
  primary: `${base} h-8 rounded-lg bg-accent-fill px-3.5 text-[13.5px] font-semibold text-on-accent ${fillHover}`,
  secondary: `${base} h-8 rounded-lg bg-elevated px-3.5 text-[13.5px] text-text shadow-card ${raisedHover}`,
  ghost: `${base} h-8 rounded-lg px-3 text-[13.5px] text-muted hover:bg-hover hover:text-text`,
  danger: `${base} h-8 rounded-lg px-3 text-[13.5px] text-danger hover:bg-danger/10`,
  smallPrimary: `${base} h-7 rounded-md bg-accent-fill px-3 text-[13px] font-semibold text-on-accent ${fillHover}`,
  smallSecondary: `${base} h-7 rounded-md bg-elevated px-2.5 text-[13px] text-text shadow-card ${raisedHover}`,
  smallGhost: `${base} h-7 rounded-md px-2.5 text-[13px] text-muted hover:bg-hover hover:text-text`,
  icon: `${base} size-8 shrink-0 rounded-lg text-muted hover:bg-hover hover:text-text`,
  smallIcon: `${base} size-7 shrink-0 rounded-md text-faint hover:bg-hover hover:text-text`,
};

const inputBase =
  "h-9 w-full min-w-0 rounded-lg px-3 text-[14px] text-text outline-none placeholder:text-faint transition-shadow focus:shadow-[0_0_0_1px_var(--accent),0_0_0_3px_color-mix(in_srgb,var(--accent)_28%,transparent)] focus-visible:outline-none";

export const inputClass = `${inputBase} bg-fill`;
export const monoInputClass = `${inputClass} font-mono text-[13px]`;

/** Fields that sit on a Group surface need their own fill to stay visible. */
export const groupInputClass = `${inputBase} bg-elevated shadow-card`;
export const monoGroupInputClass = `${groupInputClass} font-mono text-[13px]`;

/** Small tracked capitals for section labels, like the lettering on a directory board. */
export const labelClass = "text-[12px] font-semibold uppercase tracking-[0.13em] text-muted";

export const displayClass = "font-display font-normal tracking-[0.01em]";

export function Kbd({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <kbd
      className={`whitespace-nowrap rounded-md bg-fill px-1.5 py-[3px] font-sans text-[13px] font-medium leading-none text-faint ${className}`}
    >
      {children}
    </kbd>
  );
}

export function Dot({ color, className = "" }: { color: string; className?: string }) {
  return (
    <span
      aria-hidden="true"
      className={`inline-block size-[7px] shrink-0 rotate-45 ${className}`}
      style={{ background: color }}
    />
  );
}

export function Pill({ color, children }: { color: string; children: ReactNode }) {
  return (
    <span
      className="inline-flex h-6 shrink-0 items-center gap-2 whitespace-nowrap rounded-md px-2.5 text-[11.5px] font-semibold uppercase tracking-[0.1em]"
      style={{ background: `color-mix(in srgb, ${color} 13%, transparent)`, color }}
    >
      <span aria-hidden="true" className="size-[5px] rotate-45" style={{ background: color }} />
      {children}
    </span>
  );
}

/** The house sigil: an eye set in a lozenge. */
export function Mark({ size = 26 }: { size?: number }) {
  return (
    <svg
      aria-hidden="true"
      viewBox="0 0 24 24"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      strokeWidth={size >= 40 ? 1 : 1.4}
      strokeLinejoin="miter"
      className="shrink-0 text-gilt"
    >
      <path d="M12 1.2L22.8 12 12 22.8 1.2 12z" />
      <path d="M5.4 12Q12 5.6 18.6 12Q12 18.4 5.4 12z" />
      <circle cx="12" cy="12" r="2" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function SrOnly({ children }: { children: ReactNode }) {
  return <span className="sr-only">{children}</span>;
}

export function Switch({
  checked,
  label,
  onChange,
  disabled,
}: {
  checked: boolean;
  label: string;
  onChange: (on: boolean) => void;
  disabled?: boolean;
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={label}
      disabled={disabled}
      onClick={() => onChange(!checked)}
      className={`relative inline-flex h-[22px] w-[38px] shrink-0 items-center rounded-md transition-colors duration-150 disabled:opacity-40 ${
        checked ? "bg-accent-fill" : "bg-line-strong"
      }`}
    >
      <span
        className={`inline-block size-[16px] rounded-[1px] bg-[#fbf7ee] shadow-[0_1px_3px_rgb(0_0_0/0.35)] transition-transform duration-150 ${
          checked ? "translate-x-[19px]" : "translate-x-[3px]"
        }`}
      />
    </button>
  );
}

export function Segmented<T extends string>({
  label,
  value,
  options,
  onChange,
}: {
  label: string;
  value: T;
  options: { value: T; label: string }[];
  onChange: (value: T) => void;
}) {
  return (
    <div role="radiogroup" aria-label={label} className="inline-flex rounded-xl bg-fill p-[3px]">
      {options.map((o) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            type="button"
            role="radio"
            aria-checked={on}
            onClick={() => onChange(o.value)}
            className={`h-7 rounded-md px-3.5 text-[13px] font-medium transition-colors ${
              on ? "bg-elevated text-text shadow-card" : "text-muted hover:text-text"
            }`}
          >
            {o.label}
          </button>
        );
      })}
    </div>
  );
}

/** Settings-style inset group: a title, then rows on a framed surface. */
export function Group({
  title,
  description,
  action,
  children,
}: {
  title?: string;
  description?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="flex flex-col gap-2">
      {(title || action) && (
        <div className="flex items-end gap-3 px-1">
          <div className="min-w-0 flex-1">
            {title && <h3 className={labelClass}>{title}</h3>}
            {description && <p className="mt-1 text-[13px] leading-snug text-faint">{description}</p>}
          </div>
          {action}
        </div>
      )}
      <div className="overflow-hidden rounded-xl bg-group shadow-card">{children}</div>
    </section>
  );
}

export function Row({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <div className={`flex min-h-12 items-center gap-3 border-t border-line px-4 py-2.5 first:border-t-0 ${className}`}>
      {children}
    </div>
  );
}

export function Message({ tone, children }: { tone: "ok" | "error"; children: ReactNode }) {
  return tone === "error" ? (
    <p role="alert" className="selectable break-words text-[13px] leading-snug text-danger">
      {children}
    </p>
  ) : (
    <p className="text-[13px] leading-snug text-muted">{children}</p>
  );
}
