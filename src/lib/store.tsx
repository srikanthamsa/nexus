import {
  createContext,
  useContext,
  useEffect,
  useState,
  type ReactNode,
} from "react";
import { seedData } from "./data";
import type {
  AppData,
  AppSettings,
  HostSettings,
  PermissionState,
  RecentConnection,
} from "./types";

const STORAGE_KEY = "parsecplus.data.v2";
/** The v1 key held seeded demo workspaces; drop it so it can never resurface. */
const LEGACY_STORAGE_KEY = "parsecplus.data.v1";
const RECENT_LIMIT = 12;

function loadData(): AppData {
  if (typeof window === "undefined") return seedData;
  try {
    window.localStorage.removeItem(LEGACY_STORAGE_KEY);
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return seedData;
    const parsed = JSON.parse(raw) as Partial<AppData>;
    return {
      user: parsed.user ?? null,
      recent: Array.isArray(parsed.recent) ? parsed.recent.slice(0, RECENT_LIMIT) : [],
      host: {
        ...seedData.host,
        ...(parsed.host ?? {}),
        // 120 Hz+ targets from earlier experiments are not implemented yet.
        refresh: "60",
      },
      settings: { ...seedData.settings, ...(parsed.settings ?? {}) },
      permissions: { ...seedData.permissions, ...(parsed.permissions ?? {}) },
    };
  } catch {
    return seedData;
  }
}

function localId(): string {
  if (typeof crypto !== "undefined" && "randomUUID" in crypto) {
    return crypto.randomUUID();
  }
  return `local-${Date.now().toString(36)}`;
}

interface StoreActions {
  signIn: (name: string) => void;
  signOut: () => void;
  updateHost: (patch: Partial<HostSettings>) => void;
  updateSettings: (patch: Partial<AppSettings>) => void;
  updatePermissions: (patch: Partial<PermissionState>) => void;
  addRecent: (entry: Omit<RecentConnection, "id" | "lastConnectedAt">) => void;
  toggleRecentFavorite: (id: string) => void;
  clearRecent: () => void;
  resetAll: () => void;
}

interface StoreValue extends StoreActions {
  data: AppData;
}

const StoreContext = createContext<StoreValue | null>(null);

export function AppProvider({ children }: { children: ReactNode }) {
  const [data, setData] = useState<AppData>(loadData);

  useEffect(() => {
    try {
      window.localStorage.setItem(STORAGE_KEY, JSON.stringify(data));
    } catch {
      // Storage may be unavailable; the app still works in memory.
    }
  }, [data]);

  const signIn = (name: string) => {
    const trimmed = name.trim() || "This Mac";
    setData((previous) => ({
      ...previous,
      user: {
        id: previous.user?.id ?? localId(),
        name: trimmed,
      },
    }));
  };

  const signOut = () => {
    setData((previous) => ({ ...previous, user: null }));
  };

  const updateHost = (patch: Partial<HostSettings>) => {
    setData((previous) => ({
      ...previous,
      host: { ...previous.host, ...patch },
    }));
  };

  const updateSettings = (patch: Partial<AppSettings>) => {
    setData((previous) => ({
      ...previous,
      settings: { ...previous.settings, ...patch },
    }));
  };

  const updatePermissions = (patch: Partial<PermissionState>) => {
    setData((previous) => ({
      ...previous,
      permissions: { ...previous.permissions, ...patch },
    }));
  };

  const addRecent: StoreActions["addRecent"] = (entry) => {
    setData((previous) => {
      const next: RecentConnection = {
        ...entry,
        id: localId(),
        lastConnectedAt: Date.now(),
      };
      const deduped = previous.recent.filter(
        (item) => !(item.address === entry.address && item.port === entry.port),
      );
      return { ...previous, recent: [next, ...deduped].slice(0, RECENT_LIMIT) };
    });
  };

  const toggleRecentFavorite = (id: string) => {
    setData((previous) => ({
      ...previous,
      recent: previous.recent.map((item) =>
        item.id === id ? { ...item, favorite: !item.favorite } : item,
      ),
    }));
  };

  const clearRecent = () => {
    setData((previous) => ({ ...previous, recent: [] }));
  };

  const resetAll = () => {
    try {
      window.localStorage.removeItem(STORAGE_KEY);
      window.localStorage.removeItem(LEGACY_STORAGE_KEY);
    } catch {
      // No-op: state is reset below regardless.
    }
    setData(seedData);
  };

  return (
    <StoreContext.Provider
      value={{
        data,
        signIn,
        signOut,
        updateHost,
        updateSettings,
        updatePermissions,
        addRecent,
        toggleRecentFavorite,
        clearRecent,
        resetAll,
      }}
    >
      {children}
    </StoreContext.Provider>
  );
}

export function useApp(): StoreValue {
  const context = useContext(StoreContext);
  if (!context) throw new Error("useApp must be used inside AppProvider");
  return context;
}
