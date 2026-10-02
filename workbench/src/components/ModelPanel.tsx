import { useEffect, useRef, useState } from "react";
import {
  AnimationMixer,
  Box3,
  DirectionalLight,
  LoopRepeat,
  Mesh,
  NeutralToneMapping,
  PerspectiveCamera,
  PMREMGenerator,
  Scene,
  SkinnedMesh,
  Texture,
  Timer,
  Vector3,
  WebGLRenderer,
  type AnimationAction,
  type AnimationClip,
  type Object3D,
} from "three";
import { OrbitControls } from "three/addons/controls/OrbitControls.js";
import { RoomEnvironment } from "three/addons/environments/RoomEnvironment.js";
import { GLTFLoader } from "three/addons/loaders/GLTFLoader.js";
import { api, errorText } from "../lib/api";
import { Icon } from "../lib/icons";
import { animationLabel, fileName, type ModelPreview } from "../lib/preview";
import { ResizeHandle, useStoredWidth } from "./ResizeHandle";
import { btn, displayClass, inputClass } from "./ui";

type Status = { kind: "loading" } | { kind: "ready" } | { kind: "failed"; error: string };

/** What the animation controls drive once the model is up. */
interface Player {
  mixer: AnimationMixer;
  action: AnimationAction | null;
  /** Fetched animations by name, so switching back costs nothing. */
  clips: Map<string, AnimationClip>;
  render: () => void;
}

