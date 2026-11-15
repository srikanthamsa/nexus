import { useEffect, useRef } from "react";

interface Star {
  x: number;
  y: number;
  z: number;
  tint: number;
  size: number;
}

const STAR_COUNT = 420;

function hexToRgb(value: string, fallback: [number, number, number]): [number, number, number] {
  const hex = value.trim().replace(/^#/, "");
  const full = hex.length === 3 ? hex.split("").map((char) => char + char).join("") : hex;
  const parsed = Number.parseInt(full, 16);
  return Number.isFinite(parsed) && full.length === 6
    ? [(parsed >> 16) & 255, (parsed >> 8) & 255, parsed & 255]
    : fallback;
}

function spawn(z: number): Star {
  const angle = Math.random() * Math.PI * 2;
  const radius = 0.12 + Math.pow(Math.random(), 0.55) * 1.16;
  return {
    x: Math.cos(angle) * radius,
    y: Math.sin(angle) * radius,
    z,
    tint: Math.random(),
    size: 0.55 + Math.random() * 1.25,
  };
}

export function WarpField() {
  const canvasRef = useRef<HTMLCanvasElement>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const ctx = canvas?.getContext("2d");
    if (!canvas || !ctx) return;

    const tone = hexToRgb(getComputedStyle(canvas).getPropertyValue("--tone"), [253, 240, 1]);
    const toneTwo = hexToRgb(
      getComputedStyle(canvas).getPropertyValue("--tone-2"),
      [215, 243, 106],
    );
    const palette = [
      [255, 255, 255],
      [236, 233, 255],
      tone,
      toneTwo,
      [215, 243, 106],
    ] as const;

    const stars = Array.from({ length: STAR_COUNT }, () =>
      spawn(0.08 + Math.random() * 1.42),
    );
    let width = 0;
    let height = 0;
    let frame = 0;
    let last = performance.now();
    const start = last;

    const resize = () => {
      const dpr = Math.min(window.devicePixelRatio || 1, 2);
      width = window.innerWidth;
      height = window.innerHeight;
      canvas.width = Math.round(width * dpr);
      canvas.height = Math.round(height * dpr);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    };

    const draw = (now: number) => {
      const dt = Math.min(0.05, (now - last) / 1000);
      last = now;
      const elapsed = now - start;
      const progress = Math.min(1, elapsed / 860);
      const drive = Math.pow(progress, 2.4);
      const appear = Math.min(1, elapsed / 260);
      const speed = 0.06 + drive * 6.4;
      const cx = width / 2;
      const cy = height / 2;
      const focal = Math.min(width, height) * 0.56;

      ctx.clearRect(0, 0, width, height);
      ctx.globalCompositeOperation = "lighter";

      const haloRadius = Math.min(width, height) * (0.16 + appear * 0.08 + drive * 0.18);
      const halo = ctx.createRadialGradient(cx, cy, 0, cx, cy, haloRadius);
      halo.addColorStop(0, `rgba(255, 255, 255, ${0.14 + appear * 0.1 + drive * 0.14})`);
      halo.addColorStop(0.22, `rgba(${toneTwo.join(", ")}, ${0.08 + appear * 0.07 + drive * 0.1})`);
      halo.addColorStop(0.6, `rgba(${tone.join(", ")}, ${0.04 + appear * 0.02 + drive * 0.01})`);
      halo.addColorStop(1, "rgba(0, 0, 0, 0)");
      ctx.fillStyle = halo;
      ctx.fillRect(0, 0, width, height);

      for (const star of stars) {
        star.z -= speed * dt;
        if (star.z < 0.05) {
          Object.assign(star, spawn(1.28 + Math.random() * 0.55));
        }

        const z = Math.max(0.05, star.z);
        const trail = z + 0.02 + speed * 0.06;
        const x = cx + (star.x * focal) / z;
        const y = cy + (star.y * focal) / z;
        const px = cx + (star.x * focal) / trail;
        const py = cy + (star.y * focal) / trail;

        if (
          (x < -120 && px < -120) ||
          (x > width + 120 && px > width + 120) ||
          (y < -120 && py < -120) ||
          (y > height + 120 && py > height + 120)
        ) {
          continue;
        }

        const depth = 1 - Math.min(1, z / 1.7);
        const alpha = (0.12 + depth * 0.78) * (0.22 + appear * 0.55 + drive * 0.23);
        const color = palette[Math.min(palette.length - 1, Math.floor(star.tint * palette.length))];
        ctx.strokeStyle = `rgba(${color.join(", ")}, ${alpha})`;
        ctx.lineWidth = (0.55 + depth * 1.9) * star.size * (0.75 + drive * 0.35);
        ctx.lineCap = "round";
        ctx.beginPath();
        ctx.moveTo(px, py);
        ctx.lineTo(x, y);
        ctx.stroke();
      }

      const coreRadius = Math.min(width, height) * (0.045 + appear * 0.05 + drive * 0.075);
      const core = ctx.createRadialGradient(cx, cy, 0, cx, cy, coreRadius);
      core.addColorStop(0, `rgba(255, 255, 255, ${appear * (0.55 + drive * 0.44)})`);
      core.addColorStop(0.2, `rgba(255, 255, 255, ${appear * (0.28 + drive * 0.38)})`);
      core.addColorStop(0.5, `rgba(${toneTwo.join(", ")}, ${appear * (0.12 + drive * 0.16)})`);
      core.addColorStop(1, "rgba(0, 0, 0, 0)");
      ctx.fillStyle = core;
      ctx.fillRect(0, 0, width, height);

      ctx.globalCompositeOperation = "source-over";
      const vignette = ctx.createRadialGradient(
        cx,
        cy,
        Math.min(width, height) * 0.24,
        cx,
        cy,
        Math.max(width, height) * 0.78,
      );
      vignette.addColorStop(0, "rgba(3, 4, 8, 0)");
      vignette.addColorStop(1, "rgba(3, 4, 8, 0.48)");
      ctx.fillStyle = vignette;
      ctx.fillRect(0, 0, width, height);

      frame = window.requestAnimationFrame(draw);
    };

    resize();
    window.addEventListener("resize", resize);
    frame = window.requestAnimationFrame(draw);

    return () => {
      window.cancelAnimationFrame(frame);
      window.removeEventListener("resize", resize);
    };
  }, []);

  return <canvas ref={canvasRef} className="connect-warp__field" aria-hidden="true" />;
}
