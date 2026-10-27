import { useEffect, useState } from "react";
import Diagnostics from "./Diagnostics";
import { errorMessage, getAppStatus, runningInsideTauri, type AppStatus } from "./lib/api";

type State =
  | { phase: "loading" }
  | { phase: "ready"; status: AppStatus }
  | { phase: "error"; message: string }
  | { phase: "no-backend" };

// Phase 2 diagnostics: proves the typed Tauri command bridge, database initialisation and the
// collection/matching commands. The real search UI is part of Phase 4.
export default function App() {
  const [state, setState] = useState<State>({ phase: "loading" });

  useEffect(() => {
    if (!runningInsideTauri()) {
      setState({ phase: "no-backend" });
      return;
    }
    getAppStatus()
      .then((status) => setState({ phase: "ready", status }))
      .catch((err: unknown) => setState({ phase: "error", message: errorMessage(err) }));
  }, []);

  return (
    <main style={{ fontFamily: "system-ui, sans-serif", padding: "2rem", maxWidth: 900 }}>
      <h1>Loose Canon</h1>
      <p>Coleta de anúncios e correspondência conservadora de duplicatas (Fase 2, em andamento).</p>
      {state.phase === "loading" && <p>Verificando banco de dados…</p>}
      {state.phase === "no-backend" && (
        <p>Backend Tauri indisponível (executando fora do aplicativo desktop).</p>
      )}
      {state.phase === "error" && <p role="alert">Falha ao iniciar: {state.message}</p>}
      {state.phase === "ready" && (
        <>
          <dl>
            <dt>Versão do aplicativo</dt>
            <dd>{state.status.appVersion}</dd>
            <dt>Versão do esquema do banco</dt>
            <dd>{state.status.schemaVersion}</dd>
            <dt>Chaves estrangeiras ativas</dt>
            <dd>{state.status.foreignKeysEnabled ? "sim" : "não"}</dd>
          </dl>
          <Diagnostics />
        </>
      )}
    </main>
  );
}
