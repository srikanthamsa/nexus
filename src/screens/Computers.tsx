import { useEffect, useState } from "react";
import {
  Clock3,
  Cpu,
  Link2,
  Monitor,
  MonitorUp,
  Plus,
  Star,
} from "lucide-react";
import { Button, IconButton } from "../components/Button";
import { PageHeader } from "../components/PageHeader";
import { getSystemInfo } from "../lib/native";
import { useApp } from "../lib/store";
import type { RecentConnection, Screen, SystemInfo } from "../lib/types";
import { cn, friendlyDeviceName } from "../lib/utils";

type Filter = "all" | "online" | "favorites";

function lastConnectedLabel(timestamp: number): string {
  const diff = Math.max(0, Date.now() - timestamp);
  const minutes = Math.round(diff / 60_000);
  if (minutes < 1) return "Just now";
  if (minutes < 60) return `${minutes} min ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours} h ago`;
  const days = Math.round(hours / 24);
  return days <= 1 ? "Yesterday" : `${days} days ago`;
}

function SavedCard({
  entry,
  onConnect,
  onFavorite,
}: {
  entry: RecentConnection;
  onConnect: () => void;
  onFavorite: () => void;
}) {
  return (
    <article className="computer-card theme-cyan">
      <div className="computer-card__head">
        <span className="computer-card__glyph">
          <MonitorUp size={19} />
        </span>
        <div className="computer-card__identity">
          <p className="computer-card__name">{entry.name}</p>
          <p className="computer-card__owner">
            {entry.address}:{entry.port}
          </p>
        </div>
        <div className="computer-card__head-actions">
          <IconButton
            size="sm"
            label={entry.favorite ? "Remove from favorites" : "Add to favorites"}
            className={cn(entry.favorite && "is-favorited")}
            active={Boolean(entry.favorite)}
            icon={<Star size={14} fill={entry.favorite ? "currentColor" : "none"} />}
            onClick={onFavorite}
          />
        </div>
      </div>

      <div className="computer-card__specs">
        <span>
          <Link2 size={12} />
          {entry.transport === "relay" ? "Relay" : "Direct"}
        </span>
        {entry.width && entry.height ? (
          <span>
            <Monitor size={12} />
            {entry.width} × {entry.height}
          </span>
        ) : null}
      </div>

      <div className="computer-card__footer">
        <span className="computer-card__owner">
          <Clock3 size={12} /> {lastConnectedLabel(entry.lastConnectedAt)}
        </span>
        <Button
          size="sm"
          variant={entry.code ? "primary" : "secondary"}
          className={entry.code ? "btn--light" : undefined}
          onClick={onConnect}
        >
          {entry.code ? "Reconnect" : "Connect"}
        </Button>
      </div>
    </article>
  );
}

