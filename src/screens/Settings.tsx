import { useEffect, useState, type ReactNode } from "react";
import {
  Copy,
  Info,
  Keyboard,
  LogOut,
  Monitor,
  Palette,
  ShieldCheck,
  Trash2,
  User,
} from "lucide-react";
import { Avatar } from "../components/Avatar";
import { Button } from "../components/Button";
import { Kbd } from "../components/Kbd";
import { LogoMark } from "../components/Logo";
import { Modal } from "../components/Modal";
import { PageHeader } from "../components/PageHeader";
import { TextField } from "../components/TextField";
import { Toggle } from "../components/Toggle";
import { useToast } from "../components/Toast";
import { hotkeys } from "../lib/data";
import { getSystemInfo, setAlwaysOnTop } from "../lib/native";
import { useApp } from "../lib/store";
import type { SystemInfo } from "../lib/types";
import { cn, copyToClipboard } from "../lib/utils";

type TabId = "account" | "appearance" | "shortcuts" | "about";

const tabs: Array<{ id: TabId; label: string; icon: ReactNode }> = [
  { id: "account", label: "Profile", icon: <User size={16} /> },
  { id: "appearance", label: "Appearance", icon: <Palette size={16} /> },
  { id: "shortcuts", label: "Shortcuts", icon: <Keyboard size={16} /> },
  { id: "about", label: "About", icon: <Info size={16} /> },
];

function SettingsSection({
  title,
  description,
  children,
}: {
  title: string;
  description?: string;
  children: ReactNode;
}) {
  return (
    <section className="settings-panel">
      <div className="settings-panel__head">
        <div>
          <h2>{title}</h2>
          {description && <p>{description}</p>}
        </div>
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
  const [system, setSystem] = useState<SystemInfo | null>(null);
  const NAME_LIMIT = 20;

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
      <PageHeader title="Settings" description="Your profile and this Mac." />

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
              <span>This Mac</span>
            </div>
          </div>
        </nav>

        <div className="settings-body">
          {tab === "account" && (
            <>
              <SettingsSection
                title="Profile"
                description="Shown to people you connect with. It stays on this Mac."
              >
                <div className="profile-card">
                  <Avatar name={user.name} size={62} presence="online" />
                  <div className="profile-card__copy">
                    <p>{user.name}</p>
                    <span>{system ? `${system.os} ${system.osVersion}`.trim() : "This Mac"}</span>
                  </div>
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
                  <span className="settings-note">Used on this Mac and in the Host screen.</span>
                  <Button variant="primary" onClick={saveProfile}>
                    Save name
                  </Button>
                </div>
              </SettingsSection>

              <SettingsSection title="Session">
                <SettingsRow
                  title="Sign out"
                  description="Returns to the setup screen. Your saved computers stay."
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
            <SettingsSection title="Interface">
              <SettingsRow title="Compact layout" description="Fits more on smaller screens.">
                <Toggle
                  checked={data.settings.compact}
                  onChange={(value) => updateSettings({ compact: value })}
                  label="Compact layout"
                />
              </SettingsRow>
              <SettingsRow title="Reduce motion" description="Turns off animation.">
                <Toggle
                  checked={data.settings.reduceMotion}
                  onChange={(value) => updateSettings({ reduceMotion: value })}
                  label="Reduce motion"
                />
              </SettingsRow>
              <SettingsRow
                title="Keep window on top"
                description="Stays above other apps while you play."
              >
                <Toggle
                  checked={data.settings.alwaysOnTop}
                  onChange={(value) => {
                    updateSettings({ alwaysOnTop: value });
                    void setAlwaysOnTop(value);
                  }}
                  label="Keep window on top"
                />
              </SettingsRow>
            </SettingsSection>
          )}

          {tab === "shortcuts" && (
            <SettingsSection
              title="Keyboard shortcuts"
              description="These work during a session."
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
            </SettingsSection>
          )}

          {tab === "about" && (
            <>
              <SettingsSection title="ParsecPlus" description="Remote access for your Mac.">
                <div className="about-hero">
                  <div className="about-hero__mark">
                    <LogoMark size={26} />
                  </div>
                  <div className="about-hero__copy">
                    <p>ParsecPlus</p>
                    <span>{system?.appVersion ?? "0.1.0"} · macOS</span>
                  </div>
                </div>

                <div className="about-grid">
                  <div>
                    <span>System</span>
                    <strong>{system ? `${system.os} ${system.osVersion}`.trim() : "—"}</strong>
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
                    Copy app info
                  </Button>
                </div>
              </SettingsSection>

              <SettingsSection title="This Mac">
                <SettingsRow
                  title="Reset local data"
                  description="Clears your profile, settings and saved computers."
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

              <SettingsSection title="About this project">
                <div className="about-note">
                  <ShieldCheck size={18} />
                  <p>
                    ParsecPlus is an independent, clean-room project. It is not affiliated with
                    Parsec Technologies.
                  </p>
                </div>
                <div className="about-note">
                  <Monitor size={18} />
                  <p>Built for macOS. Friends can connect from another ParsecPlus install.</p>
                </div>
              </SettingsSection>
            </>
          )}
        </div>
      </div>

      <Modal
        open={resetOpen}
        onClose={() => setResetOpen(false)}
        title="Reset this Mac?"
        description="Clears your profile, settings and saved computers."
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
                Reset
              </Button>
            </div>
          </>
        }
      >
        <div className="confirm-icon is-danger">
          <Trash2 size={18} />
        </div>
        <p className="confirm-copy">
          ParsecPlus returns to setup and asks you to name this Mac again.
        </p>
      </Modal>
    </div>
  );
}
