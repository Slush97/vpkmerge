const polar = (r: number, deg: number) => {
  const a = (deg * Math.PI) / 180;
  return `${(r * Math.cos(a)).toFixed(2)} ${(r * Math.sin(a)).toFixed(2)}`;
};

const spokes = (count: number, from: number, to: (i: number) => number) =>
  Array.from({ length: count }, (_, i) => {
    const deg = (360 / count) * i;
    return `M${polar(from, deg)}L${polar(to(i), deg)}`;
  }).join("");

const ticks = spokes(72, 67, (i) => (i % 6 === 0 ? 59 : 63));
const rays = spokes(40, 28, (i) => (i % 2 === 0 ? 52 : 41));
const studs = [0, 90, 180, 270]
  .map((deg) => {
    const a = (deg * Math.PI) / 180;
    const x = 76 * Math.cos(a);
    const y = 76 * Math.sin(a);
    return `M${(x - 4).toFixed(2)} ${y.toFixed(2)}L${x.toFixed(2)} ${(y - 4).toFixed(2)}L${(x + 4).toFixed(2)} ${y.toFixed(2)}L${x.toFixed(2)} ${(y + 4).toFixed(2)}z`;
  })
  .join("");

/** The eye in glory inside a dial of hours: the emblem on the empty state. */
export function Seal({ size = 136 }: { size?: number }) {
  return (
    <svg
      aria-hidden="true"
      viewBox="-82 -82 164 164"
      width={size}
      height={size}
      fill="none"
      stroke="currentColor"
      className="shrink-0 text-gilt"
    >
      <circle r="76" strokeWidth="1" />
      <circle r="71.5" strokeWidth="0.6" opacity="0.55" />
      <path d={ticks} strokeWidth="0.8" opacity="0.8" className="seal-turn" />
      <circle r="55.5" strokeWidth="0.6" opacity="0.55" />
      <path d={rays} strokeWidth="0.8" opacity="0.7" />
      <path d={studs} fill="var(--bg)" strokeWidth="1" />
      <path d="M-23 0Q0 -18 23 0Q0 18 -23 0z" strokeWidth="1.2" />
      <circle r="7.6" strokeWidth="1.2" />
      <circle r="2.7" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** Three stepped rules, shortest on the outside, that flank the seal. */
export function Whiskers({ flip = false }: { flip?: boolean }) {
  return (
    <span aria-hidden="true" className={`flex flex-1 flex-col gap-[5px] ${flip ? "items-start" : "items-end"}`}>
      {[44, 100, 44].map((width, i) => (
        <span
          key={i}
          className="h-px"
          style={{
            width: `${width}%`,
            background: `linear-gradient(to ${flip ? "right" : "left"}, var(--line-strong), transparent)`,
          }}
        />
      ))}
    </span>
  );
}
