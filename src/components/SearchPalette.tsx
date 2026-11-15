import { useEffect, useMemo, useState } from "react";
import {
  Link2,
  Monitor,
  Radio,
  Search,
  SearchX,
  Settings,
} from "lucide-react";
import { hotkeys } from "../lib/data";
import { useApp } from "../lib/store";
import type { RecentConnection, Screen } from "../lib/types";
import { Kbd } from "./Kbd";
import { Modal } from "./Modal";
import { TextField } from "./TextField";

const jumpTargets: Array<{ screen: Screen; label: string; icon: React.ReactNode }> = [
  { screen: "computers", label: "Computers", icon: <Monitor size={16} /> },
  { screen: "host", label: "Host settings", icon: <Radio size={16} /> },
  { screen: "settings", label: "Settings", icon: <Settings size={16} /> },
];

export function SearchPalette({
  open,
  onClose,
  onNavigate,
  onDirectConnect,
}: {
  open: boolean;
  onClose: () => void;
  onNavigate: (screen: Screen) => void;
  onDirectConnect: (address?: string) => void;
}) {
  const { data } = useApp();
  const [query, setQuery] = useState("");

  useEffect(() => {
    if (open) setQuery("");
  }, [open]);

  const value = query.trim().toLowerCase();

  const screenResults = useMemo(
    () =>
      jumpTargets.filter(
        (target) => !value || target.label.toLowerCase().includes(value),
      ),
    [value],
  );

  const hotkeyResults = useMemo(
    () =>
      hotkeys.filter(
        (hotkey) =>
          value &&
          (hotkey.label.toLowerCase().includes(value) ||
            hotkey.keys.join(" ").toLowerCase().includes(value)),
      ),
    [value],
  );

  const recentResults = useMemo(
    () =>
      data.recent.filter(
        (entry) =>
          !value ||
          entry.name.toLowerCase().includes(value) ||
          `${entry.address}:${entry.port}`.toLowerCase().includes(value),
      ),
    [data.recent, value],
  );

  const noResults =
    Boolean(value) &&
    screenResults.length === 0 &&
    hotkeyResults.length === 0 &&
    recentResults.length === 0;

  const chooseScreen = (screen: Screen) => {
    onNavigate(screen);
    onClose();
  };

  const chooseRecent = (entry: RecentConnection) => {
    onClose();
    onDirectConnect(`${entry.address}:${entry.port}`);
  };

  const firstScreen = screenResults[0];

  return (
    <Modal
      open={open}
      onClose={onClose}
      title="Search ParsecPlus"
      description="Jump to a screen, a shortcut, or a recent connection."
      size="md"
      className="search-modal"
    >
      <div className="search-modal__field">
        <TextField
          autoFocus
          icon={<Search size={16} />}
          placeholder="Search screens, shortcuts, recents…"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter" && firstScreen) {
              chooseScreen(firstScreen.screen);
            }
          }}
          aria-label="Search ParsecPlus"
        />
      </div>

      <div className="search-results">
        {!value && (
          <section className="search-section">
            <p className="search-section__label">Jump to</p>
            <div className="search-jump-grid">
              {jumpTargets.map((target) => (
                <button
                  type="button"
                  key={target.screen}
                  className="search-jump"
                  onClick={() => chooseScreen(target.screen)}
                >
                  <span className="search-jump__icon">{target.icon}</span>
                  {target.label}
                </button>
              ))}
            </div>
          </section>
        )}

        {noResults ? (
          <div className="empty-state empty-state--compact">
            <span className="empty-state__icon">
              <SearchX size={20} />
            </span>
            <h3>No results for “{query.trim()}”</h3>
            <p>Search for a screen, a shortcut, or a recent connection.</p>
          </div>
        ) : (
          <>
            {screenResults.length > 0 && (
              <section className="search-section">
                <div className="search-section__head">
                  <p className="search-section__label">Screens</p>
                  <span className="search-section__count">{screenResults.length}</span>
                </div>
                <div className="search-list">
                  {screenResults.map((target) => (
                    <button
                      type="button"
                      key={target.screen}
                      className="search-result"
                      onClick={() => chooseScreen(target.screen)}
                    >
                      <span className="search-result__os">{target.icon}</span>
                      <span className="search-result__copy">
                        <span className="search-result__title">{target.label}</span>
                      </span>
                    </button>
                  ))}
                </div>
              </section>
            )}

            {hotkeyResults.length > 0 && (
              <section className="search-section">
                <p className="search-section__label">Shortcuts</p>
                <div className="search-list">
                  {hotkeyResults.map((hotkey) => (
                    <div className="search-result" key={hotkey.label}>
                      <span className="search-result__os">
                        <Kbd>{hotkey.keys[0]}</Kbd>
                      </span>
                      <span className="search-result__copy">
                        <span className="search-result__title">{hotkey.label}</span>
                        <span className="search-result__meta">
                          {hotkey.keys.join(" ")}
                        </span>
                      </span>
                    </div>
                  ))}
                </div>
              </section>
            )}

            {recentResults.length > 0 && (
              <section className="search-section">
                <div className="search-section__head">
                  <p className="search-section__label">Recent connections</p>
                  <span className="search-section__count">{recentResults.length}</span>
                </div>
                <div className="search-list">
                  {recentResults.map((entry) => (
                    <button
                      type="button"
                      key={entry.id}
                      className="search-result"
                      onClick={() => chooseRecent(entry)}
                    >
                      <span className="search-result__os">
                        <Link2 size={16} />
                      </span>
                      <span className="search-result__copy">
                        <span className="search-result__title">{entry.name}</span>
                        <span className="search-result__meta">
                          {entry.address}:{entry.port} ·{" "}
                          {entry.transport === "relay" ? "Relay" : "Direct"}
                        </span>
                      </span>
                      <span className="search-result__hint">Connect</span>
                    </button>
                  ))}
                </div>
              </section>
            )}
          </>
        )}
      </div>

      <div className="search-modal__footer">
        <span>
          <span className="kbd">esc</span> to close
        </span>
      </div>
    </Modal>
  );
}