export function ComputersScreen({
  onDirectConnect,
  onNavigate,
}: {
  onDirectConnect: (address?: string, code?: string) => void;
  onNavigate: (screen: Screen) => void;
}) {
  const { data, toggleRecentFavorite } = useApp();
  const [filter, setFilter] = useState<Filter>("all");
  const [system, setSystem] = useState<SystemInfo | null>(null);

  useEffect(() => {
    let cancelled = false;
    void getSystemInfo().then((info) => {
      if (!cancelled) setSystem(info);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  const deviceName =
    data.user?.name || friendlyDeviceName(system?.hostname) || "This Mac";
  const recent = data.recent;
  const favoriteCount = recent.filter((entry) => entry.favorite).length;
  const online = 1;
  const counts: Record<Filter, number> = {
    all: 1 + recent.length,
    online,
    favorites: favoriteCount,
  };

  const visibleSaved =
    filter === "favorites" ? recent.filter((entry) => entry.favorite) : recent;
  const showThisMac = filter !== "favorites";
  const showSaved = filter !== "online";

  const filters: Array<{ id: Filter; label: string; icon?: typeof Star }> = [
    { id: "all", label: "All" },
    { id: "online", label: "Online" },
    { id: "favorites", label: "Favorites", icon: Star },
  ];

  return (
    <div className="screen">
      <PageHeader
        title="Computers"
        description="Every machine you own, one click away."
        meta={
          <span className="meta-pill">
            <span className="presence-dot is-online" />
            {online} online
          </span>
        }
      />

      <div className="toolbar">
        <div className="filter-tabs" role="tablist" aria-label="Computer filter">
          {filters.map((entry) => (
            <button
              key={entry.id}
              type="button"
              role="tab"
              aria-selected={filter === entry.id}
              className={cn("filter-tab", filter === entry.id && "is-active")}
              onClick={() => setFilter(entry.id)}
            >
              {entry.icon ? <entry.icon size={13} /> : null}
              {entry.label}
              <span className="filter-tab__count">{counts[entry.id]}</span>
            </button>
          ))}
        </div>
      </div>

      {showThisMac ? (
        <section className="section">
          <div className="section-head">
            <h2>Your computers</h2>
            <span className="section-head__count">1</span>
          </div>
          <div className="computer-grid">
            <article className="computer-card theme-violet">
              <div className="computer-card__head">
                <span className="computer-card__glyph">
                  <Monitor size={19} />
                </span>
                <div className="computer-card__identity">
                  <p className="computer-card__name">{deviceName}</p>
                  <p className="computer-card__owner">
                    This Mac{system?.os ? ` · ${system.os}` : ""}
                  </p>
                </div>
              </div>
              <div className="computer-card__specs">
                <span>
                  <Cpu size={12} />
                  {system?.chip || "Apple silicon"}
                </span>
                <span>
                  <Monitor size={12} />
                  {system?.osVersion || "macOS"}
                </span>
              </div>
              <div className="computer-card__footer">
                <span className="computer-card__owner">
                  <span
                    className={cn(
                      "presence-dot",
                      data.host.hosting ? "is-online" : "is-idle",
                    )}
                  />
                  {data.host.hosting ? "Remote access on" : "Remote access off"}
                </span>
                <Button size="sm" variant="secondary" onClick={() => onNavigate("host")}>
                  Open Host
                </Button>
              </div>
            </article>

            <button
              type="button"
              className="add-computer-card"
              onClick={() => onDirectConnect()}
            >
              <span className="add-computer-card__icon">
                <Plus size={18} />
              </span>
              <span className="add-computer-card__title">Add a computer</span>
              <span className="add-computer-card__copy">
                Connect with a code from another Mac.
              </span>
            </button>
          </div>
        </section>
      ) : null}

      {showSaved ? (
        <section className="section">
          <div className="section-head">
            <h2>Previously connected</h2>
            <span className="section-head__count">{visibleSaved.length}</span>
            <span className="section-head__meta">
              Saved on this Mac · one-click reconnect
            </span>
          </div>
          {visibleSaved.length > 0 ? (
            <div className="computer-grid">
              {visibleSaved.map((entry) => (
                <SavedCard
                  key={entry.id}
                  entry={entry}
                  onFavorite={() => toggleRecentFavorite(entry.id)}
                  onConnect={() =>
                    onDirectConnect(`${entry.address}:${entry.port}`, entry.code)
                  }
                />
              ))}
            </div>
          ) : (
            <div className="empty-state">
              <span className="empty-state__icon">
                <MonitorUp size={22} />
              </span>
              <h3>
                {filter === "favorites"
                  ? "No favorite computers yet"
                  : "No saved computers yet"}
              </h3>
              <p>
                {filter === "favorites"
                  ? "Star a saved connection to keep it here."
                  : "Computers you connect to will appear here. Reconnect without typing a code again."}
              </p>
              <div className="empty-state__actions">
                <Button
                  variant="secondary"
                  className="btn--light"
                  leftIcon={<Link2 size={15} />}
                  onClick={() => onDirectConnect()}
                >
                  Direct connect
                </Button>
              </div>
            </div>
          )}
        </section>
      ) : null}
    </div>
  );
}
