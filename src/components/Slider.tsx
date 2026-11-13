import type { CSSProperties } from "react";
import { cn } from "../lib/utils";

interface SliderProps {
  label: string;
  value: number;
  min: number;
  max: number;
  step?: number;
  disabled?: boolean;
  format?: (value: number) => string;
  onChange: (value: number) => void;
  className?: string;
}

export function Slider({
  label,
  value,
  min,
  max,
  step = 1,
  disabled = false,
  format,
  onChange,
  className,
}: SliderProps) {
  const percent = ((value - min) / (max - min)) * 100;
  return (
    <div className={cn("slider", disabled && "is-disabled", className)}>
      <div className="slider__head">
        <span className="slider__label">{label}</span>
        <span className="slider__value">{format ? format(value) : value}</span>
      </div>
      <input
        type="range"
        className="slider__input"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        aria-label={label}
        onChange={(event) => onChange(Number(event.target.value))}
        style={{ "--fill": `${percent}%` } as CSSProperties}
      />
    </div>
  );
}
