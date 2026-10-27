import { useCallback, useEffect, useState } from "react";
import {
  ERROR_KIND,
  acceptMatch,
  cancelCollection,
  errorKind,
  errorMessage,
  isCollectionRunning,
  listCollectionRuns,
  listMatches,
  listProperties,
  listSources,
  onCollectionFinished,
  rejectMatch,
  reviewQueueArgs,
  runMatchingPass,
  startCollection,
  undoMerge,
  type CollectionFinished,
  type MatchView,
  type Page,
  type PropertyView,
  type RunRecord,
  type SourceInfo,
} from "./lib/api";

// Phase 2 diagnostics: just enough interface to exercise the typed commands. The real search
// interface is Phase 4.

const reais = (centavos: number) =>
  (centavos / 100).toLocaleString("pt-BR", { style: "currency", currency: "BRL" });

const squareMeters = (centi: number | null) =>
  centi === null ? "—" : `${(centi / 100).toLocaleString("pt-BR")} m²`;

/** Messages for the errors a user can reach by pressing a button. */
function explain(err: unknown): string {
  switch (errorKind(err)) {
    case ERROR_KIND.noSourcesConfigured:
      return "Nenhuma fonte está configurada, então não há onde coletar. Isso não significa que não existam imóveis: nada foi pesquisado.";
    case ERROR_KIND.collectionAlreadyRunning:
      return "Já existe uma coleta em andamento.";
    default:
      return errorMessage(err);
  }
}

const section = { marginTop: "1.5rem" } as const;
const cell = { padding: "2px 10px 2px 0", textAlign: "left", verticalAlign: "top" } as const;

export default function Diagnostics() {
  const [sources, setSources] = useState<SourceInfo[]>([]);
  const [runs, setRuns] = useState<RunRecord[]>([]);
  const [running, setRunning] = useState(false);
  const [properties, setProperties] = useState<Page<PropertyView> | null>(null);
  const [fullPriceOnly, setFullPriceOnly] = useState(false);
  const [queue, setQueue] = useState<Page<MatchView> | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [lastFinished, setLastFinished] = useState<CollectionFinished | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [s, r, active, p, q] = await Promise.all([
        listSources(),
        listCollectionRuns(10),
        isCollectionRunning(),
        listProperties({ fullPriceOnly, limit: 20 }),
        listMatches(reviewQueueArgs(0, 20)),
      ]);
      setSources(s);
      setRuns(r);
      setRunning(active);
      setProperties(p);
      setQueue(q);
    } catch (err) {
      setNotice(explain(err));
    }
  }, [fullPriceOnly]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    const unlisten = onCollectionFinished((finished) => {
      setLastFinished(finished);
      void refresh();
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [refresh]);

  const act = async (work: () => Promise<unknown>) => {
    setNotice(null);
    try {
      await work();
    } catch (err) {
      setNotice(explain(err));
    }
    await refresh();
  };

  return (
    <div>
      <h2>Fase 2: coleta e correspondência (diagnóstico)</h2>
      {notice && <p role="alert">{notice}</p>}

      <section style={section}>
        <h3>Fontes</h3>
        {sources.length === 0 ? (
          <p>
            Nenhuma fonte configurada. Nenhuma das candidatas avaliadas (62imoveis, OLX, ZAP Imóveis, corretoras
            locais) permite coleta automatizada por um método utilizável; as evidências estão em docs/sources/.
          </p>
        ) : (
          <table>
            <tbody>
              {sources.map((s) => (
                <tr key={s.adapterId}>
                  <td style={cell}>{s.platform}</td>
                  <td style={cell}>
                    {s.latestOutcome
                      ? `${s.latestOutcome.kind}${s.latestOutcome.complete ? "" : " (incompleta)"} — ${s.latestOutcome.stored} salvos`
                      : "nunca executada"}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <button onClick={() => void act(() => startCollection({}))} disabled={running}>
          Iniciar coleta
        </button>{" "}
        <button onClick={() => void act(cancelCollection)} disabled={!running}>
          Cancelar
        </button>{" "}
        <button onClick={() => void act(runMatchingPass)}>Procurar duplicatas</button>
        {running && <p>Coleta em andamento…</p>}
        {lastFinished && (
          <p>
            Última coleta: {lastFinished.status ?? "não executada"}
            {lastFinished.error ? ` — ${lastFinished.error}` : ""}
            {lastFinished.followUpErrors.length > 0 ? ` — ${lastFinished.followUpErrors.join("; ")}` : ""}
          </p>
        )}
      </section>

      <section style={section}>
        <h3>Execuções recentes</h3>
        {runs.length === 0 ? (
          <p>Nenhuma coleta registrada.</p>
        ) : (
          <table>
            <tbody>
              {runs.map((r) => (
                <tr key={r.id}>
                  <td style={cell}>#{r.id}</td>
                  <td style={cell}>{new Date(r.startedAt).toLocaleString("pt-BR")}</td>
                  <td style={cell}>{r.status}</td>
                  <td style={cell}>{r.note ?? ""}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section style={section}>
        <h3>Imóveis salvos{properties ? ` (${properties.total})` : ""}</h3>
        <label>
          <input type="checkbox" checked={fullPriceOnly} onChange={(e) => setFullPriceOnly(e.target.checked)} /> Ocultar
          Ágio (Apenas Valor Total)
        </label>
        {properties && properties.items.length === 0 && <p>Nenhum imóvel salvo.</p>}
        <table>
          <tbody>
            {properties?.items.map((p) => (
              <tr key={p.id}>
                <td style={cell}>{[p.neighborhood, p.municipality].filter(Boolean).join(", ") || "local desconhecido"}</td>
                <td style={cell}>
                  {p.priceSpread
                    ? p.priceSpread[0] === p.priceSpread[1]
                      ? reais(p.priceSpread[0])
                      : `${reais(p.priceSpread[0])} a ${reais(p.priceSpread[1])}`
                    : "sem valor total"}
                </td>
                <td style={cell}>{squareMeters(p.builtArea ?? p.lotArea)}</td>
                <td style={cell}>
                  {p.listings.length} anúncio(s){p.matchReviewStatus === "review_required" ? " — revisar duplicata" : ""}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section style={section}>
        <h3>Possíveis duplicatas{queue ? ` (${queue.total})` : ""}</h3>
        {queue && queue.items.length === 0 && <p>Nada para revisar.</p>}
        {queue?.items.map((m) => (
          <div key={m.id} style={{ marginBottom: "0.75rem" }}>
            <strong>{m.category === "high" ? "Alta" : "Possível"}</strong>:{" "}
            <a href={m.a.url} target="_blank" rel="noreferrer">
              {m.a.title ?? m.a.url}
            </a>{" "}
            ↔{" "}
            <a href={m.b.url} target="_blank" rel="noreferrer">
              {m.b.title ?? m.b.url}
            </a>
            <div>
              {m.evidence.signals.map((s) => s.kind).join(", ") || "sem sinais"}
              {m.evidence.conflicts.length > 0 &&
                ` — conflitos: ${m.evidence.conflicts.map((c) => c.kind).join(", ")}`}
            </div>
            <button onClick={() => void act(() => acceptMatch(m.id))}>Mesmo imóvel</button>{" "}
            <button onClick={() => void act(() => rejectMatch(m.id))}>Imóveis diferentes</button>{" "}
            {m.merge && !m.revertedAt && <button onClick={() => void act(() => undoMerge(m.id))}>Desfazer</button>}
          </div>
        ))}
      </section>
    </div>
  );
}
