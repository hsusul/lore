import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";

import {
  clusterAt,
  dotAt,
  draw,
  fit,
  onto,
  zoomAbout,
  type Frame,
  type View,
} from "../map/draw";
import type { Dot, MapModel } from "../map/layout";

const reducedMotion = () =>
  window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

/**
 * The map, drawn on a canvas. Drag or two-finger scroll pans, pinch zooms
 * about the pointer, a click on a dot opens it and a click on a cluster
 * zooms onto it. Escape goes back to the whole map.
 */
export function MapCanvas({
  model,
  selectedId,
  inset,
  onSelect,
  onDeselect,
  label,
}: {
  model: MapModel;
  selectedId: string | null;
  /** Width the card covers on the right. */
  inset: number;
  onSelect: (dot: Dot) => void;
  /** A click that lands on no dot lets go of the opened one. */
  onDeselect: () => void;
  label: string;
}) {
  const wrap = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const [size, setSize] = useState({ width: 0, height: 0 });
  const frame: Frame = { width: size.width, height: size.height, inset };
  const view = useRef<View>({ s: 1, tx: 0, ty: 0 });
  const home = useRef<View>({ s: 1, tx: 0, ty: 0 });
  const focus = useRef<string | null>(null);
  const anim = useRef(0);
  const drag = useRef<{ x: number; y: number; moved: boolean } | null>(null);

  const paint = useCallback(() => {
    const c = canvas.current;
    const g = c?.getContext("2d");
    if (!c || !g) return;
    const dpr = window.devicePixelRatio || 1;
    if (
      c.width !== Math.round(size.width * dpr) ||
      c.height !== Math.round(size.height * dpr)
    ) {
      c.width = Math.round(size.width * dpr);
      c.height = Math.round(size.height * dpr);
    }
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    // A token can hold a WebKit system color keyword, which a canvas cannot
    // parse; the browser resolves it when it is set as an element's color.
    const token = (name: string) => {
      c.style.color = `var(${name})`;
      return getComputedStyle(c).color;
    };
    draw(
      g,
      model,
      view.current,
      { width: size.width, height: size.height, inset },
      home.current,
      {
        ink: token("--mist-ac"),
        quiet: token("--mist-dotm"),
        name: token("--mist-t1"),
        count: token("--mist-t2"),
        ground: token("--mist-bg"),
        font: getComputedStyle(c).fontFamily,
      },
      selectedId,
      focus.current,
    );
  }, [model, size, inset, selectedId]);

  const glide = useCallback(
    (to: View) => {
      cancelAnimationFrame(anim.current);
      const from = { ...view.current };
      const t0 = performance.now();
      const duration = reducedMotion() ? 0 : 420;
      const step = (now: number) => {
        const k = duration === 0 ? 1 : Math.min(1, (now - t0) / duration);
        const e = 1 - Math.pow(1 - k, 3);
        view.current = {
          s: from.s + (to.s - from.s) * e,
          tx: from.tx + (to.tx - from.tx) * e,
          ty: from.ty + (to.ty - from.ty) * e,
        };
        paint();
        if (k < 1) anim.current = requestAnimationFrame(step);
      };
      anim.current = requestAnimationFrame(step);
    },
    [paint],
  );

  useLayoutEffect(() => {
    const el = wrap.current;
    if (!el) return;
    const measure = () =>
      setSize({ width: el.clientWidth, height: el.clientHeight });
    measure();
    if (typeof ResizeObserver === "undefined") return;
    const observer = new ResizeObserver(measure);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  // A new layout or a resized window starts from the whole map.
  useEffect(() => {
    home.current = fit(model, {
      width: size.width,
      height: size.height,
      inset,
    });
    if (!focus.current) glide(home.current);
    // eslint-disable-next-line react-hooks/exhaustive-deps -- refit only when the map or the space for it changes
  }, [model, size.width, size.height, inset]);

  useEffect(() => paint(), [paint]);
  useEffect(() => () => cancelAnimationFrame(anim.current), []);

  const point = (e: { clientX: number; clientY: number }) => {
    const r = canvas.current?.getBoundingClientRect();
    return { x: e.clientX - (r?.left ?? 0), y: e.clientY - (r?.top ?? 0) };
  };

  const goHome = () => {
    focus.current = null;
    glide(home.current);
  };

  useEffect(() => {
    const el = canvas.current;
    if (!el) return;
    // Wheel listeners must not be passive to keep the page from scrolling.
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      cancelAnimationFrame(anim.current);
      const { x, y } = point(e);
      view.current = e.ctrlKey
        ? zoomAbout(view.current, Math.exp(-e.deltaY * 0.01), x, y)
        : {
            ...view.current,
            tx: view.current.tx - e.deltaX,
            ty: view.current.ty - e.deltaY,
          };
      paint();
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [paint]);

  return (
    <div ref={wrap} className="map__field">
      <canvas
        ref={canvas}
        className="map__canvas"
        role="img"
        aria-label={label}
        tabIndex={0}
        style={{ width: size.width, height: size.height }}
        onKeyDown={(e) => {
          if (e.key === "Escape") {
            onDeselect();
            goHome();
          }
        }}
        onPointerDown={(e) => {
          drag.current = { ...point(e), moved: false };
          e.currentTarget.setPointerCapture?.(e.pointerId);
        }}
        onPointerMove={(e) => {
          const d = drag.current;
          if (!d) {
            const { x, y } = point(e);
            e.currentTarget.style.cursor = dotAt(model, view.current, x, y)
              ? "pointer"
              : "grab";
            return;
          }
          const { x, y } = point(e);
          if (!d.moved && Math.hypot(x - d.x, y - d.y) < 3) return;
          d.moved = true;
          cancelAnimationFrame(anim.current);
          view.current = {
            ...view.current,
            tx: view.current.tx + x - d.x,
            ty: view.current.ty + y - d.y,
          };
          d.x = x;
          d.y = y;
          paint();
        }}
        onPointerUp={(e) => {
          const d = drag.current;
          drag.current = null;
          if (!d || d.moved) return;
          const { x, y } = point(e);
          const dot = dotAt(model, view.current, x, y);
          if (dot) {
            onSelect(dot);
            return;
          }
          onDeselect();
          const cluster = clusterAt(model, view.current, x, y);
          if (cluster && cluster.key !== focus.current) {
            focus.current = cluster.key;
            glide(onto(cluster, home.current, frame));
          } else if (!cluster) {
            goHome();
          }
        }}
      />
    </div>
  );
}
