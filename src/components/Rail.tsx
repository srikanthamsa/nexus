import { Gamepad2, Monitor, Radio, Settings, Users, UsersRound } from "lucide-react";
import type { ReactNode } from "react";
import type { Screen, User } from "../lib/types";
import { cn } from "../lib/utils";
import { Avatar } from "./Avatar";

const navigation: Array<{ screen: Screen; label: string; icon: ReactNode; badge?: string }> = [
  { screen: "computers", label: "Computers", icon: <Monitor size={19} /> },
  { screen: "friends", label: "Friends", icon: <Users size={19} /> },
  { screen: "host", label: "Host", icon: <Radio size={19} /> },
  { screen: "teams", label: "Teams", icon: <UsersRound size={19} /> },
  { screen: "arcade", label: "Arcade", icon: <Gamepad2 size={19} />, badge: "SOON" },
];

export function Rail({
  screen,
  onNavigate,
  hosting,
  user,
}: {
  screen: Screen;
  onNavigate: (screen: Screen) => void;
  hosting: boolean;
  user: User;
}) {
  return (
    <nav className="rail" aria-label="Primary navigation">
      <div className="rail__nav">
        {navigation.map((item) => {
          const active = screen === item.screen;
          return (
            <button
              key={item.screen}
              type="button"
              className={cn("rail__item", active && "is-active")}
              aria-current={active ? "page" : undefined}
              onClick={() => onNavigate(item.screen)}
              title={item.label}
            >
              <span className="rail__icon">
                {item.icon}
                {item.screen === "host" && hosting && <span className="rail__live-dot" />}
                {item.badge && <span className="rail__badge">{item.badge}</span>}
              </span>
              <span className="rail__label">{item.label}</span>
            </button>
          );
        })}
      </div>

      <div className="rail__bottom">
        <button
          type="button"
          className={cn("rail__item", "rail__item--compact", screen === "settings" && "is-active")}
          aria-current={screen === "settings" ? "page" : undefined}
          onClick={() => onNavigate("settings")}
          title="Settings"
        >
          <span className="rail__icon">
            <Settings size={19} />
          </span>
          <span className="rail__label">Settings</span>
        </button>
        <button
          type="button"
          className="rail__avatar"
          onClick={() => onNavigate("settings")}
          aria-label={`Open settings for ${user.name}`}
          title={user.name}
        >
          <Avatar name={user.name} size={32} presence="online" />
        </button>
      </div>
    </nav>
  );
}