/** Shows a tool-built .glb. Loaded lazily: three.js is most of its weight. */
export default function ModelPanel({ preview, onClose }: { preview: ModelPreview; onClose: () => void }) {
  const host = useRef<HTMLDivElement>(null);
  const size = useStoredWidth("workbench.previewWidth", 440, 320, 1200);
  const scrub = useRef<HTMLInputElement>(null);
  const resetView = useRef(() => {});
  const player = useRef<Player | null>(null);
  const [status, setStatus] = useState<Status>({ kind: "loading" });
  const [animations, setAnimations] = useState<{ prefix: string; names: string[] } | null>(null);
  const [current, setCurrent] = useState<{ name: string; duration: number } | null>(null);
  const [playing, setPlaying] = useState(false);
  const [fetching, setFetching] = useState(false);
  const [clipError, setClipError] = useState<string | null>(null);

  useEffect(() => {
    const el = host.current!;
    let alive = true;
    setStatus({ kind: "loading" });
    setCurrent(null);
    setPlaying(false);

    const renderer = new WebGLRenderer({ antialias: true, alpha: true });
    renderer.setPixelRatio(window.devicePixelRatio);
    renderer.toneMapping = NeutralToneMapping;
    el.appendChild(renderer.domElement);

    const scene = new Scene();
    const pmrem = new PMREMGenerator(renderer);
    const env = pmrem.fromScene(new RoomEnvironment(), 0.04);
    scene.environment = env.texture;
    const key = new DirectionalLight(0xffffff, 1.2);
    key.position.set(2, 3, 2);
    scene.add(key);

    const camera = new PerspectiveCamera(30, 1, 1, 10_000);
    const controls = new OrbitControls(camera, renderer.domElement);
    // Drawn on demand, and every frame only while an animation plays.
    const render = () => renderer.render(scene, camera);
    controls.addEventListener("change", render);

    // Reallocating the drawing buffer on every frame of a divider drag crashes WebKitGTK's GL
    // on NVIDIA, so the buffer only follows once the size settles. Until then the canvas
    // stretches to fit, and the camera's aspect keeps the stretched picture in proportion.
    renderer.domElement.className = "block size-full";
    const fit = (buffer: boolean) => {
      const { clientWidth: w, clientHeight: h } = el;
      if (!w || !h) return;
      if (buffer) renderer.setSize(w, h, false);
      camera.aspect = w / h;
      camera.updateProjectionMatrix();
      render();
    };
    fit(true);
    let settle = 0;
    const observer = new ResizeObserver(() => {
      fit(false);
      clearTimeout(settle);
      settle = window.setTimeout(() => fit(true), 150);
    });
    observer.observe(el);

    const timer = new Timer();
    let frame = 0;
    const tick = (now: number) => {
      frame = requestAnimationFrame(tick);
      timer.update(now);
      const p = player.current;
      if (!p?.action || p.action.paused) return;
      p.mixer.update(timer.getDelta());
      render();
      if (scrub.current) scrub.current.value = String(p.action.time);
    };
    frame = requestAnimationFrame(tick);

    let model: Object3D | null = null;
    fetch(api.previewUrl(preview.glb))
      .then(async (res) => {
        if (!res.ok) throw new Error(await res.text());
        return new GLTFLoader().parseAsync(await res.arrayBuffer(), "");
      })
      .then((gltf) => {
        model = gltf.scene;
        if (!alive) {
          dispose(model);
          return;
        }
        // Bounds are computed once, so an animation that leaves them must not cull the mesh.
        model.traverse((o) => {
          if (o instanceof SkinnedMesh) o.frustumCulled = false;
        });
        scene.add(model);

        const mixer = new AnimationMixer(model);
        player.current = { mixer, action: null, clips: new Map(), render };
        const pose = gltf.animations[0];
        if (preview.pose && pose) {
          player.current.clips.set(preview.pose, pose);
          const action = mixer.clipAction(pose).play();
          action.paused = true;
          mixer.update(0);
          player.current.action = action;
          setCurrent({ name: preview.pose, duration: pose.duration });
        }

        model.updateMatrixWorld(true);
        const box = new Box3().setFromObject(model);
        resetView.current = () => {
          frameModel(camera, box);
          controls.target.copy(box.getCenter(new Vector3()));
          controls.update();
          render();
        };
        resetView.current();
        setStatus({ kind: "ready" });
      })
      .catch((e: unknown) => {
        if (alive) setStatus({ kind: "failed", error: errorText(e) });
      });

    return () => {
      alive = false;
      cancelAnimationFrame(frame);
      player.current?.mixer.stopAllAction();
      player.current = null;
      observer.disconnect();
      clearTimeout(settle);
      controls.dispose();
      if (model) dispose(model);
      env.dispose();
      pmrem.dispose();
      renderer.dispose();
      renderer.domElement.remove();
    };
  }, [preview.glb, preview.pose]);

  useEffect(() => {
    setAnimations(null);
    setClipError(null);
    if (status.kind !== "ready" || !preview.pose) return;
    let alive = true;
    api
      .previewAnimations(preview.hero, preview.vpk)
      .then((r) => alive && setAnimations({ prefix: `${r.codename}_`, names: r.animations }))
      .catch((e: unknown) => alive && setClipError(errorText(e)));
    return () => {
      alive = false;
    };
  }, [status.kind, preview.hero, preview.vpk, preview.pose]);

  const choose = async (name: string) => {
    const p = player.current;
    if (!p) return;
    setClipError(null);
    let clip = p.clips.get(name);
    if (!clip) {
      setFetching(true);
      try {
        const bytes = await api.previewAnimation(preview.hero, preview.vpk, name);
        clip = (await new GLTFLoader().parseAsync(bytes, "")).animations[0];
        if (!clip) throw new Error("That animation came back empty.");
        p.clips.set(name, clip);
      } catch (e) {
        setClipError(errorText(e));
        return;
      } finally {
        setFetching(false);
      }
    }
    if (player.current !== p) return;
    p.action?.stop();
    p.action = p.mixer.clipAction(clip).reset().setLoop(LoopRepeat, Infinity).play();
    setCurrent({ name, duration: clip.duration });
    setPlaying(true);
  };

  const togglePlaying = () => {
    const action = player.current?.action;
    if (!action) return;
    action.paused = !action.paused;
    setPlaying(!action.paused);
  };

  const seek = (time: number) => {
    const p = player.current;
    if (!p?.action) return;
    p.action.paused = true;
    p.action.time = time;
    p.mixer.update(0);
    p.render();
    setPlaying(false);
  };

  const label = (name: string) => animationLabel(name, animations?.prefix ?? "");

  return (
    <aside
      aria-label="Model preview"
      // The cap keeps a wide panel from crowding the chat out of a narrow window.
      style={{ width: size.width, maxWidth: "65%" }}
      className="fade-in relative flex min-w-[320px] shrink-0 flex-col border-l-[3px] border-double border-line-strong bg-sidebar"
    >
      <ResizeHandle
        side="left"
        label="Resize preview"
        width={size.width}
        onResize={size.setWidth}
        onReset={size.reset}
      />
      <div className="flex h-14 shrink-0 items-center gap-3 border-b border-line pl-4 pr-2">
        <Icon name="cube" size={16} className="text-accent-ink" />
        <div className="min-w-0 flex-1">
          <div className={`truncate text-[16px] leading-tight ${displayClass}`}>{preview.hero}</div>
          <div className="truncate text-[12.5px] text-faint">{preview.vpk ? fileName(preview.vpk) : "Base game"}</div>
        </div>
        <button
          type="button"
          aria-label="Reset view"
          title="Reset view"
          disabled={status.kind !== "ready"}
          onClick={() => resetView.current()}
          className={btn.icon}
        >
          <Icon name="refresh" size={16} />
        </button>
        <button type="button" aria-label="Close preview" onClick={onClose} className={btn.icon}>
          <Icon name="x" size={17} />
        </button>
      </div>
      <div className="relative min-h-0 flex-1">
        <div ref={host} className="absolute inset-0" />
        {status.kind === "loading" && (
          <div className="pointer-events-none absolute inset-0 flex items-center justify-center gap-2 text-[13.5px] text-muted">
            <Icon name="loader" size={15} className="spin text-running" />
            Loading model
          </div>
        )}
        {status.kind === "failed" && (
          <div role="alert" className="absolute inset-0 flex items-center justify-center p-6">
            <div className="flex max-w-[320px] gap-2.5 text-[13.5px] leading-snug text-danger">
              <Icon name="alertCircle" size={16} className="mt-px" />
              <span className="selectable">{status.error}</span>
            </div>
          </div>
        )}
      </div>
      {status.kind === "ready" && current && (
        <div className="flex shrink-0 flex-col gap-2 border-t border-line px-3 py-2.5">
          <div className="flex items-center gap-2">
            <button
              type="button"
              aria-label={playing ? "Pause" : "Play"}
              onClick={togglePlaying}
              disabled={current.duration === 0}
              className={btn.icon}
            >
              <Icon name={playing ? "pause" : "play"} size={15} />
            </button>
            <div className="relative min-w-0 flex-1">
              <select
                aria-label="Animation"
                value={current.name}
                disabled={!animations || fetching}
                onChange={(e) => void choose(e.target.value)}
                className={`${inputClass} appearance-none truncate pr-7`}
              >
                {(animations?.names ?? [current.name]).map((name) => (
                  <option key={name} value={name}>
                    {label(name)}
                  </option>
                ))}
              </select>
              <Icon
                name={fetching ? "loader" : "chevDown"}
                size={14}
                className={`pointer-events-none absolute right-2.5 top-[11px] text-faint ${fetching ? "spin" : ""}`}
              />
            </div>
          </div>
          {current.duration > 0 && (
            <input
              key={current.name}
              ref={scrub}
              type="range"
              aria-label="Animation time"
              min={0}
              max={current.duration}
              step={0.001}
              defaultValue={0}
              onChange={(e) => seek(Number(e.currentTarget.value))}
              className="w-full"
              style={{ accentColor: "var(--accent)" }}
            />
          )}
          {clipError && <div className="selectable text-[12.5px] leading-snug text-danger">{clipError}</div>}
        </div>
      )}
      <div className="shrink-0 border-t border-line px-4 py-2 text-[12.5px] text-faint">
        Drag to turn, scroll to zoom, right-drag to pan.
      </div>
    </aside>
  );
}

/** Puts the whole model in view from the front, which glTF puts at +Z. */
function frameModel(camera: PerspectiveCamera, box: Box3) {
  const size = box.getSize(new Vector3());
  const center = box.getCenter(new Vector3());
  const tan = Math.tan((camera.fov * Math.PI) / 360);
  // Headroom past the pose's bounds, which most animations reach beyond.
  const distance = 1.35 * Math.max(size.y / 2 / tan, size.x / 2 / tan / camera.aspect) + size.z / 2;
  camera.position.set(center.x, center.y + size.y * 0.05, center.z + distance);
  camera.near = distance / 100;
  camera.far = distance * 100;
  camera.updateProjectionMatrix();
}

function dispose(root: Object3D) {
  root.traverse((o) => {
    if (!(o instanceof Mesh)) return;
    o.geometry.dispose();
    for (const material of [o.material].flat()) {
      for (const value of Object.values(material)) if (value instanceof Texture) value.dispose();
      material.dispose();
    }
  });
}
