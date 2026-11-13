import { useEffect, useState, type ReactNode } from "react";
import {
  Check,
  Copy,
  Info,
  Keyboard,
  LogOut,
  Monitor,
  Palette,
  ShieldCheck,
  SlidersHorizontal,
  Trash2,
  User,
  Wifi,
} from "lucide-react";
import { Avatar } from "../components/Avatar";
import { Button } from "../components/Button";
import { Kbd } from "../components/Kbd";
import { LogoMark } from "../components/Logo";
import { Modal } from "../components/Modal";
import { PageHeader } from "../components/PageHeader";
import { Segmented } from "../components/Segmented";
import { Select } from "../components/Select";
import { TextField } from "../components/TextField";
import { Toggle } from "../components/Toggle";
import { useToast } from "../components/Toast";
import { accents, hotkeys } from "../lib/data";
import { getSystemInfo, setAlwaysOnTop } from "../lib/native";
import { useApp } from "../lib/store";
import type { Accent, SystemInfo } from "../lib/types";
import { cn, copyToClipboard } from "../lib/utils";

type TabId = "account" | "appearance" | "hotkeys" | "network" | "advanced" | "about";

const tabs: Array<{ id: TabId; label: string; icon: ReactNode }> = [
  { id: "account", label: "Account", icon: <User size={16} /> },
  { id: "appearance", label: "Appearance", icon: <Palette size={16} /> },
  { id: "hotkeys", label: "Hotkeys", icon: <Keyboard size={16} /> },
  { id: "network", label: "Network", icon: <Wifi size={16} /> },
  { id: "advanced", label: "Advanced", icon: <SlidersHorizontal size={16} /> },
  { id: "about", label: "About", icon: <Info size={16} /> },
];

function SettingsSection({
  title,
  description,
  actions,
  children,
}: {
  title: string;
  description?: string;
  actions?: ReactNode;
  children: ReactNode;
}) {
  return (
    <section className="settings-panel">
      <div className="settings-panel__head">
        <div>
          <h2>{title}</h2>
          {description && <p>{description}</p>}
        </div>
        {actions}
      </div>
      <div className="settings-panel__body">{children}</div>
    </section>
  );
}

function SettingsRow({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <div className="settings-row">
      <div className="settings-row__copy">
        <p>{title}</p>
        {description && <span>{description}</span>}
      </div>
      <div className="settings-row__control">{children}</div>
    </div>
  );
}

