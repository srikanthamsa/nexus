import { useId } from "react";

export function Sparkline({
  values,
  height = 56,
  className,
}: {
  values: number[];
  height?: number;
  className?: string;
}) {
  const rawId = useId();
  const gradientId = `spark-${rawId.replace(/:/g, "")}`;
  const width = 240;
  const max = Math.max(...values, 1);
  const min = Math.min(...values, 0);
  const spread = max - min || 1;
  const stepX = width / Math.max(values.length - 1, 1);
  const points = values.map((value, index) => {
    const x = index * stepX;
    const y = height - 6 - ((value - min) / spread) * (height - 12);
    return `${x.toFixed(1)},${y.toFixed(1)}`;
  });
  const area = `M 0 ${height} L ${points.join(" L ")} L ${width} ${height} Z`;

  return (
    <svg
      className={className}
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      role="img"
      aria-label="Performance trend"
    >
      <defs>
        <linearGradient id={gradientId} x1="0" y1="0" x2="0" y2={height}>
          <stop stopColor="var(--accent)" stopOpacity="0.32" />
          <stop offset="1" stopColor="var(--accent)" stopOpacity="0" />
        </linearGradient>
      </defs>
      <path d={area} fill={`url(#${gradientId})`} />
      <polyline
        points={points.join(" ")}
        fill="none"
        stroke="var(--accent)"
        strokeWidth="1.6"
        strokeLinecap="round"
        strokeLinejoin="round"
        vectorEffect="non-scaling-stroke"
      />
    </svg>
  );
}
