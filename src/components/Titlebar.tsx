import { Maximize2, Minus, X } from "lucide-react";
import { LogoMark } from "./Logo";
import { useToast } from "./Toast";
import { windowAction } from "../lib/native";
import { cn } from "../lib/utils";

export function Titlebar({ title, className }: { title?: string; className?: string }) {
  const { push } = useToast();

  const runWindowAction = async (action: "minimize" | "maximize" | "close") => {
    const handled = await windowAction(action);
    if (!handled) {
      push({
        title: "Desktop window controls",
        description: "Minimize, zoom, and close are available in the native build.",
        tone: "info",
      });
    }
  };

  return (
    <header
      className={cn("titlebar", className)}
      data-tauri-drag-region
      onDoubleClick={(event) => {
        if ((event.target as HTMLElement).hasAttribute("data-tauri-drag-region")) {
          void runWindowAction("maximize");
        }
      }}
    >
      <div className="titlebar__left">
        <div className="window-controls">
          <button
            type="button"
            className="window-control is-close"
            aria-label="Close window"
            onClick={() => void runWindowAction("close")}
          >
            <X size={8} strokeWidth={2.5} />
          </button>
          <button
            type="button"
            className="window-control is-minimize"
            aria-label="Minimize window"
            onClick={() => void runWindowAction("minimize")}
          >
            <Minus size={8} strokeWidth={2.5} />
          </button>
          <button
            type="button"
            className="window-control is-zoom"
            aria-label="Maximize window"
            onClick={() => void runWindowAction("maximize")}
          >
            <Maximize2 size={7} strokeWidth={2.5} />
          </button>
        </div>
        <div className="titlebar__brand" data-tauri-drag-region>
          <LogoMark size={16} />
          <span>ParsecPlus</span>
        </div>
      </div>

      <div className="titlebar__center" data-tauri-drag-region>
        {title && <span className="titlebar__title">{title}</span>}
      </div>

      <div className="titlebar__right" data-tauri-drag-region />
    </header>
  );
}