export function SettingsScreen() {
  const { data, signIn, updateSettings, signOut, resetAll } = useApp();
  const { push } = useToast();
  const [tab, setTab] = useState<TabId>("account");
  const [draftName, setDraftName] = useState(data.user?.name ?? "");
  const [nameError, setNameError] = useState<string>();
  const [resetOpen, setResetOpen] = useState(false);
  const NAME_LIMIT = 20;
  const [system, setSystem] = useState<SystemInfo | null>(null);

  const user = data.user;

  useEffect(() => {
    if (!user) return;
    setDraftName(user.name);
  }, [user]);

  useEffect(() => {
    let cancelled = false;
    void getSystemInfo().then((info) => {
      if (!cancelled) setSystem(info);
    });
    return () => {
      cancelled = true;
    };
  }, []);

  if (!user) return null;

  const saveProfile = () => {
    if (!draftName.trim()) {
      push({ title: "Add a display name", tone: "error" });
      return;
    }
    signIn(draftName);
    push({ title: "Profile updated", tone: "success" });
  };

  const copyDiagnostics = async () => {
    const payload = {
      app: "ParsecPlus",
      version: system?.appVersion ?? "0.1.0",
      os: system?.os ?? "unknown",
      osVersion: system?.osVersion ?? "unknown",
      arch: system?.arch ?? "unknown",
      chip: system?.chip ?? "unknown",
      hosting: data.host.hosting,
    };
    const ok = await copyToClipboard(JSON.stringify(payload, null, 2));
    push({
      title: ok ? "Diagnostics copied" : "Could not copy diagnostics",
      tone: ok ? "success" : "error",
    });
  };

  return (
    <div className="screen">
      <PageHeader
        title="Settings"
        description="Shape ParsecPlus around how you work."
      />

      <div className="settings-layout">
        <nav className="settings-nav" aria-label="Settings sections">
          {tabs.map((entry) => (
            <button
              key={entry.id}
              type="button"
              className={cn("settings-nav__item", tab === entry.id && "is-active")}
              onClick={() => setTab(entry.id)}
              aria-current={tab === entry.id ? "page" : undefined}
            >
              {entry.icon}
              <span>{entry.label}</span>
            </button>
          ))}
          <div className="settings-nav__foot">
            <Avatar name={user.name} size={28} presence="online" />
            <div>
              <p>{user.name}</p>
              <span>Local profile</span>
            </div>
          </div>
        </nav>

        <div className="settings-body">
          {tab === "account" && (
            <>
              <SettingsSection
                title="Local profile"
                description="This profile stays on this Mac. No account needed."
              >
                <div className="profile-card">
                  <Avatar name={user.name} size={62} presence="online" />
                  <div className="profile-card__copy">
                    <p>{user.name}</p>
                    <span>
                      Local profile · no account
                      {system ? ` · ${system.os}` : ""}
                    </span>
                  </div>
                  <span className="chip">This Mac</span>
                </div>

                <div className="settings-form">
                  <TextField
                    label="Display name"
                    value={draftName}
                    error={nameError}
                    hint={`Up to ${NAME_LIMIT} characters.`}
                    containerClassName={nameError ? "field--wiggle" : undefined}
                    onChange={(event) => {
                      const next = event.target.value;
                      setDraftName(next);
                      setNameError(
                        next.trim().length > NAME_LIMIT
                          ? `Keep the display name to ${NAME_LIMIT} characters or fewer.`
                          : undefined,
                      );
                    }}
                  />
                </div>
                <div className="settings-actions">
                  <span className="settings-note">
                    Shown on this Mac and in the host dock when someone connects.
                  </span>
                  <Button variant="primary" onClick={saveProfile}>
                    Save name
                  </Button>
                </div>
              </SettingsSection>

              <SettingsSection title="Session">
                <SettingsRow
                  title="Sign out of ParsecPlus"
                  description="Returns to the local setup screen. Your preferences stay on this Mac."
                >
                  <Button
                    variant="ghost"
                    leftIcon={<LogOut size={15} />}
                    onClick={() => {
                      signOut();
                      push({ title: "Signed out", tone: "info" });
                    }}
                  >
                    Sign out
                  </Button>
                </SettingsRow>
              </SettingsSection>
            </>
          )}

          {tab === "appearance" && (
            <>
              <SettingsSection
                title="Accent"
                description="Tint highlights, controls, and the session HUD."
              >
                <div className="accent-grid">
                  {accents.map((accent) => (
                    <button
                      key={accent.id}
                      type="button"
                      className={cn(
                        "accent-swatch",
                        data.settings.accent === accent.id && "is-active",
                      )}
                      onClick={() => updateSettings({ accent: accent.id as Accent })}
                      aria-pressed={data.settings.accent === accent.id}
                    >
                      <span
                        className="accent-swatch__orb"
                        style={{
                          background: `linear-gradient(135deg, ${accent.colors[0]}, ${accent.colors[1]})`,
                        }}
                      >
                        {data.settings.accent === accent.id && <Check size={16} />}
                      </span>
                      <span className="accent-swatch__label">{accent.label}</span>
                    </button>
                  ))}
                </div>

                <div className="appearance-preview">
                  <div className="appearance-preview__bar">
                    <span />
                    <span />
                    <span />
                  </div>
                  <div className="appearance-preview__body">
                    <span className="appearance-preview__tile" />
                    <div className="appearance-preview__copy">
                      <span className="appearance-preview__line" />
                      <span className="appearance-preview__line is-short" />
                    </div>
                    <span className="appearance-preview__button">Connect</span>
                  </div>
                </div>
              </SettingsSection>

              <SettingsSection title="Interface">
                <SettingsRow
                  title="Compact density"
                  description="Tighter spacing for smaller displays."
                >
                  <Toggle
                    checked={data.settings.compact}
                    onChange={(value) => updateSettings({ compact: value })}
                    label="Compact density"
                  />
                </SettingsRow>
                <SettingsRow
                  title="Reduce motion"
                  description="Remove transitions and ambient animation."
                >
                  <Toggle
                    checked={data.settings.reduceMotion}
                    onChange={(value) => updateSettings({ reduceMotion: value })}
                    label="Reduce motion"
                  />
                </SettingsRow>
                <SettingsRow
                  title="Always on top"
                  description="Keep the ParsecPlus window above other apps."
                >
                  <Toggle
                    checked={data.settings.alwaysOnTop}
                    onChange={(value) => {
                      updateSettings({ alwaysOnTop: value });
                      void setAlwaysOnTop(value);
                    }}
                    label="Always on top"
                  />
                </SettingsRow>
                <SettingsRow
                  title="Show performance stats"
                  description="Start each session with the stats panel open."
                >
                  <Toggle
                    checked={data.settings.showStats}
                    onChange={(value) => updateSettings({ showStats: value })}
                    label="Show performance stats"
                  />
                </SettingsRow>
              </SettingsSection>
            </>
          )}

          {tab === "hotkeys" && (
            <SettingsSection
              title="Hotkeys"
              description="Global shortcuts during a remote session."
              actions={<span className="chip">System shortcuts</span>}
            >
              <div className="hotkey-list">
                {hotkeys.map((hotkey) => (
                  <div className="hotkey-row" key={hotkey.label}>
                    <span>{hotkey.label}</span>
                    <span className="keycap-row">
                      {hotkey.keys.map((key) => (
                        <Kbd key={`${hotkey.label}-${key}`}>{key}</Kbd>
                      ))}
                    </span>
                  </div>
                ))}
              </div>
              <p className="settings-note">
                Custom key bindings and per-app profiles are planned for a later phase.
              </p>
            </SettingsSection>
          )}

          {tab === "network" && (
            <>
              <SettingsSection
                title="Connection preference"
                description="Direct paths are fastest. Relay is the dependable fallback."
              >
                <Segmented
                  value="auto"
                  onChange={() => {}}
                  ariaLabel="Connection preference"
                  disabled
                  options={[{ value: "auto", label: "Auto (direct, then relay)" }]}
                />
                <SettingsRow
                  title="Bandwidth limit · planned"
                  description="Manual caps are not wired; use the Host bitrate presets for now."
                >
                  <Toggle
                    checked={false}
                    disabled
                    onChange={() => {}}
                    label="Bandwidth limit (planned)"
                  />
                </SettingsRow>
                <p className="settings-note">
                  Transport selection stays automatic. Per-session bitrate is configured under
                  Host → Video.
                </p>
              </SettingsSection>

              <SettingsSection
                title="NAT traversal (STUN/TURN)"
                description="Find a direct UDP path across the internet. Applies to hosting and connecting."
              >
                <div className="settings-form settings-form--two">
                  <TextField
                    id="ice-stun-urls"
                    label="STUN servers"
                    placeholder="stun:stun.l.google.com:19302, stun:stun.cloudflare.com:3478"
                    value={data.settings.iceStunUrls}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    onChange={(event) => updateSettings({ iceStunUrls: event.target.value })}
                    hint="Comma or space separated. Empty by default, so no third party is contacted."
                  />
                  <TextField
                    id="ice-turn-urls"
                    label="TURN servers"
                    placeholder="turn:your-server:3478"
                    value={data.settings.iceTurnUrls}
                    spellCheck={false}
                    autoCapitalize="off"
                    autoCorrect="off"
                    onChange={(event) => updateSettings({ iceTurnUrls: event.target.value })}
                    hint="Only needed when direct UDP fails; TURN relays encrypted media."
                  />
                  <TextField
                    id="ice-turn-username"
                    label="TURN username"
                    value={data.settings.iceTurnUsername}
                    autoComplete="off"
                    spellCheck={false}
                    onChange={(event) => updateSettings({ iceTurnUsername: event.target.value })}
                  />
                  <TextField
                    id="ice-turn-credential"
                    label="TURN credential"
                    type="password"
                    value={data.settings.iceTurnCredential}
                    autoComplete="off"
                    spellCheck={false}
                    onChange={(event) => updateSettings({ iceTurnCredential: event.target.value })}
                  />
                </div>
                <p className="settings-note">
                  A public STUN server reveals your IP address to that server. TURN relays
                  encrypted media and is only needed when direct UDP fails. The same servers are
                  used for hosting and connecting.
                </p>
              </SettingsSection>
            </>
          )}

          {tab === "advanced" && (
            <>
              <SettingsSection title="Streaming">
                <div className="settings-form settings-form--narrow">
                  <Select
                    id="decoder"
                    label="Video decoder"
                    value="auto"
                    onChange={() => {}}
                    options={[{ value: "auto", label: "Auto (hardware HEVC)" }]}
                    hint="Hardware HEVC is the only decoder in this build; fallbacks are planned."
                  />
                </div>
                <SettingsRow
                  title="Stats overlay by default"
                  description="Show live latency and bitrate during sessions."
                >
                  <Toggle
                    checked={data.settings.showStats}
                    onChange={(value) => updateSettings({ showStats: value })}
                    label="Stats overlay by default"
                  />
                </SettingsRow>
              </SettingsSection>

              <SettingsSection title="Diagnostics & data">
                <SettingsRow
                  title="Copy diagnostics"
                  description="Platform details and preview settings for support."
                >
                  <Button
                    variant="secondary"
                    size="sm"
                    leftIcon={<Copy size={14} />}
                    onClick={() => void copyDiagnostics()}
                  >
                    Copy
                  </Button>
                </SettingsRow>
                <SettingsRow
                  title="Reset local data"
                  description="Clear this local profile, preferences, and recent connections."
                >
                  <Button
                    variant="danger"
                    size="sm"
                    leftIcon={<Trash2 size={14} />}
                    onClick={() => setResetOpen(true)}
                  >
                    Reset
                  </Button>
                </SettingsRow>
              </SettingsSection>
            </>
          )}

          {tab === "about" && (
            <>
              <SettingsSection
                title="ParsecPlus"
                description="Fast, private access to every computer you own."
              >
                <div className="about-hero">
                  <div className="about-hero__mark">
                    <LogoMark size={26} />
                  </div>
                  <div className="about-hero__copy">
                    <p>ParsecPlus</p>
                    <span>
                      {system?.appVersion ?? "0.1.0"} · macOS · Phase 1
                    </span>
                  </div>
                  <span className="chip">Clean-room build</span>
                </div>

                <div className="about-grid">
                  <div>
                    <span>Platform</span>
                    <strong>{system ? `${system.os} ${system.osVersion}`.trim() : "—"}</strong>
                  </div>
                  <div>
                    <span>Architecture</span>
                    <strong>{system?.arch ?? "—"}</strong>
                  </div>
                  <div>
                    <span>Device</span>
                    <strong>{system?.model || system?.hostname || "—"}</strong>
                  </div>
                  <div>
                    <span>Chip</span>
                    <strong>{system?.chip || "—"}</strong>
                  </div>
                </div>

                <div className="settings-actions">
                  <Button
                    variant="ghost"
                    leftIcon={<Copy size={15} />}
                    onClick={() => void copyDiagnostics()}
                  >
                    Copy diagnostics
                  </Button>
                </div>
              </SettingsSection>

              <SettingsSection title="Project">
                <div className="about-note">
                  <ShieldCheck size={18} />
                  <p>
                    ParsecPlus is an independent clean-room project. It is not affiliated with,
                    endorsed by, or derived from Parsec Technologies. All interface design and
                    code are original.
                  </p>
                </div>
                <div className="about-note">
                  <Monitor size={18} />
                  <p>
                    Native host control, live preview, direct sessions, invite codes, STUN/TURN, and
                    relay fallback are implemented on macOS. Accounts, friends, and teams arrive
                    in later phases.
                  </p>
                </div>
              </SettingsSection>
            </>
          )}
        </div>
      </div>

      <Modal
        open={resetOpen}
        onClose={() => setResetOpen(false)}
        title="Reset this Mac's profile?"
        description="Clears the local profile, preferences, and recent connections on this device."
        size="sm"
        footer={
          <>
            <span className="modal__note">This cannot be undone.</span>
            <div className="modal__footer-actions">
              <Button variant="ghost" onClick={() => setResetOpen(false)}>
                Cancel
              </Button>
              <Button
                variant="danger"
                onClick={() => {
                  resetAll();
                  setResetOpen(false);
                  push({ title: "Local data reset", tone: "default" });
                }}
              >
                Reset local data
              </Button>
            </div>
          </>
        }
      >
        <div className="confirm-icon is-danger">
          <Trash2 size={18} />
        </div>
        <p className="confirm-copy">
          ParsecPlus returns to its first-run state and you’ll be asked to set up this Mac again.
        </p>
      </Modal>
    </div>
  );
}
