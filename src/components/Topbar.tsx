import { Bell, SignOut as LogOut, MagnifyingGlass as Search, GearSix as Settings, ShieldCheck } from "@phosphor-icons/react";
import { useApp } from "../lib/store";
import type { Screen } from "../lib/types";
import { cn } from "../lib/utils";
import { Avatar } from "./Avatar";
import { Kbd } from "./Kbd";
import { Popover } from "./Popover";
import { useToast } from "./Toast";

export function Topbar({
  onSearch,
  onNavigate,
  hosting,
}: {
  onSearch: () => void;
  onNavigate: (screen: Screen) => void;
  hosting: boolean;
}) {
  const { data, signOut } = useApp();
  const { push } = useToast();
  const user = data.user;

  if (!user) return null;

  return (
    <div className="topbar">
      <button type="button" className="search-trigger" onClick={onSearch}>
        <Search size={15} />
        <span>Search screens, settings, recent connections…</span>
        <span className="search-trigger__kbd">
          <Kbd>⌘</Kbd>
          <Kbd>K</Kbd>
        </span>
      </button>

      <div className="topbar__actions">
        <button
          type="button"
          className={cn("host-pill", hosting && "is-live")}
          onClick={() => onNavigate("host")}
          title={hosting ? "You are hosting this computer" : "Start hosting this computer"}
        >
          <span className="host-pill__dot" />
          {hosting ? "Hosting" : "Hosting off"}
        </button>

        <Popover
          ariaLabel="Notifications"
          width={344}
          buttonClassName="topbar__icon-button"
          panelClassName="notification-panel"
          button={
            <span className="topbar__bell">
              <Bell size={18} />
            </span>
          }
        >
          {() => (
            <div>
              <div className="popover__head">
                <div>
                  <h3 className="popover__title">Notifications</h3>
                  <p className="popover__subtitle">Nothing to show yet</p>
                </div>
              </div>
              <div className="notification-empty">
                <ShieldCheck size={18} />
                <p>Host and connection alerts appear here once sessions start.</p>
              </div>
              <div className="popover__foot">
                <button type="button" className="link-button" onClick={() => onNavigate("host")}>
                  Open host settings
                </button>
              </div>
            </div>
          )}
        </Popover>

        <Popover
          ariaLabel="Account menu"
          width={264}
          buttonClassName="topbar__avatar-button"
          button={<Avatar name={user.name} size={32} presence="online" />}
        >
          {(close) => (
            <div>
              <div className="account-pop">
                <Avatar name={user.name} size={40} presence="online" />
                <div className="account-pop__copy">
                  <p className="account-pop__name">{user.name}</p>
                  <p className="account-pop__email">Local profile</p>
                </div>
              </div>
              <div className="menu">
                <button
                  type="button"
                  className="menu__item"
                  onClick={() => {
                    close();
                    onNavigate("settings");
                  }}
                >
                  <Settings size={15} className="menu__icon" />
                  <span>Local profile & settings</span>
                </button>
                <div className="menu__divider" />
                <button
                  type="button"
                  className="menu__item is-danger"
                  onClick={() => {
                    close();
                    signOut();
                    push({ title: "Signed out", tone: "info" });
                  }}
                >
                  <LogOut size={15} className="menu__icon" />
                  <span>Sign out</span>
                </button>
              </div>
            </div>
          )}
        </Popover>
      </div>
    </div>
  );
}
