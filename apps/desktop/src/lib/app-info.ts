import { invoke, isTauri } from "@tauri-apps/api/core";

/** Espelha `kr_core::AppInfo` (serializado em camelCase). */
export interface AppInfo {
  name: string;
  version: string;
  protocolVersion: number;
  targetOs: string;
  targetArch: string;
}

/** Busca as informações da build no núcleo Rust. Fora do Tauri (navegador), retorna null. */
export async function fetchAppInfo(): Promise<AppInfo | null> {
  if (!isTauri()) return null;
  return invoke<AppInfo>("app_info");
}
