import { invoke, isTauri } from "@tauri-apps/api/core";

/** Mirrors `commands::AppStatus` in src-tauri (serialised with camelCase). */
export interface AppStatus {
  appVersion: string;
  schemaVersion: number;
  foreignKeysEnabled: boolean;
}

/** Mirrors the `{ kind, message }` shape produced by `error::Error`'s Serialize impl. */
export interface CommandError {
  kind: string;
  message: string;
}

export function runningInsideTauri(): boolean {
  return isTauri();
}

export function getAppStatus(): Promise<AppStatus> {
  return invoke<AppStatus>("get_app_status");
}
